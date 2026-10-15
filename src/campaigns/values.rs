use crate::{
    AppError,
    definitions::{Control, Definition},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) fn schema(definition: &Definition) -> Result<jsonschema::Validator, AppError> {
    let mut properties = serde_json::Map::new();
    for field in &definition.fields {
        let mut property = match field.control {
            Control::Text => json!({"type":"string","maxLength":8000}),
            Control::Multiline => json!({"type":"string","maxLength":20000}),
            Control::Integer => json!({"type":"integer"}),
            Control::Boolean => json!({"type":"boolean"}),
            Control::Enumeration => {
                json!({"type":"string","enum":field.options.as_ref().ok_or(AppError::Invalid("Missing enum options."))?.iter().map(|c|Value::String(c.id.clone())).collect::<Vec<_>>()})
            }
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
    let schema = json!({"type":"object","properties":properties,"additionalProperties":false,"required":definition.fields.iter().map(|f|f.id.clone()).collect::<Vec<_>>()});
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
    for field in &definition.fields {
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
            }
        };
        values.insert(field.id.clone(), value);
    }
    Ok(Value::Object(values))
}
