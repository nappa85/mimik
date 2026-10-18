//! The catalogue decode profile (plan D2): serde tokenizes, this guard rejects duplicate keys
//! (including escaped-equivalent spellings, which serde unescapes before the check) and bounds
//! expansion with its own constants. It shares nothing with `definitions::decode`, so
//! `MAX_NODES`, `parse` and `parse_sheet` keep their limits.
//!
//! Limits depend on the member's position (`Context`), not only on its key, because entry
//! `properties` are keyed by author-chosen property ids.
use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use std::fmt;

/// Raw draft input bytes (D5); the canonical encoding has the same cap.
pub const MAX_BYTES: usize = 4 * 1024 * 1024;
/// Decoder nodes of the catalogue profile (D5).
pub(crate) const MAX_NODES: usize = 524_288;
/// Container nesting levels (D5): containers sit at depths 0..8; the deepest valid one is a
/// choice object at depth 6.
pub(crate) const MAX_DEPTH: usize = 8;
/// Members per object; the schema closes every object with at most 8 declared members, and
/// entry `properties` hold at most 16 declared ids.
const MAX_KEYS: usize = 32;
/// Key length in scalars (ids are at most 64 bytes).
const MAX_KEY: usize = 64;

pub(crate) const MAX_ENTRIES: usize = 8_192;
pub(crate) const MAX_TYPES: usize = 32;
pub(crate) const MAX_PROPERTIES: usize = 16;
pub(crate) const MAX_OPTIONS: usize = 128;
pub(crate) const MAX_UNITS: usize = 16;
pub(crate) const MAX_SOURCES: usize = 32;
pub(crate) const MAX_NAME: usize = 100;
pub(crate) const MAX_ID: usize = 64;
/// A text property value (display data; not set by the plan, chosen for step 1).
pub(crate) const MAX_TEXT_PROPERTY: usize = 1_000;
pub(crate) const MAX_OWNER_TEXT: usize = 20_000;

#[derive(Clone, Copy)]
enum Context {
    Root,
    Source,
    EntryType,
    Property,
    Choice,
    Entry,
    EntryProperties,
    Description,
    Unit,
    /// Unknown members and scalars: bounded tightly; the schema rejects them.
    Other,
}

/// `(context of the value, array limit, string limit)` of member `key` inside `parent`.
fn member(parent: Context, key: &str) -> (Context, usize, usize) {
    use Context::{
        Choice, Description, Entry, EntryProperties, EntryType, Other, Property, Root, Source, Unit,
    };
    match (parent, key) {
        (Root, "sources") => (Source, MAX_SOURCES, MAX_ID),
        (Root, "entry_types") => (EntryType, MAX_TYPES, MAX_NAME),
        (Root, "entries") => (Entry, MAX_ENTRIES, MAX_NAME),
        (Root | EntryType | Property | Choice, "title" | "label") | (Entry, "name") => {
            (Other, 0, MAX_NAME)
        }
        (EntryType, "properties") => (Property, MAX_PROPERTIES, MAX_NAME),
        (Property, "options") => (Choice, MAX_OPTIONS, MAX_NAME),
        (Entry, "properties") => (EntryProperties, 0, MAX_ID),
        (Entry, "description") => (Description, 0, MAX_ID),
        (EntryProperties, _) => (Other, 0, MAX_TEXT_PROPERTY),
        (Description, "units") => (Unit, MAX_UNITS, MAX_ID),
        (Description, "owner_text") => (Other, 0, MAX_OWNER_TEXT),
        _ => (Other, 0, MAX_ID),
    }
}

struct Seed<'a> {
    depth: usize,
    nodes: &'a mut usize,
    context: Context,
    array_limit: usize,
    string_limit: usize,
    reject: bool,
}

impl<'de> DeserializeSeed<'de> for Seed<'_> {
    type Value = Value;
    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Value, D::Error> {
        if self.reject || *self.nodes >= MAX_NODES {
            return Err(serde::de::Error::custom("catalogue expansion limit"));
        }
        *self.nodes += 1;
        decoder.deserialize_any(self)
    }
}

fn bounded<E: serde::de::Error>(value: &str, limit: usize) -> Result<(), E> {
    if value.chars().count() > limit {
        return Err(E::custom("catalogue string limit"));
    }
    Ok(())
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
            .map_err(|_| E::custom("integer outside signed range"))
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Value, E> {
        Err(E::custom("fraction/exponent tokens are unsupported"))
    }
    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Value, E> {
        bounded(value, self.string_limit)?;
        Ok(Value::String(value.into()))
    }
    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Value, E> {
        bounded(&value, self.string_limit)?;
        Ok(Value::String(value))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
        if self.depth >= MAX_DEPTH {
            return Err(serde::de::Error::custom("catalogue depth limit"));
        }
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(Seed {
            depth: self.depth + 1,
            nodes: self.nodes,
            context: self.context,
            // Arrays never nest in the catalogue envelope.
            array_limit: 0,
            string_limit: self.string_limit,
            reject: values.len() >= self.array_limit,
        })? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<Value, A::Error> {
        if self.depth >= MAX_DEPTH {
            return Err(serde::de::Error::custom("catalogue depth limit"));
        }
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if key.chars().count() > MAX_KEY
                || values.len() >= MAX_KEYS
                || values.contains_key(&key)
            {
                return Err(serde::de::Error::custom(
                    "duplicate key or object/key limit",
                ));
            }
            let (context, array_limit, string_limit) = member(self.context, &key);
            let value = object.next_value_seed(Seed {
                depth: self.depth + 1,
                nodes: self.nodes,
                context,
                array_limit,
                string_limit,
                reject: false,
            })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

/// Decode a catalogue draft under the catalogue profile. Failures carry no input text.
pub(crate) fn parse_catalogue(bytes: &[u8]) -> Result<Value, super::CatalogueError> {
    let error = || super::invalid("/", "malformed-json-or-duplicate-key-or-input-limit");
    if bytes.len() > MAX_BYTES {
        return Err(error());
    }
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let mut nodes = 0;
    let value = Seed {
        depth: 0,
        nodes: &mut nodes,
        context: Context::Root,
        array_limit: 0,
        string_limit: MAX_ID,
        reject: false,
    }
    .deserialize(&mut decoder)
    .map_err(|_| error())?;
    decoder.end().map_err(|_| error())?;
    Ok(value)
}
