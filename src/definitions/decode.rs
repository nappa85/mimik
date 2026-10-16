//! Serde supplies JSON tokenization; this guard prevents last-key-wins and bounds expansion.
use super::{DefinitionError, MAX_BYTES, ValidationReport};
use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use std::fmt;

struct Seed<'a> {
    depth: usize,
    nodes: &'a mut usize,
    array_limit: usize,
    string_limit: usize,
    reject: bool,
    sheet: bool,
    data: bool,
}

fn limits(key: &str) -> (usize, usize) {
    match key {
        "sources" | "groups" | "properties" => (32, 64),
        "options" => (128, 64),
        "roles" => (3, 20),
        "regions" => (4, 64),
        "fields" | "id" | "game_id" | "edition_id" | "definition_id" | "pdf_sha256" => (256, 64),
        "title" | "label" => (256, 100),
        "help" => (256, 2000),
        // Formula profile members (byte limits for `expr` are enforced by the formula layer).
        "constraints" => (128, 5000),
        "rule_ids" => (16, 64),
        "expr" => (256, 1024),
        "message" => (256, 500),
        _ => (256, 5000),
    }
}

impl<'de> DeserializeSeed<'de> for Seed<'_> {
    type Value = Value;
    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Value, D::Error> {
        if self.reject || *self.nodes >= 32_768 {
            return Err(serde::de::Error::custom("Definition expansion limit"));
        }
        *self.nodes += 1;
        decoder.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Seed<'_> {
    type Value = Value;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded JSON with exact signed integer tokens")
    }
    fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(Number::from(value)))
    }
    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Value, E> {
        i64::try_from(value)
            .map(|value| Value::Number(Number::from(value)))
            .map_err(|_| E::custom("Integer outside signed range"))
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Value, E> {
        Err(E::custom("Fraction/exponent tokens are unsupported"))
    }
    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Value, E> {
        if value.chars().count() > self.string_limit {
            return Err(E::custom("Definition string limit"));
        }
        Ok(Value::String(value.into()))
    }
    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Value, E> {
        if value.chars().count() > self.string_limit {
            return Err(E::custom("Definition string limit"));
        }
        Ok(Value::String(value))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
        if self.depth >= 12 {
            return Err(serde::de::Error::custom("Definition depth limit"));
        }
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(Seed {
            depth: self.depth + 1,
            nodes: self.nodes,
            array_limit: 256,
            string_limit: self.string_limit,
            reject: values.len() >= self.array_limit,
            sheet: self.sheet,
            data: self.data,
        })? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<Value, A::Error> {
        if self.depth >= 12 {
            return Err(serde::de::Error::custom("Definition depth limit"));
        }
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if key.chars().count() > 64
                || values.len() >= if self.data { 256 } else { 32 }
                || values.contains_key(&key)
            {
                return Err(serde::de::Error::custom(
                    "Duplicate key or object/key limit",
                ));
            }
            let data = self.data || (self.sheet && self.depth == 0 && key == "data");
            let (array_limit, string_limit) = if data {
                (256, 20_000)
            } else if self.sheet {
                (
                    256,
                    match key.as_str() {
                        "name" => 100,
                        "notes" => 20_000,
                        _ => 5000,
                    },
                )
            } else {
                limits(&key)
            };
            let value = object.next_value_seed(Seed {
                depth: self.depth + 1,
                nodes: self.nodes,
                array_limit,
                string_limit,
                reject: false,
                sheet: self.sheet,
                data,
            })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

pub(super) fn parse(bytes: &[u8]) -> Result<Value, DefinitionError> {
    parse_profile(bytes, false)
}

pub(crate) fn parse_sheet(bytes: &[u8]) -> Result<Value, DefinitionError> {
    parse_profile(bytes, true)
}

fn parse_profile(bytes: &[u8], sheet: bool) -> Result<Value, DefinitionError> {
    let error = || ValidationReport::one("/", "malformed-json-or-duplicate-key-or-input-limit");
    if bytes.len() > if sheet { 1024 * 1024 } else { MAX_BYTES } {
        return Err(error());
    }
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let mut nodes = 0;
    let value = Seed {
        depth: 0,
        nodes: &mut nodes,
        array_limit: 256,
        string_limit: 5000,
        reject: false,
        sheet,
        data: false,
    }
    .deserialize(&mut decoder)
    .map_err(|_| error())?;
    decoder.end().map_err(|_| error())?;
    Ok(value)
}
