//! Mimik-owned JSON <-> CEL conversion. `cel::to_value` is not used: it turns non-negative
//! JSON integers into `uint`, which would break integer arithmetic.
use super::{
    Code, InputError, MAX_INPUT_BYTES, MAX_LIST_ITEMS, Scalar, ValueType, profile::Lookup,
    profile::slug,
};
use cel::Value;
use serde_json::Value as Json;
use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

fn invalid(path: String) -> InputError {
    InputError {
        code: Code::InvalidInput,
        path,
    }
}

/// Fixed charge per converted value, on top of string bytes.
const VALUE_OVERHEAD_BYTES: usize = 16;

/// Remaining aggregate input allowance, charged before any string is copied.
struct Budget(usize);

impl Budget {
    fn take(&mut self, value: &Json, path: &str) -> Result<(), InputError> {
        let bytes = VALUE_OVERHEAD_BYTES + value.as_str().map_or(0, str::len);
        self.0 = self.0.checked_sub(bytes).ok_or_else(|| InputError {
            code: Code::InputTooLarge,
            path: path.to_owned(),
        })?;
        Ok(())
    }
}

/// Exact typed conversion of one non-null scalar.
fn scalar(
    ty: &ValueType,
    value: &Json,
    path: &str,
    budget: &mut Budget,
) -> Result<Value, InputError> {
    budget.take(value, path)?;
    let converted = match (ty, value) {
        // `as_i64` is `None` for u64 values above i64::MAX and for any float.
        (ValueType::Int, Json::Number(number)) => number.as_i64().map(Value::Int),
        (ValueType::Bool, Json::Bool(value)) => Some(Value::Bool(*value)),
        (ValueType::Text(max), Json::String(text)) if text.chars().count() <= *max => {
            Some(Value::String(Arc::new(text.clone())))
        }
        (ValueType::Enum(options), Json::String(text)) if options.contains(text) => {
            Some(Value::String(Arc::new(text.clone())))
        }
        _ => None,
    };
    converted.ok_or_else(|| invalid(path.to_owned()))
}

fn record(
    properties: &std::collections::BTreeMap<String, ValueType>,
    optional: &BTreeSet<String>,
    value: &Json,
    path: &str,
    ids: &mut BTreeSet<String>,
    budget: &mut Budget,
) -> Result<Value, InputError> {
    let Json::Object(object) = value else {
        return Err(invalid(path.to_owned()));
    };
    let mut map: HashMap<String, Value> = HashMap::new();
    let id_path = format!("{path}/id");
    match object.get("id") {
        Some(json @ Json::String(id)) if slug(id) => {
            budget.take(json, &id_path)?;
            if !ids.insert(id.clone()) {
                return Err(invalid(id_path));
            }
            map.insert("id".into(), Value::String(Arc::new(id.clone())));
        }
        _ => return Err(invalid(format!("{path}/id"))),
    }
    for key in object.keys() {
        if key != "id" && !properties.contains_key(key) {
            return Err(invalid(format!("{path}/{key}")));
        }
    }
    for (id, ty) in properties {
        let child = format!("{path}/{id}");
        match object.get(id) {
            // Absent optional properties are absent keys. `null` is only an integer
            // property's unknown value (assisted sheet slice 1.4).
            None if optional.contains(id) => {}
            Some(value @ Json::Null) if *ty == ValueType::Int => {
                budget.take(value, &child)?;
                map.insert(id.clone(), Value::Null);
            }
            None | Some(Json::Null) => return Err(invalid(child)),
            Some(value) => {
                map.insert(id.clone(), scalar(ty, value, &child, budget)?);
            }
        }
    }
    Ok(Value::from(map))
}

/// Converts and validates every declared input, in declaration-name order.
pub(super) fn inputs(lookup: &Lookup, values: &Json) -> Result<Vec<(String, Value)>, InputError> {
    let Json::Object(object) = values else {
        return Err(invalid(String::new()));
    };
    let mut declared = BTreeSet::new();
    let mut converted = Vec::new();
    let mut budget = Budget(MAX_INPUT_BYTES);
    for (id, (ty, nullable)) in lookup.scalars() {
        declared.insert(id.as_str());
        let path = format!("/{id}");
        let value = match object.get(id) {
            None => return Err(invalid(path)),
            Some(Json::Null) if *nullable => Value::Null,
            // `null` is the unknown value of every integer field (slice 1.4).
            Some(value @ Json::Null) if *ty == ValueType::Int => {
                budget.take(value, &path)?;
                Value::Null
            }
            Some(value) => scalar(ty, value, &path, &mut budget)?,
        };
        converted.push((id.clone(), value));
    }
    for id in lookup.lists() {
        declared.insert(id.as_str());
        let path = format!("/{id}");
        let Some(Json::Array(items)) = object.get(id) else {
            return Err(invalid(path));
        };
        if items.len() > MAX_LIST_ITEMS {
            return Err(InputError {
                code: Code::CollectionTooLarge,
                path,
            });
        }
        let (properties, optional) = lookup.list(id).ok_or_else(|| invalid(path.clone()))?;
        let mut ids = BTreeSet::new();
        let records = items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                record(
                    properties,
                    optional,
                    item,
                    &format!("{path}/{index}"),
                    &mut ids,
                    &mut budget,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        converted.push((id.clone(), Value::List(Arc::new(records))));
    }
    if let Some(key) = object.keys().find(|key| !declared.contains(key.as_str())) {
        // Derived values are never inputs; unknown keys are never ignored.
        return Err(invalid(format!("/{key}")));
    }
    Ok(converted)
}

/// What a formula may not read (assisted sheet slice 1.4): integer fields that are unknown
/// (`null`), nullable or not (user decision D-Z), and `(list, property)` pairs with an
/// unknown item value. A formula reading either reports `null_value` without running,
/// because CEL would evaluate `null == 0` to `false` (and `!=` to `true`) instead of failing.
/// Nullable text, enum and boolean fields keep CEL's null semantics.
#[derive(Default)]
pub(super) struct Unknowns {
    pub fields: BTreeSet<String>,
    pub properties: BTreeSet<(String, String)>,
}

/// The unknowns of input values that [`inputs`] accepted.
pub(super) fn unknowns(lookup: &Lookup, values: &Json) -> Unknowns {
    let mut unknowns = Unknowns::default();
    for (id, (ty, _)) in lookup.scalars() {
        if *ty == ValueType::Int && values.get(id).is_some_and(Json::is_null) {
            unknowns.fields.insert(id.clone());
        }
    }
    for id in lookup.lists() {
        for item in values[id].as_array().into_iter().flatten() {
            for (property, value) in item.as_object().into_iter().flatten() {
                if value.is_null() {
                    unknowns.properties.insert((id.clone(), property.clone()));
                }
            }
        }
    }
    unknowns
}

/// Checks a derived result against its declared type, including enum options and text length.
/// A `null` result (a nullable field copied while unknown) is `null_value` (Sol rev-2 M5).
pub(super) fn result(ty: Option<&ValueType>, value: &Value) -> Result<Scalar, Code> {
    match (ty, value) {
        (_, Value::Null) => Err(Code::NullValue),
        (Some(ValueType::Int), Value::Int(value)) => Ok(Scalar::Int(*value)),
        (Some(ValueType::Bool), Value::Bool(value)) => Ok(Scalar::Bool(*value)),
        (Some(ValueType::Text(max)), Value::String(text)) if text.chars().count() <= *max => {
            Ok(Scalar::Text(text.as_str().to_owned()))
        }
        (Some(ValueType::Enum(options)), Value::String(text))
            if options.contains(text.as_ref()) =>
        {
            Ok(Scalar::Text(text.as_str().to_owned()))
        }
        (Some(ValueType::Text(_) | ValueType::Enum(_)), Value::String(_)) => {
            Err(Code::InvalidResult)
        }
        _ => Err(Code::TypeMismatch),
    }
}

pub(super) fn to_cel(value: &Scalar) -> Value {
    match value {
        Scalar::Int(value) => Value::Int(*value),
        Scalar::Bool(value) => Value::Bool(*value),
        Scalar::Text(text) => Value::String(Arc::new(text.clone())),
    }
}
