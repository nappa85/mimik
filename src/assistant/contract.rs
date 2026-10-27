//! Output schema v2, the instructions and the fixed prefix of every interpretation request
//! (assisted sheet plan §1.4, "Request builder" and "Output contract").
//!
//! The schema and the instructions are data files under `schemas/` (the Python screen,
//! `tools/assist_eval.py`, reads the same files). The fixed prefix is the instructions, a
//! fixed separator and the schema's canonical text, byte-identical across requests, so
//! llama-server reuses it from its prompt cache. Everything a request adds goes inside the
//! data envelope ([`envelope`]), which the instructions declare to be data.
use serde::Serialize;
use serde_json::Value;
use std::sync::LazyLock;

/// The schema's text: compact JSON, ASCII only, one trailing newline, its keys in reply
/// order (llama.cpp's grammar conversion emits object properties in document order). It is
/// pinned by hash; `response_format` carries these bytes and the prefix quotes them.
pub(crate) const SCHEMA_TEXT: &str = include_str!("../../schemas/interpretation-v2.json");
/// The instructions, ending with a newline.
pub(crate) const INSTRUCTIONS: &str =
    include_str!("../../schemas/interpretation-v2-instructions.txt");
/// The schema's `title`, also the `json_schema.name` of `response_format`.
pub(crate) const SCHEMA_NAME: &str = "mimik_interpretation_v2";
/// Between the instructions and the schema in the fixed prefix.
const SCHEMA_HEADING: &str = "\nJSON schema of the reply:\n";

/// SHA-256 of [`FIXED_PREFIX`] in lower-case hex: which prompt a trial measured a model with
/// (recorded in the trial, compared when it is read back).
pub(crate) fn prefix_sha256() -> &'static str {
    use sha2::{Digest, Sha256};
    static HASH: LazyLock<String> =
        LazyLock::new(|| hex::encode(Sha256::digest(FIXED_PREFIX.as_bytes())));
    HASH.as_str()
}

/// The most options of a reply, and the most readings of an `ambiguous` reply.
pub(crate) const MAX_OPTIONS: usize = 3;
/// The most characters of an explanation or a reading.
pub(crate) const MAX_TEXT_CHARS: usize = 120;
/// Operator levels below the root: leaves at depth 3 at most, so at most 8 leaves.
pub(crate) const MAX_DEPTH: usize = 3;

/// The largest dynamic bound B of any path (the GPU path's 4 KiB, plan §1.4a).
pub(crate) const MAX_DYNAMIC_BYTES: usize = 4096;

/// The target's name in `ref` and in the envelope (ASSIST-56): the target is called `t`,
/// every other item by its ID.
pub(crate) const TARGET_ALIAS: &str = "t";

/// What the prefix quotes in place of the ID enum: the grammar (`response_format`, the
/// pinned schema with this request's IDs, [`schema_for`]) enforces the enum.
pub(crate) const QUOTED_REF: &str = r#"{"type":"string","description":"t for the target, or the id of a context item from the data"}"#;

/// The parsed schema, for `response_format` and for Rust's own validation.
pub(crate) static SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(SCHEMA_TEXT).expect("the embedded interpretation schema is JSON")
});

/// The fixed prefix: the system message of every interpretation request: the
/// instructions, the heading and the schema's compact text with the ID enum replaced by
/// [`QUOTED_REF`] (keys in the pinned order).
pub(crate) static FIXED_PREFIX: LazyLock<String> = LazyLock::new(|| {
    let mut quoted = SCHEMA.clone();
    quoted["$defs"]["ref"]["properties"]["ref"] =
        serde_json::from_str(QUOTED_REF).expect("the quoted reference is JSON");
    let quoted = serde_json::to_string(&quoted).expect("the schema serialises");
    let mut prefix = String::with_capacity(INSTRUCTIONS.len() + quoted.len() + 32);
    prefix.push_str(INSTRUCTIONS);
    prefix.push_str(SCHEMA_HEADING);
    prefix.push_str(&quoted);
    prefix
});

/// The reply schema of a request that supplies `ids` (context item IDs in envelope order):
/// the pinned schema with `ref` limited to `t` and these IDs, at the same key position.
pub(crate) fn schema_for(ids: &[String]) -> Value {
    let mut schema = SCHEMA.clone();
    let mut names = vec![Value::from(TARGET_ALIAS)];
    names.extend(ids.iter().map(|id| Value::from(id.as_str())));
    schema["$defs"]["ref"]["properties"]["ref"] = serde_json::json!({ "enum": names });
    schema
}

/// The schema replies are validated against: the pinned schema with `ref` any non-empty
/// string. Which IDs a request supplied is checked per option (`output::read`), so an unknown
/// ID drops that option and not the reply.
pub(crate) static VALIDATION_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    let mut schema = SCHEMA.clone();
    schema["$defs"]["ref"]["properties"]["ref"] =
        serde_json::json!({"type": "string", "minLength": 1});
    schema
});

/// Whether `id` can name a context item: a field or item-property ID (the one id grammar,
/// `definitions::valid_field_id`) that is neither the target's name nor the target's ID.
pub(crate) fn nameable(id: &str, target_id: &str) -> bool {
    crate::definitions::valid_field_id(id) && id != TARGET_ALIAS && id != target_id
}

/// The target as the envelope shows it. `label` is the first thing dropped for the bound.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct EnvelopeTarget {
    pub alias: &'static str,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// `None` is unknown (`null`).
    pub value: Option<i64>,
}

/// The kind of a context item: only integers can be referenced; enums are context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ItemKind {
    Integer,
    Enum,
}

/// One context item: a field or a same-row cell, named by its ID. Its value is not sent;
/// Mimik substitutes it when it evaluates a reply (`output::Supplied`).
#[derive(Clone, Debug, Serialize)]
pub(crate) struct EnvelopeItem {
    pub id: String,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: ItemKind,
}

/// The dynamic part's content, in this order: the note, the target, the context.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Envelope {
    pub note: String,
    pub target: EnvelopeTarget,
    pub context: Vec<EnvelopeItem>,
}

/// The dynamic part: `<data>`, the envelope as compact JSON with `<`, `>` and `&` escaped
/// as `<`, `>` and `&` (so nothing inside can close the envelope), then
/// `</data>`. Its byte length is what the bound B measures.
pub(crate) fn envelope(content: &Envelope) -> String {
    let json = serde_json::to_string(content).expect("an envelope serialises");
    let mut out = String::with_capacity(json.len() + 16);
    out.push_str("<data>\n");
    for ch in json.chars() {
        match ch {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            other => out.push(other),
        }
    }
    out.push_str("\n</data>");
    out
}

#[cfg(test)]
#[path = "contract_tests.rs"]
mod tests;
