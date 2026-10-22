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

/// The most options of a reply, and the most readings of an `ambiguous` reply.
pub(crate) const MAX_OPTIONS: usize = 3;
/// The most characters of an explanation or a reading.
pub(crate) const MAX_TEXT_CHARS: usize = 120;
/// Operator levels below the root: leaves at depth 3 at most, so at most 8 leaves.
pub(crate) const MAX_DEPTH: usize = 3;

/// The largest dynamic bound B of any path (the GPU path's 4 KiB, plan §1.4a).
pub(crate) const MAX_DYNAMIC_BYTES: usize = 4096;

/// The target's alias.
pub(crate) const TARGET_ALIAS: &str = "t";
/// The most field aliases (`f1`…`f64`) the schema's alias enum lists. No request can supply
/// more: at [`MAX_DYNAMIC_BYTES`] at most 63 of the smallest possible context items fit
/// (tested at the exact boundary), so the enum never makes the builder drop an item. The
/// schema is a superset of every request's aliases: Rust drops a reference to an alias the
/// request did not supply.
pub(crate) const MAX_FIELD_ALIASES: usize = 64;
/// The most same-row cell aliases (`c1`…`c31`): a list item has at most 32 properties, one
/// of which is the target.
pub(crate) const MAX_CELL_ALIASES: usize = 31;

/// The parsed schema, for `response_format` and for Rust's own validation.
pub(crate) static SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(SCHEMA_TEXT).expect("the embedded interpretation schema is JSON")
});

/// The fixed prefix: the system message of every interpretation request.
pub(crate) static FIXED_PREFIX: LazyLock<String> = LazyLock::new(|| {
    let mut prefix = String::with_capacity(INSTRUCTIONS.len() + SCHEMA_TEXT.len() + 32);
    prefix.push_str(INSTRUCTIONS);
    prefix.push_str(SCHEMA_HEADING);
    prefix.push_str(SCHEMA_TEXT.trim_end_matches('\n'));
    prefix
});

/// The alias of the `n`-th supplied field (1-based), or `None` past the enum.
pub(crate) fn field_alias(n: usize) -> Option<String> {
    (1..=MAX_FIELD_ALIASES)
        .contains(&n)
        .then(|| format!("f{n}"))
}

/// The alias of the `n`-th supplied same-row cell (1-based), or `None` past the enum.
pub(crate) fn cell_alias(n: usize) -> Option<String> {
    (1..=MAX_CELL_ALIASES).contains(&n).then(|| format!("c{n}"))
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

/// One context item: a field (`f…`) or a same-row cell (`c…`).
#[derive(Clone, Debug, Serialize)]
pub(crate) struct EnvelopeItem {
    pub alias: String,
    pub id: String,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: ItemKind,
    /// An integer, an enum option ID, or `null` for unknown.
    pub value: Value,
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
