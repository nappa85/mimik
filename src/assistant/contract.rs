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
    // The builder guarantees this (`campaigns::interpret::unambiguous`); a duplicate would make
    // the grammar's enum ambiguous and an empty ID or `t` could never be a context item.
    debug_assert!(
        ids.iter()
            .enumerate()
            .all(|(n, id)| !id.is_empty() && id != TARGET_ALIAS && !ids[..n].contains(id)),
        "reply schema ids must be unique, non-empty and not `t`: {ids:?}"
    );
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

/// The reminder reply schema (plan 3c-1): compact JSON, ASCII, one trailing newline, keys in
/// reply order, pinned by hash like [`SCHEMA_TEXT`].
pub(crate) const REMINDER_SCHEMA_TEXT: &str = include_str!("../../schemas/reminder-v1.json");
/// The reminder instructions, ending with a newline.
pub(crate) const REMINDER_INSTRUCTIONS: &str =
    include_str!("../../schemas/reminder-v1-instructions.txt");
/// The reminder schema's `title`, also its `json_schema.name`.
pub(crate) const REMINDER_SCHEMA_NAME: &str = "mimik_reminder_v1";
/// What the reminder prefix quotes in place of the row ID enum.
pub(crate) const QUOTED_ROW: &str =
    r#"{"type":"string","description":"the id of a list item from the data, or null"}"#;
/// What it quotes in place of the change-target and expression-reference enums.
pub(crate) const QUOTED_FIELD: &str =
    r#"{"type":"string","description":"the id of a field from the data"}"#;

/// The parsed reminder schema.
pub(crate) static REMINDER_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(REMINDER_SCHEMA_TEXT).expect("the embedded reminder schema is JSON")
});

/// The reminder system message: the instructions, the heading and the schema's compact text
/// with the ID enums replaced by their quoted descriptions. Not primed: it has no shared start
/// with the interpretation prefix, so a reminder replaces the slot's cache (plan 3c-0).
pub(crate) static REMINDER_PREFIX: LazyLock<String> = LazyLock::new(|| {
    let mut quoted = REMINDER_SCHEMA.clone();
    quoted["properties"]["row"]["anyOf"][0] =
        serde_json::from_str(QUOTED_ROW).expect("the quoted row is JSON");
    let field: Value = serde_json::from_str(QUOTED_FIELD).expect("the quoted field is JSON");
    quoted["$defs"]["target"] = field.clone();
    quoted["$defs"]["ref"]["properties"]["ref"] = field;
    let quoted = serde_json::to_string(&quoted).expect("the schema serialises");
    let mut prefix = String::with_capacity(REMINDER_INSTRUCTIONS.len() + quoted.len() + 32);
    prefix.push_str(REMINDER_INSTRUCTIONS);
    prefix.push_str(SCHEMA_HEADING);
    prefix.push_str(&quoted);
    prefix
});

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "only the tests and the eval tooling compare the pinned prefix hash"
    )
)]
/// SHA-256 of [`REMINDER_PREFIX`] in lower-case hex.
pub(crate) fn reminder_prefix_sha256() -> &'static str {
    use sha2::{Digest, Sha256};
    static HASH: LazyLock<String> =
        LazyLock::new(|| hex::encode(Sha256::digest(REMINDER_PREFIX.as_bytes())));
    HASH.as_str()
}

/// The reply schema of a reminder request: `row` limited to the request's row IDs (or null),
/// change targets to `targets` (the decreased field and the sent fields) and expression
/// references to `refs` (the sent whole-number fields and the decreased one), at the same key
/// positions.
pub(crate) fn reminder_schema_for(rows: &[String], targets: &[String], refs: &[String]) -> Value {
    let names = |ids: &[String]| {
        ids.iter()
            .map(|id| Value::from(id.as_str()))
            .collect::<Vec<_>>()
    };
    let mut schema = REMINDER_SCHEMA.clone();
    if rows.is_empty() {
        // An empty enum is no grammar: with no rows the only row is null.
        schema["properties"]["row"] = serde_json::json!({"type": "null"});
    } else {
        schema["properties"]["row"]["anyOf"][0] = serde_json::json!({ "enum": names(rows) });
    }
    schema["$defs"]["target"] = serde_json::json!({ "enum": names(targets) });
    schema["$defs"]["ref"]["properties"]["ref"] = serde_json::json!({ "enum": names(refs) });
    schema
}

/// A deterministic 64-character identifier that tokenises like a realistic one (mixed
/// letters, digits and separators, not a single repeated character), distinct per `n`.
fn long_id(n: usize) -> String {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64 ^ (n as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let alphabet = b"abcdefghijklmnopqrstuvwxyz0123456789_";
    let mut id = String::from("f");
    while id.len() < 64 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        id.push(char::from(
            alphabet[usize::try_from(state % 37).unwrap_or(0)],
        ));
    }
    id
}

/// The IDs [`largest_reminder_reply`] uses: the decreased field and three others.
pub(crate) fn largest_reply_ids() -> Vec<String> {
    (0..4).map(long_id).collect()
}

/// The largest valid reminder reply with realistic 64-character IDs (as ASSIST-59 did for
/// interpretations): relevant, a row, a 120-character hint, and four changes whose
/// expressions have two operator levels with four leaves, each a division with its rounding,
/// and a 120-character explanation. Its token count, from the server's `/tokenize`, is the
/// reminder's reply bound. Compact single-line JSON like a model's reply.
pub(crate) fn largest_reminder_reply() -> String {
    let ids = largest_reply_ids();
    let leaf = |n: usize| serde_json::json!({"ref": ids[n % ids.len()]});
    let div =
        |l: Value, r: Value| serde_json::json!({"op": "div", "round": "exact", "l": l, "r": r});
    let expr = || div(div(leaf(0), leaf(1)), div(leaf(2), leaf(3)));
    // Realistic mixed text (words, digits, punctuation, accents), which tokenizes to many more
    // tokens per character than one repeated letter does, cut to the longest allowed.
    let text = |sample: &str| {
        sample
            .chars()
            .cycle()
            .take(MAX_TEXT_CHARS)
            .collect::<String>()
    };
    let changes: Vec<Value> = ids
        .iter()
        .map(|id| serde_json::json!({"ref": id, "expr": expr()}))
        .collect();
    serde_json::to_string(&serde_json::json!({
        "relevant": true,
        "row": "r32",
        "hint": text("Ricorda: 3 pozioni, 12 monete e l'antidoto (r5)! Però non è l'ultimo; 7/9 ok. "),
        "option": {"changes": changes, "explanation": text("Ripristina 14 PF perché l'ora è già passata: 2 × 7 = 14, non 15 (città) - ok? ")},
    }))
    .expect("a reply serialises")
}

/// The reminder's context fields (other whole-number and choice fields; never booleans, text,
/// notes, keys or other properties) as the envelope shows them.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ReminderField {
    pub id: String,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: ItemKind,
    /// The integer, or the enum option ID; `null` when unknown.
    pub value: Option<serde_json::Value>,
}

/// The decreased field.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ReminderDecrease {
    pub id: String,
    pub label: String,
    pub before: i64,
    pub after: i64,
    pub phrase: String,
}

/// One catalogue-linked row: its request ID and name, as quoted data.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ReminderRow {
    pub id: String,
    pub list: String,
    pub name: String,
}

/// The reminder's dynamic part: the decrease, the fields, the rows, in this order.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ReminderEnvelope {
    pub decrease: ReminderDecrease,
    pub fields: Vec<ReminderField>,
    pub rows: Vec<ReminderRow>,
}

/// The reminder's dynamic part, escaped like [`envelope`].
pub(crate) fn reminder_envelope(content: &ReminderEnvelope) -> String {
    escape_data(&serde_json::to_string(content).expect("an envelope serialises"))
}

/// The dynamic part: `<data>`, the envelope as compact JSON with `<`, `>` and `&` escaped
/// as `<`, `>` and `&` (so nothing inside can close the envelope), then
/// `</data>`. Its byte length is what the bound B measures.
pub(crate) fn envelope(content: &Envelope) -> String {
    escape_data(&serde_json::to_string(content).expect("an envelope serialises"))
}

fn escape_data(json: &str) -> String {
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
