use crate::{
    AppError,
    definitions::{Control, Definition},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

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
