//! What a reminder sends and how its reply is read (assisted sheet plan 3c-1, "Context",
//! "Reply" and "Read set and hash"): the request's data (the decrease, the other whole-number
//! and choice fields, the catalogue rows' names), rendered within B and N(B) with rows dropped
//! first and then fields, and the reply's reading with literal provenance and the read set the
//! proposal is bound to.
//!
//! The sheet side that builds a [`ReminderRequest`] from a snapshot is
//! `campaigns::remind`; nothing here knows the sheet.
use super::contract::{
    ItemKind, ReminderDecrease, ReminderEnvelope, ReminderField, ReminderRow, reminder_envelope,
    reminder_schema_for,
};
use super::interpreter::{Completion, Interpreter, RequestKind};
use super::lexer;
use super::output::{self, Checks, Invalid, Supplied};
use super::proposal::{Change, Edit, Read};
use super::request::{Bounds, NotRendered};
use crate::campaigns::NoteValue;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// The most catalogue rows a request names: the reply schema's row enum (`r1` to `r32`).
pub(crate) const MAX_ROWS: usize = 32;

/// One field the request may send: a whole-number or choice field of the sheet, with its typed
/// address and, for a choice, the option IDs the pin declares.
#[derive(Clone, Debug)]
pub(crate) struct FieldDraft {
    pub item: ReminderField,
    pub address: String,
    pub options: Vec<String>,
}

impl FieldDraft {
    fn integer(&self) -> bool {
        self.item.kind == ItemKind::Integer
    }

    fn number(&self) -> Option<i64> {
        self.item.value.as_ref().and_then(Value::as_i64)
    }

    fn note_value(&self) -> NoteValue {
        match (&self.item.value, self.integer()) {
            (Some(Value::Number(number)), true) => {
                number.as_i64().map_or(NoteValue::Unknown, NoteValue::Int)
            }
            (Some(Value::String(id)), false) => NoteValue::Enum(id.clone()),
            _ => NoteValue::Unknown,
        }
    }
}

/// One catalogue-linked row: its request ID (`r<n>`), list label, name as quoted data and the
/// typed address `row:<list>/<host>` its name is re-read by.
#[derive(Clone, Debug)]
pub(crate) struct RowDraft {
    pub row: ReminderRow,
    pub address: String,
}

/// Everything a reminder could carry, in the order it is sent: the decrease, the fields, the
/// rows. A rendered request keeps a prefix of the fields and of the rows.
#[derive(Clone, Debug)]
pub(crate) struct ReminderRequest {
    pub decrease: ReminderDecrease,
    /// The decreased field's typed address (the origin's target).
    pub address: String,
    pub fields: Vec<FieldDraft>,
    pub rows: Vec<RowDraft>,
}

/// A rendered request: what is sent, the values it sent (the proposal's read set) and the
/// request that was kept (what its reply is read against).
#[derive(Clone, Debug)]
pub(crate) struct Rendered {
    pub completion: Completion,
    pub reads: Vec<Read>,
    pub kept: ReminderRequest,
}

/// What a target, a change's `ref`, stands for.
struct Target<'a> {
    address: &'a str,
    label: &'a str,
    kind: ItemKind,
    value: NoteValue,
    number: Option<i64>,
    options: &'a [String],
    /// The decreased field: only an expression may change it.
    decrease: bool,
}

impl ReminderRequest {
    fn kept(&self, fields: usize, rows: usize) -> Self {
        Self {
            decrease: self.decrease.clone(),
            address: self.address.clone(),
            fields: self.fields[..fields].to_vec(),
            rows: self.rows[..rows].to_vec(),
        }
    }

    fn envelope(&self) -> ReminderEnvelope {
        ReminderEnvelope {
            decrease: self.decrease.clone(),
            fields: self.fields.iter().map(|field| field.item.clone()).collect(),
            rows: self.rows.iter().map(|row| row.row.clone()).collect(),
        }
    }

    fn read_set(&self) -> Vec<Read> {
        let encoded = |value: &NoteValue| value.encode().unwrap_or_default();
        let mut reads = vec![Read {
            address: self.address.clone(),
            value: encoded(&NoteValue::Int(self.decrease.after)),
        }];
        reads.extend(self.fields.iter().map(|field| Read {
            address: field.address.clone(),
            value: encoded(&field.note_value()),
        }));
        reads.extend(self.rows.iter().map(|row| Read {
            address: row.address.clone(),
            value: row.row.name.clone(),
        }));
        reads
    }

    /// The IDs a change may name: the decreased field and every sent field.
    fn targets(&self) -> Vec<String> {
        std::iter::once(self.decrease.id.clone())
            .chain(self.fields.iter().map(|field| field.item.id.clone()))
            .collect()
    }

    /// The IDs an expression may reference: the decreased field and the sent integers.
    fn references(&self) -> Vec<String> {
        std::iter::once(self.decrease.id.clone())
            .chain(
                self.fields
                    .iter()
                    .filter(|field| field.integer())
                    .map(|field| field.item.id.clone()),
            )
            .collect()
    }

    fn target(&self, id: &str) -> Option<Target<'_>> {
        if id == self.decrease.id {
            return Some(Target {
                address: &self.address,
                label: &self.decrease.label,
                kind: ItemKind::Integer,
                value: NoteValue::Int(self.decrease.after),
                number: Some(self.decrease.after),
                options: &[],
                decrease: true,
            });
        }
        let field = self.fields.iter().find(|field| field.item.id == id)?;
        Some(Target {
            address: &field.address,
            label: &field.item.label,
            kind: field.item.kind,
            value: field.note_value(),
            number: field.number(),
            options: &field.options,
            decrease: false,
        })
    }

    fn checks(&self) -> Checks {
        let mut supplied = BTreeMap::from([(
            self.decrease.id.clone(),
            Supplied {
                address: self.address.clone(),
                integer: true,
                value: Some(self.decrease.after),
            },
        )]);
        for field in &self.fields {
            supplied.insert(
                field.item.id.clone(),
                Supplied {
                    address: field.address.clone(),
                    integer: field.integer(),
                    value: field.number(),
                },
            );
        }
        Checks {
            phrase: self.decrease.phrase.clone(),
            target_value: Some(self.decrease.after),
            supplied,
        }
    }

    /// The literal magnitudes provenance admits for a change of `target` (plan 3c-1, "Reply"):
    /// the 2c rule with the decrease's phrase as the phrase (its numbers and number words, with
    /// a fraction or percent word 2, 3, 4 and 100), plus the decrease's `before`, `after` and
    /// their difference, and the target's own value.
    fn allowed(&self, target: &Target<'_>) -> BTreeSet<u64> {
        let phrase = &self.decrease.phrase;
        let mut allowed: BTreeSet<u64> = lexer::phrase_numbers(phrase).into_iter().collect();
        allowed.extend(lexer::number_words(phrase));
        allowed.insert(self.decrease.before.unsigned_abs());
        allowed.insert(self.decrease.after.unsigned_abs());
        allowed.insert(self.decrease.before.abs_diff(self.decrease.after));
        if let Some(value) = target.number {
            allowed.insert(value.unsigned_abs());
        }
        if lexer::has_fraction_word(phrase) {
            allowed.extend(lexer::FRACTION_LITERALS);
        }
        allowed
    }
}

/// Renders `request` within `bounds`, counting tokens with `interpreter` only for shapes that
/// already fit the byte bound: rows are dropped first (the last first), then fields (the last
/// first); the decrease never. `reply_tokens` is the reminder reply bound.
///
/// # Errors
///
/// [`NotRendered::TooLong`] when the decrease alone exceeds a bound, [`NotRendered::Tokenizer`]
/// when `/tokenize` fails.
pub(crate) async fn render(
    request: &ReminderRequest,
    bounds: Bounds,
    reply_tokens: u32,
    interpreter: &dyn Interpreter,
) -> Result<Rendered, NotRendered> {
    let (fields, rows) = (request.fields.len(), request.rows.len());
    let shapes = (0..=rows)
        .rev()
        .map(|kept| (fields, kept))
        .chain((0..fields).rev().map(|kept| (kept, 0)));
    for (kept_fields, kept_rows) in shapes {
        let kept = request.kept(kept_fields, kept_rows);
        let dynamic = reminder_envelope(&kept.envelope());
        if dynamic.len() > bounds.bytes {
            continue;
        }
        let tokens = interpreter
            .tokens(&dynamic)
            .await
            .map_err(NotRendered::Tokenizer)?;
        if tokens <= bounds.tokens {
            return Ok(Rendered {
                completion: Completion {
                    dynamic,
                    ref_ids: kept.references(),
                    max_tokens: reply_tokens,
                    kind: RequestKind::Reminder {
                        rows: kept.rows.iter().map(|row| row.row.id.clone()).collect(),
                        targets: kept.targets(),
                    },
                },
                reads: kept.read_set(),
                kept,
            });
        }
    }
    Err(NotRendered::TooLong)
}

/// What a valid reply says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Reading {
    /// `relevant: false`: nothing is shown.
    Nothing,
    Hint {
        hint: String,
        /// The cited row's name.
        row: Option<String>,
        /// The option's changes and explanation, when it passed every check; an option that
        /// fails one is dropped alone and the hint stays.
        option: Option<(Vec<Change>, String)>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    relevant: bool,
    row: Option<String>,
    hint: String,
    option: Option<RawOption>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOption {
    changes: Vec<RawChange>,
    explanation: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawChange {
    Expr(RawExpr),
    Enum(RawEnum),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExpr {
    #[serde(rename = "ref")]
    id: String,
    expr: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEnum {
    #[serde(rename = "ref")]
    id: String,
    #[serde(rename = "enum")]
    option: String,
}

/// Reads a reminder reply's text against the request that was kept: the schema with this
/// request's IDs, then the typed reply. A truncated or invalid reply is an error (the job shows
/// nothing and the breaker is not touched).
///
/// # Errors
///
/// [`Invalid`].
pub(crate) fn read(text: &str, request: &ReminderRequest) -> Result<Reading, Invalid> {
    let value: Value = serde_json::from_str(text).map_err(|_| Invalid::Json)?;
    let rows: Vec<String> = request.rows.iter().map(|row| row.row.id.clone()).collect();
    let schema = reminder_schema_for(&rows, &request.targets(), &request.references());
    let validator = jsonschema::draft202012::options()
        .offline()
        .build(&schema)
        .map_err(|_| Invalid::Schema)?;
    if !validator.is_valid(&value) {
        return Err(Invalid::Schema);
    }
    let reply: Reply = serde_json::from_value(value).map_err(|_| Invalid::Decode)?;
    if !reply.relevant {
        return Ok(Reading::Nothing);
    }
    let row = match reply.row {
        None => None,
        Some(id) => Some(
            request
                .rows
                .iter()
                .find(|row| row.row.id == id)
                .map(|row| row.row.name.clone())
                .ok_or(Invalid::Decode)?,
        ),
    };
    let option = reply.option.and_then(|option| change_set(request, option));
    Ok(Reading::Hint {
        hint: reply.hint,
        row,
        option,
    })
}

/// The option's changes, or `None` when any change fails a check (references, provenance,
/// checked evaluation, an enum option the pin declares, a kind that suits the target).
fn change_set(request: &ReminderRequest, option: RawOption) -> Option<(Vec<Change>, String)> {
    let checks = request.checks();
    let mut changes = Vec::new();
    for raw in option.changes {
        changes.push(match raw {
            RawChange::Expr(raw) => {
                let target = request.target(&raw.id)?;
                if target.kind != ItemKind::Integer {
                    return None;
                }
                let allowed = request.allowed(&target);
                let candidate = output::candidate(
                    serde_json::json!({"expr": raw.expr, "explanation": option.explanation}),
                    &checks,
                    &allowed,
                )
                .ok()?;
                let name = |id: &str| {
                    let label = request.target(id).map_or(id, |target| target.label);
                    let value = checks.supplied.get(id).and_then(|supplied| supplied.value);
                    value.map_or_else(|| label.to_owned(), |value| format!("{label} ({value})"))
                };
                let shown = output::render(&candidate.expr, &name);
                Change {
                    address: target.address.to_owned(),
                    before: target.value,
                    edit: Edit::Expr { candidate, shown },
                }
            }
            RawChange::Enum(raw) => {
                let target = request.target(&raw.id)?;
                if target.decrease
                    || target.kind != ItemKind::Enum
                    || !target.options.contains(&raw.option)
                {
                    return None;
                }
                Change {
                    address: target.address.to_owned(),
                    before: target.value,
                    edit: Edit::Enum(raw.option),
                }
            }
        });
    }
    Some((changes, option.explanation))
}

#[cfg(test)]
#[path = "reminder_tests.rs"]
mod tests;
