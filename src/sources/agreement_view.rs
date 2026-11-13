//! The agreement on the review screen (`docs/source-agreement-plan.md`, slice 3b): reads the
//! page's units, suggestions and stored words, runs the rule of [`super::agreement`] off the
//! async runtime and returns what the screen shows. An agreement is never a decision and never
//! evidence (SRC-6); it only orders the Owner's work.
//!
//! The pass without the caller's exclusions is memoised beside the table proposals, keyed by
//! page revision, stored-words size, rule version and exclusion revision. The caller's
//! exclusions (open suggestions, table proposals, table shape, text budget) only mark
//! candidates and are applied on every view, so dismissing a suggestion shows at once.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use super::SuggestionTarget;
use super::agreement::{
    Agreement, Diff, DiffReason, Exclusion, MAX_UNITS, RULE_VERSION, ReadingKind, UnitInput,
    UnitVerdict, Verdict, classify_provenance, evaluate,
};
use super::materials::{ORDERED_UNITS, text};
use super::page_words::{self, PageWord};
use super::shapes::{self, Shape, ShapeInput};
use crate::{AppError, Store};

/// Slice 4's batch form is not shipped yet: until it is, no page offers it, so the default tab
/// stays `all` and `matching` shows full cards. Flipping this is slice 4's change.
pub(crate) const BATCH_FORM_SHIPPED: bool = false;

#[cfg(test)]
thread_local! {
    /// Lets a test act as if slice 4's form shipped (route tests run on their own thread).
    pub(crate) static FORCE_BATCH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the batch form is shipped (always, in a test that forces it).
pub(crate) fn batch_shipped() -> bool {
    #[cfg(test)]
    if FORCE_BATCH.get() {
        return true;
    }
    BATCH_FORM_SHIPPED
}
/// A non-plain shape scoring this much marks a unit as "looks like a table".
const SHAPE_SCORE: u32 = 80;
/// The same text budget as the table proposals.
const TEXT_BUDGET: usize = super::tables::MAX_HTML_BYTES;
/// Orphan words outlined on the page image.
pub(crate) const MAX_ORPHANS_DRAWN: usize = 2_000;
/// Characters kept of one side of one displayed change.
const CHANGE_CHARS: usize = 160;
/// Characters of changes displayed on one card.
const CARD_CHARS: usize = 600;
/// Characters of changes displayed on one card window (worst case six escaped bytes each).
const SCREEN_CHARS: usize = 4 * 1024;
/// Changes displayed on one card window.
const SCREEN_CHANGES: usize = 200;
/// Characters of a unit's first line in the matching list.
const FIRST_LINE_CHARS: usize = 80;

/// Page id, page revision, stored-words size, rule version and the source's exclusion revision.
/// Imported sources have no exclusion boxes (those belong to staged uploads), so the last part
/// is 0; it is part of the key so a source that gets boxes cannot reuse a pass without them.
pub(crate) type PassKey = (i64, i64, i64, &'static str, i64);

// ---------------------------------------------------------------------------------------
// Tabs
// ---------------------------------------------------------------------------------------

/// The `show` parameter as sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Show {
    /// Not sent.
    #[default]
    Absent,
    /// Sent but not one of the four words: treated as the default.
    Unknown,
    Attention,
    Matching,
    Reviewed,
    All,
}

impl<'de> Deserialize<'de> for Show {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let word = String::deserialize(deserializer)?;
        Ok(match word.as_str() {
            "attention" => Self::Attention,
            "matching" => Self::Matching,
            "reviewed" => Self::Reviewed,
            "all" => Self::All,
            _ => Self::Unknown,
        })
    }
}

impl Show {
    /// The tab a request names, without an agreement to consult (absent or unknown: `all`).
    pub(crate) fn tab(self) -> Tab {
        match self {
            Self::Attention => Tab::Attention,
            Self::Matching => Tab::Matching,
            Self::Reviewed => Tab::Reviewed,
            Self::All | Self::Absent | Self::Unknown => Tab::All,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tab {
    Attention,
    Matching,
    Reviewed,
    All,
}

impl Tab {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Attention => "attention",
            Self::Matching => "matching",
            Self::Reviewed => "reviewed",
            Self::All => "all",
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Attention => "Needs attention",
            Self::Matching => "Matching",
            Self::Reviewed => "Reviewed",
            Self::All => "All",
        }
    }
}

pub(crate) const NO_RESULT_NOTICE: &str =
    "No agreement result for this page, so this view needs one: showing all units.";
pub(crate) const HIDDEN_NOTICE: &str =
    "The unit you asked for is not in the chosen view: showing all units around it.";

/// Whether a page offers slice 4's batch form: only an independent reading with a result.
#[must_use]
pub(crate) fn batch_offered(kind: Option<ReadingKind>) -> bool {
    batch_shipped() && batch_rule(kind)
}

/// D1 (SRC-21): batch accept only for independent readings.
#[must_use]
pub(crate) fn batch_rule(kind: Option<ReadingKind>) -> bool {
    kind == Some(ReadingKind::Independent)
}

// ---------------------------------------------------------------------------------------
// The view
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Computed(ReadingKind),
    /// No usable stored words (no row, an invalid row, or none inside the page).
    NoWords,
    /// More than 2,000 units on the page.
    OverBound,
    /// A read deadline fired (not remembered).
    TimedOut,
    /// A storage or capacity error (logged by the caller).
    Unreadable,
    /// The units were built from the stored words: a comparison proves nothing.
    Derived,
    /// The stored words are over the size bound.
    WordsTooLarge,
}

/// Whether a stored words row of `len` bytes (`-1`: none) is over the size bound.
pub(crate) fn words_too_large(len: i64) -> bool {
    usize::try_from(len).is_ok_and(|len| len > page_words::MAX_ROW_BYTES)
}

#[derive(Clone, Debug)]
pub(crate) struct UnitAgreement {
    pub id: i64,
    pub position: i64,
    pub state: String,
    pub kind: String,
    /// The start of the extracted text (the matching list).
    pub first_line: String,
    pub verdict: Verdict,
}

impl UnitAgreement {
    fn unreviewed(&self) -> bool {
        self.state == "unreviewed"
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub matching: usize,
    pub differ: usize,
    pub no_words: usize,
    pub excluded: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct AgreementView {
    pub status: Status,
    /// Every unit of the page in reading order (empty unless computed).
    pub units: Vec<UnitAgreement>,
    /// Boxes of the text-layer words inside no unit.
    pub orphans: Vec<[f32; 4]>,
    index: HashMap<i64, usize>,
}

impl AgreementView {
    pub(crate) fn unavailable(status: Status) -> Self {
        Self {
            status,
            units: Vec::new(),
            orphans: Vec::new(),
            index: HashMap::new(),
        }
    }

    fn computed(units: Vec<UnitAgreement>, orphans: Vec<[f32; 4]>, kind: ReadingKind) -> Self {
        let index = units.iter().enumerate().map(|(i, u)| (u.id, i)).collect();
        Self {
            status: Status::Computed(kind),
            units,
            orphans,
            index,
        }
    }

    pub(crate) fn kind(&self) -> Option<ReadingKind> {
        match self.status {
            Status::Computed(kind) => Some(kind),
            _ => None,
        }
    }

    pub(crate) fn has_result(&self) -> bool {
        self.kind().is_some()
    }

    pub(crate) fn offers_batch(&self) -> bool {
        batch_offered(self.kind())
    }

    pub(crate) fn unit(&self, id: i64) -> Option<&UnitAgreement> {
        self.index.get(&id).map(|&i| &self.units[i])
    }

    /// Whether the unit is listed by the tab. Attention and matching need a result; reviewed
    /// and all are answered from the units when they are known.
    pub(crate) fn in_tab(unit: &UnitAgreement, tab: Tab) -> bool {
        match tab {
            Tab::All => true,
            Tab::Reviewed => !unit.unreviewed(),
            Tab::Matching => unit.unreviewed() && unit.verdict == Verdict::Agrees,
            Tab::Attention => unit.unreviewed() && unit.verdict != Verdict::Agrees,
        }
    }

    pub(crate) fn ids(&self, tab: Tab) -> Vec<i64> {
        self.units
            .iter()
            .filter(|u| Self::in_tab(u, tab))
            .map(|u| u.id)
            .collect()
    }

    /// The first unit of the tab after `after` (a position), else the tab's first.
    pub(crate) fn next_in(&self, tab: Tab, after: i64) -> Option<&UnitAgreement> {
        let listed = || self.units.iter().filter(move |u| Self::in_tab(u, tab));
        listed()
            .find(|u| u.position > after)
            .or_else(|| listed().next())
    }

    pub(crate) fn counts(&self) -> Counts {
        let mut counts = Counts::default();
        for unit in self.units.iter().filter(|u| u.unreviewed()) {
            match unit.verdict {
                Verdict::Agrees => counts.matching += 1,
                Verdict::Differs { .. } => counts.differ += 1,
                Verdict::NoWords => counts.no_words += 1,
                Verdict::Excluded(_) => counts.excluded += 1,
            }
        }
        counts
    }

    pub(crate) fn tab_count(&self, tab: Tab) -> usize {
        self.units.iter().filter(|u| Self::in_tab(u, tab)).count()
    }

    /// The tab a request shows, and the notice that explains a fallback.
    ///
    /// The default is `attention` only when the batch form is offered, else `all`; a request
    /// with `units=` but no `show` is `all`; an unknown word is the default. Without a result,
    /// `attention` and `matching` fall back to `all` with a notice. A requested unit (`unit=`
    /// or a save's `to`) the tab would hide shows `all` around it.
    pub(crate) fn resolve(
        &self,
        show: Show,
        has_cursor: bool,
        focus: i64,
        batch: bool,
        focus_reviewed: Option<bool>,
    ) -> (Tab, Option<&'static str>) {
        let default = if batch { Tab::Attention } else { Tab::All };
        let wanted = match show {
            Show::Absent if has_cursor => Tab::All,
            Show::Absent | Show::Unknown => default,
            Show::Attention => Tab::Attention,
            Show::Matching => Tab::Matching,
            Show::Reviewed => Tab::Reviewed,
            Show::All => Tab::All,
        };
        if matches!(wanted, Tab::Attention | Tab::Matching) && !self.has_result() {
            return (Tab::All, Some(NO_RESULT_NOTICE));
        }
        if focus > 0 && wanted != Tab::All {
            let listed = if wanted == Tab::Reviewed {
                // Without a result the view has no units: the caller says whether it is decided.
                self.unit(focus)
                    .map(|u| !u.unreviewed())
                    .or(focus_reviewed)
                    .unwrap_or(false)
            } else {
                self.unit(focus).is_some_and(|u| Self::in_tab(u, wanted))
            };
            if !listed {
                return (Tab::All, Some(HIDDEN_NOTICE));
            }
        }
        (wanted, None)
    }

    /// A text difference that is too long to display (no diff was computed).
    pub(crate) fn diff_too_long(&self, id: i64) -> bool {
        self.unit(id).is_some_and(|u| {
            u.unreviewed()
                && matches!(
                    u.verdict,
                    Verdict::Differs {
                        reason: DiffReason::Tokens,
                        diff: None
                    }
                )
        })
    }

    /// The label and the reason text of a unit's verdict for its card header.
    pub(crate) fn label(&self, id: i64) -> Option<(&'static str, &'static str)> {
        let unit = self.unit(id)?;
        if !unit.unreviewed() {
            return None;
        }
        Some(match &unit.verdict {
            Verdict::Agrees => ("agrees", "Matches the PDF text layer"),
            Verdict::Differs {
                reason: DiffReason::Tokens,
                ..
            } => ("differs", "Differs from the PDF text layer"),
            Verdict::Differs {
                reason: DiffReason::OrphanWords,
                ..
            } => (
                "differs",
                "A text-layer word next to this unit is in no unit",
            ),
            Verdict::NoWords => (
                "no-words",
                "No second reading: no PDF text-layer word inside this unit",
            ),
            Verdict::Excluded(reason) => ("excluded", exclusion_text(*reason)),
        })
    }
}

fn exclusion_text(reason: Exclusion) -> &'static str {
    match reason {
        Exclusion::Owner => "Not compared: your own unit",
        Exclusion::Reviewed => "Already reviewed",
        Exclusion::Kind => {
            "Not compared: not running text (table, figure, picture, equation or form)"
        }
        Exclusion::TableProposal => "Not compared: inside a likely table",
        Exclusion::TableShape => "Not compared: its text looks like a table",
        Exclusion::TextBudget => "Not compared: past the page's text budget",
        Exclusion::OpenSuggestion => "Not compared: an open suggestion targets it",
        Exclusion::PageSuggestion => "Not compared: the page has an open page-level suggestion",
        Exclusion::NoText => "Not compared: no text",
        Exclusion::SharedWord => "Not compared: a text-layer word lies in two units",
        Exclusion::ExcludedArea => "Not compared: next to an excluded area",
    }
}

/// The comparison's label: what an agreement on this page proves, as the bundle declares.
pub(crate) fn kind_label(kind: ReadingKind) -> &'static str {
    match kind {
        ReadingKind::Independent => {
            "Compared with the PDF text layer (which may itself be OCR). These units were read from the page image, so a match means two readings produced the same words."
        }
        ReadingKind::SameLayer => {
            "Compared with the PDF text layer. These units were taken from that same layer, so a match shows the text was carried over intact, not that it is right."
        }
        ReadingKind::Derived => {
            "These units were built from the PDF text layer's words, so they match by construction."
        }
        ReadingKind::Unknown => {
            "Compared with the PDF text layer. Where these units came from is not known, so a match proves little."
        }
    }
}

/// Why a page has no comparison.
pub(crate) fn status_reason(status: Status) -> Option<&'static str> {
    match status {
        Status::Computed(_) => None,
        Status::NoWords => Some(
            "No comparison: this page has no stored PDF text-layer words (running import-source again on the bundle stores them).",
        ),
        Status::OverBound => Some(
            "No comparison: the page has more than 2,000 units or more than 64 KiB of unit text.",
        ),
        Status::TimedOut => {
            Some("No comparison: the read time limit was reached; reload to try again.")
        }
        Status::Unreadable => Some("No comparison: the page's agreement could not be read."),
        Status::Derived => Some("Units built from the PDF text layer's words; no comparison."),
        Status::WordsTooLarge => Some("No comparison: the stored words are over the size bound."),
    }
}

// ---------------------------------------------------------------------------------------
// Bounded display of a difference
// ---------------------------------------------------------------------------------------

/// One displayed change (either side may be empty).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShownChange {
    pub unit: String,
    pub layer: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShownDiff {
    pub changes: Vec<ShownChange>,
    /// More changes exist than are shown.
    pub more: bool,
}

fn clip(text: &str, limit: usize) -> (String, usize) {
    let mut chars = text.chars();
    let mut out: String = chars.by_ref().take(limit).collect();
    let used = out.chars().count();
    if chars.next().is_some() {
        out.push('\u{2026}');
    }
    (out, used)
}

/// What is left of the window's display bound.
pub(crate) struct Window {
    chars: usize,
    changes: usize,
}

impl Window {
    pub(crate) fn new() -> Self {
        Self {
            chars: SCREEN_CHARS,
            changes: SCREEN_CHANGES,
        }
    }
}

/// The bounded form of a diff: each side clipped, a card and the whole window capped, so the
/// response budget holds whatever the words are.
pub(crate) fn shown_diff(diff: &Diff, window: &mut Window) -> ShownDiff {
    let mut card = CARD_CHARS;
    let mut changes = Vec::new();
    for change in &diff.changes {
        if card == 0 || window.changes == 0 {
            break;
        }
        let (unit, a) = clip(&change.unit, CHANGE_CHARS.min(card));
        let (layer, b) = clip(&change.layer, CHANGE_CHARS.min(card - a));
        if a + b > window.chars {
            break;
        }
        card -= a + b;
        window.chars -= a + b;
        window.changes -= 1;
        changes.push(ShownChange { unit, layer });
    }
    let more = diff.truncated || changes.len() < diff.changes.len();
    ShownDiff { changes, more }
}

impl AgreementView {
    /// The bounded differences of the units in a window (those whose verdict has a diff).
    pub(crate) fn diffs(&self, units: &[i64]) -> BTreeMap<i64, ShownDiff> {
        let mut window = Window::new();
        let mut shown = BTreeMap::new();
        for &id in units {
            if let Some(unit) = self.unit(id)
                && unit.unreviewed()
                && let Verdict::Differs {
                    diff: Some(diff), ..
                } = &unit.verdict
            {
                shown.insert(id, shown_diff(diff, &mut window));
            }
        }
        shown
    }
}

// ---------------------------------------------------------------------------------------
// The pass
// ---------------------------------------------------------------------------------------

/// One unit of the page as the pass reads it.
#[derive(Clone, Debug)]
pub(super) struct UnitRow {
    pub id: i64,
    pub position: i64,
    pub kind: String,
    pub origin: String,
    pub state: String,
    pub polygon: String,
    /// `None` when the stored text is over the read bound.
    pub text: Option<String>,
}

impl UnitRow {
    fn candidate(&self) -> bool {
        self.origin == "extracted" && self.state == "unreviewed"
    }
}

/// The memoised pass: the rule without the caller's exclusions, and the units whose text alone
/// reads as a table (a function of the text, so of the page revision).
pub(crate) struct Pass {
    pub kind: ReadingKind,
    /// `None`: no usable words.
    pub agreement: Option<Agreement>,
    pub shaped: HashSet<i64>,
}

/// The text each candidate contributes within the page budget (`None` past it, or oversized);
/// other units contribute none.
pub(super) fn budgeted(rows: &[UnitRow]) -> Vec<Option<&str>> {
    let mut budget = TEXT_BUDGET;
    rows.iter()
        .map(|row| {
            if !row.candidate() {
                return None;
            }
            let text = row.text.as_deref()?;
            if text.trim().is_empty() {
                return Some("");
            }
            (text.len() <= budget).then(|| {
                budget -= text.len();
                text
            })
        })
        .collect()
}

fn inputs<'a>(
    rows: &'a [UnitRow],
    texts: &[Option<&'a str>],
    marks: Option<&HashMap<i64, Exclusion>>,
) -> Vec<UnitInput<'a>> {
    rows.iter()
        .zip(texts)
        .map(|(row, text)| UnitInput {
            id: row.id,
            kind: &row.kind,
            origin: &row.origin,
            state: &row.state,
            polygon: &row.polygon,
            text: text.unwrap_or(""),
            excluded: marks.and_then(|m| m.get(&row.id).copied()),
        })
        .collect()
}

/// The pass without the caller's exclusions (what is memoised).
pub(super) fn base_pass(rows: &[UnitRow], texts: &[Option<&str>], words: &[PageWord]) -> Agreement {
    evaluate(&inputs(rows, texts, None), words, &[])
}

/// The candidates whose text alone a table shape scores at the bound or more.
pub(super) fn shaped_units(rows: &[UnitRow], texts: &[Option<&str>]) -> HashSet<i64> {
    rows.iter()
        .zip(texts)
        .filter(|(row, _)| row.candidate())
        .filter(|(_, text)| {
            text.is_some_and(|t| {
                shapes::best(&ShapeInput::new(&[t]))
                    .iter()
                    .any(|d| d.shape != Shape::Plain && d.score >= SHAPE_SCORE)
            })
        })
        .map(|(row, _)| row.id)
        .collect()
}

/// What the caller knows about the page, per view.
pub(super) struct Marks<'a> {
    pub proposed: &'a HashSet<i64>,
    pub open_units: &'a HashSet<i64>,
    pub page_level: bool,
}

/// The caller's exclusions: they only mark candidates.
pub(super) fn marks(
    rows: &[UnitRow],
    texts: &[Option<&str>],
    shaped: &HashSet<i64>,
    known: &Marks<'_>,
) -> HashMap<i64, Exclusion> {
    let mut marked = HashMap::new();
    for (row, text) in rows.iter().zip(texts) {
        if !row.candidate() || row.text.as_deref().is_some_and(|t| t.trim().is_empty()) {
            continue;
        }
        let reason = if known.page_level {
            Some(Exclusion::PageSuggestion)
        } else if known.open_units.contains(&row.id) {
            Some(Exclusion::OpenSuggestion)
        } else if known.proposed.contains(&row.id) {
            Some(Exclusion::TableProposal)
        } else if shaped.contains(&row.id) {
            Some(Exclusion::TableShape)
        } else if text.is_none() {
            Some(Exclusion::TextBudget)
        } else {
            None
        };
        if let Some(reason) = reason {
            marked.insert(row.id, reason);
        }
    }
    marked
}

/// The base verdicts with the marks applied: what a fresh `evaluate` with the marks gives.
/// Owner, reviewed and kind exclusions come first, as in the rule; a mark replaces any
/// verdict after them.
pub(super) fn apply_marks(base: &Agreement, marked: &HashMap<i64, Exclusion>) -> Vec<UnitVerdict> {
    base.verdicts
        .iter()
        .map(|unit| {
            let keep = matches!(
                unit.verdict,
                Verdict::Excluded(Exclusion::Owner | Exclusion::Reviewed | Exclusion::Kind)
            );
            let verdict = match marked.get(&unit.id) {
                Some(reason) if !keep => Verdict::Excluded(*reason),
                _ => unit.verdict.clone(),
            };
            UnitVerdict {
                id: unit.id,
                verdict,
            }
        })
        .collect()
}

/// A fresh evaluation with the marks (the reference the memoised path must equal).
#[cfg(test)]
pub(super) fn fresh(
    rows: &[UnitRow],
    texts: &[Option<&str>],
    words: &[PageWord],
    marked: &HashMap<i64, Exclusion>,
) -> Agreement {
    evaluate(&inputs(rows, texts, Some(marked)), words, &[])
}

fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map_or_else(String::new, |line| clip(line, FIRST_LINE_CHARS).0)
}

fn assemble(rows: &[UnitRow], pass: &Pass, marked: &HashMap<i64, Exclusion>) -> AgreementView {
    let Some(base) = pass.agreement.as_ref() else {
        return AgreementView::unavailable(Status::NoWords);
    };
    if !base.computed {
        return AgreementView::unavailable(Status::OverBound);
    }
    let verdicts = apply_marks(base, marked);
    let units = rows
        .iter()
        .zip(verdicts)
        .map(|(row, unit)| UnitAgreement {
            id: row.id,
            position: row.position,
            state: row.state.clone(),
            kind: row.kind.clone(),
            first_line: row.text.as_deref().map(first_line).unwrap_or_default(),
            verdict: unit.verdict,
        })
        .collect();
    AgreementView::computed(units, base.facts.orphans.clone(), pass.kind)
}

// ---------------------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------------------

/// What the pass needs when it is not memoised.
struct PassInput {
    words: Option<Vec<u8>>,
    name: String,
    version: String,
    settings: Value,
}

/// The memoised pass, or what computing it needs.
enum PassSource {
    Memo(Arc<Pass>),
    Compute(PassInput),
}

struct AgreementRead {
    key: PassKey,
    rows: Vec<UnitRow>,
    over_bound: bool,
    open_units: HashSet<i64>,
    page_level: bool,
    source: Option<PassSource>,
}

fn read_units(tx: &Connection, page: i64) -> Result<Vec<UnitRow>, AppError> {
    let mut query = tx.prepare(&format!(
        "{ORDERED_UNITS} SELECT id,position,{},origin,state,{},{} FROM ordered ORDER BY position LIMIT {}",
        text("kind", 100),
        text("polygon", 65_536),
        text("raw_text", 65_536),
        MAX_UNITS + 1
    ))?;
    let found = query.query_map([page], |r| {
        Ok(UnitRow {
            id: r.get(0)?,
            position: r.get(1)?,
            kind: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            origin: r.get(3)?,
            state: r.get(4)?,
            polygon: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
            text: r.get(6)?,
        })
    })?;
    Ok(found.collect::<Result<_, _>>()?)
}

/// The units targeted by an open suggestion (not dismissed, not applied) and whether the page
/// has an open page-level `missing_text` or `note`.
fn read_open_suggestions(tx: &Connection, page: i64) -> Result<(HashSet<i64>, bool), AppError> {
    let (mut open_units, mut page_level) = (HashSet::new(), false);
    let mut query = tx.prepare(&format!(
        "SELECT s.kind,{} FROM source_suggestions s WHERE s.source_id=(SELECT source_id FROM source_pages WHERE id=?1) \
         AND NOT EXISTS(SELECT 1 FROM source_suggestion_applications a WHERE a.suggestion_id=s.id) \
         AND NOT EXISTS(SELECT 1 FROM source_suggestion_dismissals d WHERE d.suggestion_id=s.id) \
         AND (json_extract(s.targets,'$.page')=?1 OR EXISTS(SELECT 1 FROM source_units u WHERE u.id=coalesce(json_extract(s.targets,'$.target.unit'),json_extract(s.targets,'$.targets[0].unit')) AND u.page_id=?1))",
        text("s.targets", 65_536)
    ))?;
    let found = query.query_map([page], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
    })?;
    for row in found {
        let (kind, targets) = row?;
        // A stale suggestion is kept as open (the stricter reading); a target that cannot be
        // read excludes the whole page.
        let Some(target) = targets.and_then(|t| serde_json::from_str::<SuggestionTarget>(&t).ok())
        else {
            page_level = true;
            continue;
        };
        match target {
            SuggestionTarget::Unit { target } => {
                open_units.insert(target.unit);
            }
            SuggestionTarget::Units { targets } => {
                open_units.extend(targets.iter().map(|t| t.unit));
            }
            SuggestionTarget::Region { page: at, .. } | SuggestionTarget::Page { page: at, .. }
                if (kind == "missing_text" || kind == "note") && at == page =>
            {
                page_level = true;
            }
            _ => {}
        }
    }
    Ok((open_units, page_level))
}

/// The stored words and the extractor provenance the bundle declares.
fn read_pass_input(tx: &Connection, page: i64) -> Result<PassInput, AppError> {
    let words = tx
        .query_row(
            &format!(
                "SELECT CASE WHEN length(words)<={} THEN words END FROM source_page_words WHERE page_id=?1",
                page_words::MAX_ROW_BYTES
            ),
            [page],
            |r| r.get::<_, Option<Vec<u8>>>(0),
        )
        .optional()?
        .flatten();
    let (name, version, settings) = tx
        .query_row(
            "SELECT json_extract(j,'$.extractor.name'),json_extract(j,'$.extractor.version'),json_extract(j,'$.extractor.settings') FROM (SELECT CASE WHEN json_valid(CAST(s.prepared_json AS TEXT)) THEN CAST(s.prepared_json AS TEXT) END j FROM sources s JOIN source_pages p ON p.source_id=s.id WHERE p.id=?1)",
            [page],
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0).unwrap_or(None),
                    r.get::<_, Option<String>>(1).unwrap_or(None),
                    r.get::<_, Option<String>>(2).unwrap_or(None),
                ))
            },
        )
        .optional()?
        .unwrap_or((None, None, None));
    Ok(PassInput {
        words,
        name: name.unwrap_or_default(),
        version: version.unwrap_or_default(),
        settings: settings
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(Value::Null),
    })
}

fn read_inputs(
    tx: &Connection,
    page: i64,
    cache: &super::proposals::ProposalCache,
) -> Result<AgreementRead, AppError> {
    let (revision, words_len) = tx
        .query_row(
            "SELECT p.revision,coalesce((SELECT length(words) FROM source_page_words w WHERE w.page_id=p.id),-1) FROM source_pages p WHERE p.id=?1",
            [page],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .optional()?
        .ok_or(AppError::SourceNotFound)?;
    let key = (page, revision, words_len, RULE_VERSION, 0);
    let rows = read_units(tx, page)?;
    if rows.len() > MAX_UNITS {
        return Ok(AgreementRead {
            key,
            rows: Vec::new(),
            over_bound: true,
            open_units: HashSet::new(),
            page_level: false,
            source: None,
        });
    }
    let (open_units, page_level) = read_open_suggestions(tx, page)?;
    let source = if words_too_large(words_len) {
        None
    } else if let Some(memo) = cache.pass_get(&key) {
        Some(PassSource::Memo(memo))
    } else {
        Some(PassSource::Compute(read_pass_input(tx, page)?))
    };
    Ok(AgreementRead {
        key,
        rows,
        over_bound: false,
        open_units,
        page_level,
        source,
    })
}

impl Store {
    /// The agreement of a page: never an error for a timeout or a missing second reading (the
    /// view says why there is none); a storage or authority error is returned.
    pub(crate) async fn material_agreement(
        &self,
        auth: crate::auth::RequestAuth,
        page: i64,
    ) -> Result<AgreementView, AppError> {
        let Some(proposals) = self
            .material_table_proposals_checked(auth.clone(), page)
            .await?
        else {
            return Ok(AgreementView::unavailable(Status::TimedOut));
        };
        let cache = Arc::clone(&self.2);
        let read = self
            .run_bounded_auth({
                let cache = Arc::clone(&cache);
                move |db| {
                    let tx = db.transaction()?;
                    auth.recheck_owner(&tx)?;
                    let read = match super::review::read_with_deadline(&tx, "agreement", || {
                        read_inputs(&tx, page, &cache)
                    }) {
                        Ok(read) => Some(read),
                        Err(AppError::Invalid(super::review::READ_TIMEOUT)) => None,
                        Err(e) => return Err(e),
                    };
                    auth.recheck_owner(&tx)?;
                    tx.commit()?;
                    Ok(read)
                }
            })
            .await?;
        let Some(read) = read else {
            return Ok(AgreementView::unavailable(Status::TimedOut));
        };
        if read.over_bound {
            return Ok(AgreementView::unavailable(Status::OverBound));
        }
        let Some(source) = read.source else {
            return Ok(AgreementView::unavailable(Status::WordsTooLarge));
        };
        tokio::task::spawn_blocking(move || {
            let texts = budgeted(&read.rows);
            let pass = match source {
                PassSource::Memo(pass) => pass,
                PassSource::Compute(input) => {
                    let kind = classify_provenance(&input.name, &input.version, &input.settings);
                    let words = input
                        .words
                        .as_deref()
                        .and_then(|bytes| page_words::decode(bytes).ok());
                    let agreement = words
                        .filter(|w| !w.is_empty())
                        .map(|words| base_pass(&read.rows, &texts, &words));
                    let shaped = shaped_units(&read.rows, &texts);
                    cache.pass_put(
                        read.key,
                        Pass {
                            kind,
                            agreement,
                            shaped,
                        },
                    )
                }
            };
            if pass.kind == ReadingKind::Derived {
                return AgreementView::unavailable(Status::Derived);
            }
            let proposed: HashSet<i64> = proposals.iter().flat_map(|p| p.units.clone()).collect();
            let marked = marks(
                &read.rows,
                &texts,
                &pass.shaped,
                &Marks {
                    proposed: &proposed,
                    open_units: &read.open_units,
                    page_level: read.page_level,
                },
            );
            assemble(&read.rows, &pass, &marked)
        })
        .await
        .map_err(|_| AppError::Worker)
    }
}

/// A difference's change, for the tests of the display bound.
#[cfg(test)]
pub(super) fn change(unit: &str, layer: &str) -> super::agreement::Change {
    super::agreement::Change {
        unit: unit.into(),
        layer: layer.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, position: i64, kind: &str, state: &str, polygon: &str, text: &str) -> UnitRow {
        UnitRow {
            id,
            position,
            kind: kind.into(),
            origin: "extracted".into(),
            state: state.into(),
            polygon: polygon.into(),
            text: Some(text.into()),
        }
    }

    fn word(text: &str, x: f32, y: f32) -> PageWord {
        PageWord {
            text: text.into(),
            bounds: [x, y, x + 20.0, y + 10.0],
        }
    }

    fn view(verdicts: &[(i64, &str, Verdict)], kind: ReadingKind) -> AgreementView {
        AgreementView::computed(
            verdicts
                .iter()
                .enumerate()
                .map(|(i, (id, state, verdict))| UnitAgreement {
                    id: *id,
                    position: i64::try_from(i).unwrap() + 1,
                    state: (*state).into(),
                    kind: "Text".into(),
                    first_line: String::new(),
                    verdict: verdict.clone(),
                })
                .collect(),
            Vec::new(),
            kind,
        )
    }

    fn sample() -> AgreementView {
        view(
            &[
                (1, "unreviewed", Verdict::Agrees),
                (2, "unreviewed", Verdict::NoWords),
                (3, "accepted", Verdict::Excluded(Exclusion::Reviewed)),
                (4, "unreviewed", Verdict::Excluded(Exclusion::Kind)),
                (
                    5,
                    "unreviewed",
                    Verdict::Differs {
                        reason: DiffReason::Tokens,
                        diff: None,
                    },
                ),
            ],
            ReadingKind::Independent,
        )
    }

    fn resolve(
        v: &AgreementView,
        show: Show,
        cursor: bool,
        focus: i64,
    ) -> (Tab, Option<&'static str>) {
        v.resolve(show, cursor, focus, false, None)
    }

    #[test]
    fn tab_membership_and_counts() {
        let v = sample();
        assert_eq!(v.ids(Tab::Attention), vec![2, 4, 5]);
        assert_eq!(v.ids(Tab::Matching), vec![1]);
        assert_eq!(v.ids(Tab::Reviewed), vec![3]);
        assert_eq!(v.ids(Tab::All), vec![1, 2, 3, 4, 5]);
        let c = v.counts();
        assert_eq!((c.matching, c.differ, c.no_words, c.excluded), (1, 1, 1, 1));
        assert_eq!(v.tab_count(Tab::Attention), 3);
    }

    #[test]
    fn the_default_tab_follows_the_batch_rule() {
        assert!(batch_rule(Some(ReadingKind::Independent)));
        for kind in [
            ReadingKind::SameLayer,
            ReadingKind::Derived,
            ReadingKind::Unknown,
        ] {
            assert!(!batch_rule(Some(kind)));
        }
        assert!(!batch_rule(None));
        // Until slice 4 ships its form, nothing is offered, so the default is `all`.
        let v = sample();
        assert!(!v.offers_batch());
        assert_eq!(resolve(&v, Show::Absent, false, 0).0, Tab::All);
        assert_eq!(resolve(&v, Show::Unknown, false, 0).0, Tab::All);
        assert_eq!(resolve(&v, Show::Absent, true, 0).0, Tab::All);
        // With the form offered the default is `attention`, but `units=` alone is `all`.
        assert_eq!(
            v.resolve(Show::Absent, false, 0, true, None).0,
            Tab::Attention
        );
        assert_eq!(
            v.resolve(Show::Unknown, false, 0, true, None).0,
            Tab::Attention
        );
        assert_eq!(v.resolve(Show::Absent, true, 0, true, None).0, Tab::All);
        assert_eq!(v.resolve(Show::All, false, 0, true, None).0, Tab::All);
    }

    #[test]
    fn requested_tabs_unknown_words_and_missing_results() {
        let v = sample();
        assert_eq!(
            resolve(&v, Show::Attention, true, 0),
            (Tab::Attention, None)
        );
        assert_eq!(resolve(&v, Show::Matching, false, 0), (Tab::Matching, None));
        assert_eq!(resolve(&v, Show::Reviewed, false, 0), (Tab::Reviewed, None));
        let none = AgreementView::unavailable(Status::NoWords);
        assert_eq!(
            resolve(&none, Show::Attention, false, 0),
            (Tab::All, Some(NO_RESULT_NOTICE))
        );
        assert_eq!(
            resolve(&none, Show::Matching, false, 0),
            (Tab::All, Some(NO_RESULT_NOTICE))
        );
        assert_eq!(resolve(&none, Show::Absent, false, 0), (Tab::All, None));
        // The default can never be `attention` without a result.
        assert_eq!(none.resolve(Show::Absent, false, 0, true, None).0, Tab::All);
    }

    #[test]
    fn a_focused_unit_the_tab_hides_shows_all() {
        let v = sample();
        assert_eq!(
            resolve(&v, Show::Attention, false, 5),
            (Tab::Attention, None)
        );
        assert_eq!(
            resolve(&v, Show::Attention, false, 1),
            (Tab::All, Some(HIDDEN_NOTICE))
        );
        assert_eq!(
            resolve(&v, Show::Reviewed, false, 1),
            (Tab::All, Some(HIDDEN_NOTICE))
        );
        assert_eq!(
            resolve(&v, Show::Matching, false, 999),
            (Tab::All, Some(HIDDEN_NOTICE))
        );
    }

    #[test]
    fn without_a_result_the_reviewed_tab_asks_the_unit_itself() {
        let none = AgreementView::unavailable(Status::NoWords);
        assert_eq!(
            none.resolve(Show::Reviewed, false, 7, false, Some(true)),
            (Tab::Reviewed, None)
        );
        assert_eq!(
            none.resolve(Show::Reviewed, false, 7, false, Some(false)),
            (Tab::All, Some(HIDDEN_NOTICE))
        );
        assert_eq!(
            none.resolve(Show::Reviewed, false, 7, false, None),
            (Tab::All, Some(HIDDEN_NOTICE))
        );
    }

    #[test]
    fn a_request_names_its_tab_without_a_view() {
        assert_eq!(Show::Reviewed.tab(), Tab::Reviewed);
        assert_eq!(Show::Attention.tab(), Tab::Attention);
        assert_eq!(Show::Absent.tab(), Tab::All);
        assert_eq!(Show::Unknown.tab(), Tab::All);
    }

    #[test]
    fn the_next_unit_of_a_tab_wraps_inside_it() {
        let v = sample();
        assert_eq!(v.next_in(Tab::Attention, 0).unwrap().id, 2);
        assert_eq!(v.next_in(Tab::Attention, 2).unwrap().id, 4);
        assert_eq!(v.next_in(Tab::Attention, 5).unwrap().id, 2);
        assert!(v.next_in(Tab::Matching, 1).is_some());
        let empty = view(
            &[(1, "accepted", Verdict::Excluded(Exclusion::Reviewed))],
            ReadingKind::Independent,
        );
        assert!(empty.next_in(Tab::Attention, 0).is_none());
    }

    #[test]
    fn a_shown_difference_is_bounded() {
        let long = "w ".repeat(5_000);
        let diff = Diff {
            changes: (0..20).map(|_| change(&long, &long)).collect(),
            truncated: true,
        };
        let mut window = Window::new();
        let shown = shown_diff(&diff, &mut window);
        let total: usize = shown
            .changes
            .iter()
            .map(|c| c.unit.chars().count() + c.layer.chars().count())
            .sum();
        assert!(total <= CARD_CHARS + 2 * shown.changes.len());
        assert!(shown.more);
        // The window runs out across cards, in characters and in changes.
        let mut tiny = Window {
            chars: 10,
            changes: SCREEN_CHANGES,
        };
        let shown = shown_diff(&diff, &mut tiny);
        assert!(shown.changes.is_empty() && shown.more);
        let mut few = Window {
            chars: SCREEN_CHARS,
            changes: 3,
        };
        let many = Diff {
            changes: (0..10).map(|_| change("a", "b")).collect(),
            truncated: false,
        };
        let shown = shown_diff(&many, &mut few);
        assert_eq!(shown.changes.len(), 3);
        assert!(shown.more);
    }

    #[test]
    fn a_word_row_over_the_bound_is_not_read() {
        assert!(!words_too_large(-1));
        assert!(!words_too_large(0));
        let max = i64::try_from(page_words::MAX_ROW_BYTES).unwrap();
        assert!(!words_too_large(max));
        assert!(words_too_large(max + 1));
    }

    #[test]
    fn the_memoised_pass_with_marks_equals_a_fresh_evaluation() {
        let poly = |x: f32, y: f32| {
            format!(
                "[[{x},{y}],[{},{y}],[{},{}],[{x},{}]]",
                x + 30.0,
                x + 30.0,
                y + 14.0,
                y + 14.0
            )
        };
        let rows = vec![
            row(1, 1, "Text", "unreviewed", &poly(10.0, 10.0), "alpha"),
            row(2, 2, "Text", "unreviewed", &poly(10.0, 40.0), "beta"),
            row(3, 3, "Text", "unreviewed", &poly(10.0, 70.0), "gamma"),
            row(4, 4, "Text", "accepted", &poly(10.0, 100.0), "delta"),
            row(5, 5, "Table", "unreviewed", &poly(10.0, 130.0), "x"),
            row(6, 6, "Text", "unreviewed", &poly(10.0, 160.0), "   "),
        ];
        let words = vec![
            word("alpha", 12.0, 11.0),
            word("beta", 12.0, 41.0),
            word("gammX", 12.0, 71.0),
            word("delta", 12.0, 101.0),
        ];
        let texts = budgeted(&rows);
        let base = base_pass(&rows, &texts, &words);
        let shaped = shaped_units(&rows, &texts);
        let proposed = HashSet::from([2]);
        let open = HashSet::from([1]);
        let known = Marks {
            proposed: &proposed,
            open_units: &open,
            page_level: false,
        };
        let marked = marks(&rows, &texts, &shaped, &known);
        assert_eq!(marked.get(&1), Some(&Exclusion::OpenSuggestion));
        assert_eq!(marked.get(&2), Some(&Exclusion::TableProposal));
        assert!(!marked.contains_key(&6));
        let overlaid = apply_marks(&base, &marked);
        let reference = fresh(&rows, &texts, &words, &marked);
        assert_eq!(overlaid, reference.verdicts);
        // Without marks the unit agrees; the mark is what hides it (a dismissed suggestion
        // shows at once because only the marks change).
        assert_eq!(base.verdicts[0].verdict, Verdict::Agrees);
        let empty = HashSet::new();
        let none = marks(
            &rows,
            &texts,
            &shaped,
            &Marks {
                proposed: &empty,
                open_units: &empty,
                page_level: false,
            },
        );
        assert_eq!(apply_marks(&base, &none)[0].verdict, Verdict::Agrees);
        assert_eq!(
            apply_marks(&base, &none),
            fresh(&rows, &texts, &words, &none).verdicts
        );
        // A page-level suggestion marks every candidate, never a reviewed unit or a table.
        let page = marks(
            &rows,
            &texts,
            &shaped,
            &Marks {
                proposed: &empty,
                open_units: &empty,
                page_level: true,
            },
        );
        let overlaid = apply_marks(&base, &page);
        assert_eq!(overlaid, fresh(&rows, &texts, &words, &page).verdicts);
        assert_eq!(overlaid[3].verdict, Verdict::Excluded(Exclusion::Reviewed));
        assert_eq!(overlaid[4].verdict, Verdict::Excluded(Exclusion::Kind));
        assert_eq!(
            overlaid[0].verdict,
            Verdict::Excluded(Exclusion::PageSuggestion)
        );
    }
}
