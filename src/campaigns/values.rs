use crate::{
    AppError,
    definitions::{Control, Definition},
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

pub(crate) fn form(
    definition: &Definition,
    form: &BTreeMap<String, String>,
) -> Result<Value, AppError> {
    let mut values = serde_json::Map::new();
    for field in definition
        .fields
        .iter()
        .filter(|field| field.formula.is_none())
    {
        if field.control == Control::List {
            return Err(AppError::Invalid(
                "List editing is unsupported until milestone M4.",
            ));
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
            match field.control {
                Control::Text | Control::Multiline | Control::Enumeration => {
                    Value::String(raw.clone())
                }
                Control::Integer => integer(raw)?.into(),
                Control::Boolean => match raw.as_str() {
                    "true" => true.into(),
                    "false" => false.into(),
                    _ => return Err(AppError::Invalid("Select true or false.")),
                },
                Control::List => return Err(AppError::Invalid("List editing is unsupported.")),
            }
        };
        values.insert(field.id.clone(), value);
    }
    Ok(Value::Object(values))
}
