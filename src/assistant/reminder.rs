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
use super::proposal::{Change, Edit, Read, Source};
use super::request::{Bounds, NotRendered};
use crate::campaigns::NoteValue;
use crate::catalogues::read::cut_text;
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

/// The catalogue entry a row is linked to (plan 3c-2): the sealed version the link named when
/// the reminder started and the key, with the entry's name and badge label once its text is
/// attached ([`RowDraft::attach`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EntryBinding {
    pub catalogue: String,
    pub version: i64,
    pub key: String,
    /// The entry's name, shown with its sources.
    pub name: String,
    /// The badge's label as the entry page shows it.
    pub badge: String,
    /// Whether the text sent is shorter than the entry's whole context (an excerpt that reaches
    /// its end then shows "…").
    pub cut: bool,
}

/// One catalogue-linked row: its request ID (`r<n>`), list label, name as quoted data and the
/// typed address `row:<list>/<host>` its name is re-read by. `entry` is the row's catalogue
/// link; `row.entry` holds that entry's text when it is sent (always with a binding).
#[derive(Clone, Debug)]
pub(crate) struct RowDraft {
    pub row: ReminderRow,
    pub address: String,
    pub entry: Option<EntryBinding>,
}

impl RowDraft {
    /// Attaches what the entry read gave: the text (sent as the row's `entry`) with the entry's
    /// name and badge, or nothing, which drops the binding (the row stays, with its name only).
    pub(crate) fn attach(&mut self, context: Option<(String, String, String)>) {
        self.attach_cut(context, false);
    }

    /// [`attach`](Self::attach) with whether the text is cut ([`EntryContext::cut`]).
    ///
    /// [`EntryContext::cut`]: crate::catalogues::read::EntryContext::cut
    pub(crate) fn attach_cut(&mut self, context: Option<(String, String, String)>, cut: bool) {
        match (context, self.entry.as_mut()) {
            (Some((name, badge, text)), Some(binding)) if !text.is_empty() => {
                binding.name = name;
                binding.badge = badge;
                binding.cut = cut;
                self.row.entry = Some(text);
            }
            _ => {
                self.entry = None;
                self.row.entry = None;
            }
        }
    }

    /// The text this row sends, with its binding.
    fn context(&self) -> Option<(&EntryBinding, &str)> {
        self.entry.as_ref().zip(self.row.entry.as_deref())
    }

    /// The entry read's address: `entry:<list>/<host>`.
    fn entry_address(&self) -> String {
        format!(
            "entry:{}",
            self.address.strip_prefix("row:").unwrap_or(&self.address)
        )
    }
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

    /// The request with only the first `k` contexts (in row order); the others lose their text
    /// and binding. With `cut` the `k`-th keeps its first `cut` characters, and with `whole_words`
    /// too ends where [`cut_text`] puts the cut (never inside a number).
    fn with_contexts(&self, k: usize, cut: Option<usize>, whole_words: bool) -> Self {
        let mut seen = 0;
        let mut rows = self.rows.clone();
        for row in &mut rows {
            if row.context().is_none() {
                continue;
            }
            seen += 1;
            if seen > k {
                row.entry = None;
                row.row.entry = None;
            } else if let (true, Some(cut), Some(text), Some(binding)) =
                (seen == k, cut, row.row.entry.as_mut(), row.entry.as_mut())
            {
                if whole_words {
                    *text = cut_text(text, cut).0;
                } else {
                    *text = text.chars().take(cut).collect();
                }
                binding.cut = true;
            }
        }
        Self {
            rows,
            ..self.clone()
        }
    }

    fn contexts(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| row.context().is_some())
            .count()
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
        // What each sent entry text is bound to: catalogue, sealed version, key, badge, the
        // text's fingerprint, its length in characters and whether it was cut (the apply reads
        // the entry again and cuts it alike).
        reads.extend(self.rows.iter().filter_map(|row| {
            let (binding, text) = row.context()?;
            Some(Read {
                address: row.entry_address(),
                value: serde_json::json!([
                    binding.catalogue,
                    binding.version,
                    binding.key,
                    binding.badge,
                    crate::catalogues::read::context_fingerprint(text),
                    text.chars().count(),
                    binding.cut,
                ])
                .to_string(),
            })
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
    /// their difference, and the target's own value; with `context` (the cited row's entry text
    /// as sent), also the numbers it spells in digits (plan 3c-2, step 6).
    fn allowed(&self, target: &Target<'_>, context: Option<&str>) -> BTreeSet<u64> {
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
        if let Some(context) = context {
            allowed.extend(lexer::phrase_numbers(context));
        }
        allowed
    }
}

/// Renders `request` within `bounds`, counting tokens with `interpreter` only for shapes that
/// already fit the byte bound. The shapes in order (plan 3c-2, step 4): every field and row
/// with the first k entry texts, k from all to 1, the k-th, before it is dropped whole, cut at
/// a character boundary to the bytes left; then, as in 3c-1, rows dropped (the last first) and
/// then fields (the last first), with no text; the decrease never. `reply_tokens` is the
/// reminder reply bound.
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
    let fit = |kept: ReminderRequest| async move {
        let dynamic = reminder_envelope(&kept.envelope());
        if dynamic.len() > bounds.bytes {
            return Ok(None);
        }
        let tokens = interpreter
            .tokens(&dynamic)
            .await
            .map_err(NotRendered::Tokenizer)?;
        Ok(if tokens <= bounds.tokens {
            Some(Rendered {
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
            })
        } else {
            None
        })
    };
    for k in (1..=request.contexts()).rev() {
        let whole = request.with_contexts(k, None, false);
        if reminder_envelope(&whole.envelope()).len() <= bounds.bytes {
            // It fits the bytes: only the tokens failed. As planned, a text is cut only to
            // the bytes left, never to fit the tokens, so the k-th is dropped whole.
            if let Some(rendered) = fit(whole).await? {
                return Ok(rendered);
            }
            continue;
        }
        if let Some(cut) = cut_to_fit(request, k, bounds.bytes) {
            let kept = request.with_contexts(k, Some(cut), true);
            let sends = kept
                .rows
                .iter()
                .filter_map(|row| row.row.entry.as_deref())
                .nth(k - 1)
                .is_some_and(|text| !text.is_empty());
            if sends && let Some(rendered) = fit(kept).await? {
                return Ok(rendered);
            }
        }
    }
    let bare = request.with_contexts(0, None, false);
    let (fields, rows) = (bare.fields.len(), bare.rows.len());
    let shapes = (0..=rows)
        .rev()
        .map(|kept| (fields, kept))
        .chain((0..fields).rev().map(|kept| (kept, 0)));
    for (kept_fields, kept_rows) in shapes {
        if let Some(rendered) = fit(bare.kept(kept_fields, kept_rows)).await? {
            return Ok(rendered);
        }
    }
    Err(NotRendered::TooLong)
}

/// The most characters the `k`-th text may keep for the request to fit `bytes`, at least one;
/// `None` when not even one character fits. The escaped length never shrinks as text is added,
/// so the largest fitting length is found by bisection; the caller then ends it at a word
/// ([`cut_text`]), which only shortens it.
fn cut_to_fit(request: &ReminderRequest, k: usize, bytes: usize) -> Option<usize> {
    let longest = request
        .with_contexts(k, None, false)
        .rows
        .iter()
        .filter_map(|row| row.row.entry.as_deref())
        .nth(k - 1)?
        .chars()
        .count();
    let fits = |cut: usize| {
        reminder_envelope(&request.with_contexts(k, Some(cut), false).envelope()).len() <= bytes
    };
    if !fits(1) {
        return None;
    }
    let (mut low, mut high) = (1, longest);
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        if fits(middle) {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    Some(low)
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
        /// The option's changes, explanation and sources, when it passed every check; an
        /// option that fails one is dropped alone and the hint stays.
        option: Option<Advice>,
    },
}

/// A reminder's option that passed every check: its changes, its explanation and where the
/// literals that come from entry text come from (plan 3c-2, step 7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Advice {
    pub changes: Vec<Change>,
    pub explanation: String,
    pub sources: Vec<Source>,
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
    let cited = match &reply.row {
        None => None,
        Some(id) => Some(
            request
                .rows
                .iter()
                .find(|row| &row.row.id == id)
                .ok_or(Invalid::Decode)?,
        ),
    };
    let row = cited.map(|row| row.row.name.clone());
    let option = reply
        .option
        .and_then(|option| change_set(request, option, cited));
    Ok(Reading::Hint {
        hint: reply.hint,
        row,
        option,
    })
}

/// The option's changes, or `None` when any change fails a check (references, provenance,
/// checked evaluation, an enum option the pin declares, a kind that suits the target). The
/// numbers of the `cited` row's entry text, as sent, count as provenance; a literal that only
/// they admit is given its source.
fn change_set(
    request: &ReminderRequest,
    option: RawOption,
    cited: Option<&RowDraft>,
) -> Option<Advice> {
    let checks = request.checks();
    let context = cited.and_then(|row| row.context());
    let mut changes = Vec::new();
    let mut entry_literals: Vec<u64> = Vec::new();
    for raw in option.changes {
        changes.push(match raw {
            RawChange::Expr(raw) => {
                let target = request.target(&raw.id)?;
                if target.kind != ItemKind::Integer {
                    return None;
                }
                let allowed = request.allowed(&target, context.map(|(_, text)| text));
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
                if context.is_some() {
                    let own = request.allowed(&target, None);
                    for literal in output::literals(&candidate.expr) {
                        let magnitude = literal.unsigned_abs();
                        if !own.contains(&magnitude) && !entry_literals.contains(&magnitude) {
                            entry_literals.push(magnitude);
                        }
                    }
                }
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
    let sources = context
        .map(|(binding, text)| sources(binding, text, &entry_literals))
        .unwrap_or_default();
    Some(Advice {
        changes,
        explanation: option.explanation,
        sources,
    })
}

/// The most characters of an excerpt, "…" included.
const EXCERPT_CHARACTERS: usize = 64;

/// One source per literal of `literals`: the first place `text` spells it in digits.
fn sources(binding: &EntryBinding, text: &str, literals: &[u64]) -> Vec<Source> {
    let spans = lexer::phrase_number_spans(text);
    literals
        .iter()
        .filter_map(|literal| {
            let (_, start, end) = spans.iter().find(|(value, _, _)| value == literal)?;
            Some(Source {
                entry: binding.name.clone(),
                badge: binding.badge.clone(),
                excerpt: excerpt(text, *start, *end, binding.cut),
            })
        })
        .collect()
}

/// The number at characters `start..end` of `text` with the words around it, at most 64
/// characters including a "…" on each cut side (`cut`: the text itself goes on past its end):
/// as many characters as fit, cut at word boundaries where one lies between the cut and the
/// number and at a character otherwise; any whitespace as a space.
fn excerpt(text: &str, start: usize, end: usize, cut: bool) -> String {
    let chars: Vec<char> = text
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();
    let length = chars.len();
    let more = |to: usize| to < length || cut;
    let cost = |from: usize, to: usize| (to - from) + usize::from(from > 0) + usize::from(more(to));
    let (mut from, mut to) = (start, end);
    loop {
        let mut grew = false;
        if from > 0 && cost(from - 1, to) <= EXCERPT_CHARACTERS {
            from -= 1;
            grew = true;
        }
        if to < length && cost(from, to + 1) <= EXCERPT_CHARACTERS {
            to += 1;
            grew = true;
        }
        if !grew {
            break;
        }
    }
    let word_start = |at: usize| at == 0 || chars[at - 1] == ' ';
    let word_end = |at: usize| at == length || chars[at] == ' ';
    if !word_start(from)
        && let Some(at) = (from..=start).find(|at| word_start(*at))
    {
        from = at;
    }
    if !word_end(to)
        && let Some(at) = (end..=to).rev().find(|at| word_end(*at))
    {
        to = at;
    }
    let body: String = chars[from..to].iter().collect();
    format!(
        "{}{}{}",
        if from > 0 { "\u{2026}" } else { "" },
        body.trim(),
        if more(to) { "\u{2026}" } else { "" }
    )
}

#[cfg(test)]
#[path = "reminder_tests.rs"]
mod tests;
