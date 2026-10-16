use crate::{
    AppError,
    definitions::{Control, Definition, Field},
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// A form index in the canonical grammar `0|[1-9][0-9]{0,2}` (plan D1).
fn index(raw: &str) -> Option<usize> {
    let canonical = matches!(raw.len(), 1..=3)
        && raw.bytes().all(|b| b.is_ascii_digit())
        && (raw == "0" || !raw.starts_with('0'));
    canonical.then(|| raw.parse().ok()).flatten()
}

/// The exact key set a runtime sheet POST may carry (plan D1 and D2), or `None` for a
/// malformed row-key set (answered 400).
///
/// One scan of the submitted `id-f-r` keys bounds every list's row count first: each index
/// must be canonical, `f` must name a list field, `r` must stay below `MAX_LIST_ITEMS`, and
/// the rows of a list must be contiguous from 0. Only then are the expected keys generated:
/// the scalars, every row's ID, property values and (optional properties) presence states,
/// and `op` when one was submitted.
pub(crate) fn sheet_keys<'a>(
    definition: &Definition,
    submitted: impl Iterator<Item = &'a str>,
) -> Option<HashSet<String>> {
    let mut rows: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    let mut op = false;
    for key in submitted {
        if key == "op" {
            op = true;
        }
        let Some(rest) = key.strip_prefix("id-") else {
            continue;
        };
        let (f, r) = rest.split_once('-')?;
        let (f, r) = (index(f)?, index(r)?);
        if r >= crate::definitions::formula::MAX_LIST_ITEMS
            || definition.fields.get(f)?.control != Control::List
        {
            return None;
        }
        rows.entry(f).or_default().insert(r);
    }
    if rows
        .values()
        .any(|rows| rows.last().is_some_and(|last| last + 1 != rows.len()))
    {
        return None;
    }
    let mut keys: HashSet<String> = ["csrf", "revision", "name", "notes"]
        .into_iter()
        .map(String::from)
        .collect();
    if op {
        keys.insert("op".into());
    }
    for (f, field) in definition.fields.iter().enumerate() {
        if field.formula.is_some() {
            continue;
        }
        if field.control == Control::List {
            let properties = field
                .items
                .as_ref()
                .map_or(&[][..], |items| &items.properties);
            for r in 0..rows.get(&f).map_or(0, BTreeSet::len) {
                keys.insert(format!("id-{f}-{r}"));
                for (p, property) in properties.iter().enumerate() {
                    keys.insert(format!("row-{f}-{r}-{p}"));
                    if property.optional {
                        keys.insert(format!("set-{f}-{r}-{p}"));
                    }
                }
            }
        } else {
            keys.insert(format!("value-{}", field.id));
            if field.nullable {
                keys.insert(format!("null-{}", field.id));
            }
        }
    }
    Some(keys)
}

fn scalar_schema(
    control: Control,
    options: Option<&Vec<crate::definitions::Choice>>,
) -> Result<Value, AppError> {
    Ok(match control {
        Control::Text => json!({"type":"string","maxLength":8000}),
        Control::Multiline => json!({"type":"string","maxLength":20000}),
        Control::Integer => json!({"type":"integer"}),
        Control::Boolean => json!({"type":"boolean"}),
        Control::Enumeration => {
            json!({"type":"string","enum":options.ok_or(AppError::Invalid("Missing enum options."))?.iter().map(|c|Value::String(c.id.clone())).collect::<Vec<_>>()})
        }
        Control::List => return Err(AppError::Invalid("Nested lists are unsupported.")),
    })
}

/// Records with a host-managed `id` plus declared properties; `null` never appears inside
/// items. ID grammar, uniqueness and i64 range are domain checks in `private_pin`.
fn list_schema(field: &crate::definitions::Field) -> Result<Value, AppError> {
    let items = field
        .items
        .as_ref()
        .ok_or(AppError::Invalid("Missing list items."))?;
    let mut properties = serde_json::Map::new();
    properties.insert(
        "id".into(),
        json!({"type":"string","minLength":1,"maxLength":64}),
    );
    let mut required = vec![Value::String("id".into())];
    for property in &items.properties {
        properties.insert(
            property.id.clone(),
            scalar_schema(property.control, property.options.as_ref())?,
        );
        if !property.optional {
            required.push(Value::String(property.id.clone()));
        }
    }
    Ok(
        json!({"type":"array","maxItems":crate::definitions::formula::MAX_LIST_ITEMS,"items":{"type":"object","properties":properties,"additionalProperties":false,"required":required}}),
    )
}

/// Value schema of the editable inputs. Derived (formula) fields are never stored.
pub(crate) fn schema(definition: &Definition) -> Result<jsonschema::Validator, AppError> {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for field in definition
        .fields
        .iter()
        .filter(|field| field.formula.is_none())
    {
        required.push(field.id.clone());
        let mut property = if field.control == Control::List {
            list_schema(field)?
        } else {
            scalar_schema(field.control, field.options.as_ref())?
        };
        if field.nullable {
            let ty = property["type"].clone();
            property["type"] = json!([ty, "null"]);
            if let Some(choices) = property.get_mut("enum").and_then(Value::as_array_mut) {
                choices.push(Value::Null);
            }
        }
        properties.insert(field.id.clone(), property);
    }
    let schema = json!({"type":"object","properties":properties,"additionalProperties":false,"required":required});
    if serde_json::to_vec(&schema)?.len() > 256 * 1024 {
        return Err(AppError::Invalid("Derived schema exceeds limit."));
    }
    jsonschema::draft202012::options()
        .offline()
        .build(&schema)
        .map_err(|_| AppError::Invalid("Derived validation schema failed."))
}

pub(crate) fn integer(raw: &str) -> Result<i64, AppError> {
    let unsigned = raw.strip_prefix('-').unwrap_or(raw);
    if unsigned.is_empty()
        || !unsigned.bytes().all(|b| b.is_ascii_digit())
        || (unsigned.len() > 1 && unsigned.starts_with('0'))
    {
        return Err(AppError::Invalid("Use exact decimal signed i64 text."));
    }
    raw.parse()
        .map_err(|_| AppError::Invalid("Integer outside signed i64 range."))
}

/// The declared properties of a list field (empty for any other field).
pub(crate) fn properties(field: &Field) -> &[crate::definitions::ItemProperty] {
    field
        .items
        .as_ref()
        .map_or(&[][..], |items| &items.properties)
}

/// One list row as submitted or saved: raw strings, never validated here (plan D5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DraftRow {
    /// The saved item ID, or empty for a new row.
    pub id: String,
    /// One raw value per declared property.
    pub values: Vec<String>,
    /// Per property: the presence state of an optional property (`present`, `absent`, or
    /// whatever was submitted); empty for a required property.
    pub states: Vec<String>,
}

/// The rows of one list field, in row order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ListDraft {
    pub rows: Vec<DraftRow>,
}

fn raw_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

impl ListDraft {
    /// The rows of list field `f` in an admitted form (rows `0..n`, contiguous by admission).
    pub(crate) fn submitted(f: usize, field: &Field, form: &BTreeMap<String, String>) -> Self {
        let properties = properties(field);
        let get = |key: String| form.get(&key).cloned().unwrap_or_default();
        let rows = (0..=crate::definitions::formula::MAX_LIST_ITEMS)
            .map_while(|r| {
                let id = form.get(&format!("id-{f}-{r}"))?.clone();
                Some(DraftRow {
                    id,
                    values: (0..properties.len())
                        .map(|p| get(format!("row-{f}-{r}-{p}")))
                        .collect(),
                    states: properties
                        .iter()
                        .enumerate()
                        .map(|(p, property)| {
                            if property.optional {
                                get(format!("set-{f}-{r}-{p}"))
                            } else {
                                String::new()
                            }
                        })
                        .collect(),
                })
            })
            .collect();
        Self { rows }
    }

    /// The rows of a saved (validated) list value.
    pub(crate) fn saved(field: &Field, value: &Value) -> Self {
        let properties = properties(field);
        let rows = value
            .as_array()
            .into_iter()
            .flatten()
            .map(|item| DraftRow {
                id: item["id"].as_str().unwrap_or_default().to_owned(),
                values: properties
                    .iter()
                    .map(|property| item.get(&property.id).map(raw_text).unwrap_or_default())
                    .collect(),
                states: properties
                    .iter()
                    .map(|property| {
                        if !property.optional {
                            String::new()
                        } else if item.get(&property.id).is_some() {
                            "present".into()
                        } else {
                            "absent".into()
                        }
                    })
                    .collect(),
            })
            .collect();
        Self { rows }
    }
}

fn scalar_value(control: Control, raw: &str) -> Result<Value, AppError> {
    Ok(match control {
        Control::Text | Control::Multiline | Control::Enumeration => Value::String(raw.into()),
        Control::Integer => integer(raw)?.into(),
        Control::Boolean => match raw {
            "true" => true.into(),
            "false" => false.into(),
            _ => return Err(AppError::Invalid("Select true or false.")),
        },
        Control::List => return Err(AppError::Invalid("Nested lists are unsupported.")),
    })
}

/// Turns a list draft into stored items (plan D3): an empty ID gets the next host ID
/// `r<next>-<k>`, k = 1, 2, … over this list's new rows in row order; a non-empty ID must
/// name a row of the saved list (anything else is tampering); IDs stay unique. Optional
/// properties need an explicit Set (`present`) or Not set (`absent`) state.
pub(crate) fn reconcile(
    field: &Field,
    draft: &ListDraft,
    saved: &Value,
    next: i64,
) -> Result<Value, AppError> {
    if draft.rows.len() > crate::definitions::formula::MAX_LIST_ITEMS {
        return Err(AppError::Invalid("A list holds at most 256 rows."));
    }
    let known: HashSet<&str> = saved
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["id"].as_str())
        .collect();
    let mut seen = HashSet::new();
    let mut allocated = 0_usize;
    let mut items = Vec::with_capacity(draft.rows.len());
    for row in &draft.rows {
        let id = if row.id.is_empty() {
            allocated += 1;
            format!("r{next}-{allocated}")
        } else if known.contains(row.id.as_str()) {
            row.id.clone()
        } else {
            return Err(AppError::Invalid(
                "A list row ID is not part of the saved sheet.",
            ));
        };
        if !seen.insert(id.clone()) {
            return Err(AppError::Invalid("List row IDs must be unique."));
        }
        let mut item = serde_json::Map::new();
        item.insert("id".into(), Value::String(id));
        for (p, property) in properties(field).iter().enumerate() {
            if property.optional {
                match row.states.get(p).map(String::as_str) {
                    Some("present") => {}
                    Some("absent") => continue,
                    _ => {
                        return Err(AppError::Invalid(
                            "Select Set or Not set for every optional row property.",
                        ));
                    }
                }
            }
            let raw = row.values.get(p).map_or("", String::as_str);
            item.insert(property.id.clone(), scalar_value(property.control, raw)?);
        }
        items.push(Value::Object(item));
    }
    Ok(Value::Array(items))
}

/// The values of an admitted runtime form. `saved` and `revision` are the current saved
/// data and revision: list rows reconcile against them (new IDs use `revision + 1`).
pub(crate) fn form(
    definition: &Definition,
    form: &BTreeMap<String, String>,
    saved: &Value,
    revision: i64,
) -> Result<Value, AppError> {
    let next = revision
        .checked_add(1)
        .ok_or(AppError::Invalid("Sheet revision exhausted."))?;
    let mut values = serde_json::Map::new();
    for (f, field) in definition.fields.iter().enumerate() {
        if field.formula.is_some() {
            continue;
        }
        if field.control == Control::List {
            let draft = ListDraft::submitted(f, field, form);
            values.insert(
                field.id.clone(),
                reconcile(field, &draft, &saved[&field.id], next)?,
            );
            continue;
        }
        let raw = form
            .get(&format!("value-{}", field.id))
            .ok_or(AppError::Invalid("Missing pinned field."))?;
        let null = if field.nullable {
            match form.get(&format!("null-{}", field.id)).map(String::as_str) {
                Some("known") => false,
                Some("unknown") => true,
                _ => return Err(AppError::Invalid("Select explicit known/unknown state.")),
            }
        } else {
            false
        };
        let value = if null {
            Value::Null
        } else {
            scalar_value(field.control, raw)?
        };
        values.insert(field.id.clone(), value);
    }
    Ok(Value::Object(values))
}
