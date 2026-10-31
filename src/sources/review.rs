//! Bounded review advice reads and authenticated, atomic application of stored advice.
use super::{
    OwnerUnit, SuggestionApplication, SuggestionTarget, UnitReview,
    materials::{LOOKUP_TEXT, audit, read_text, text},
};
use crate::{
    AppError, Store,
    auth::{CsrfAdmission, RequestAuth, with_domain_request},
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::{
    fmt::Write,
    time::{Duration, Instant},
};

type AdvicePage = (Vec<Advice>, Option<i64>);
type ScreenAdvice = std::collections::BTreeMap<i64, AdvicePage>;

pub(crate) const READ_TIMEOUT: &str =
    "Review read time limit reached; retry with a smaller page or list.";

pub(super) fn read_with_deadline<T>(
    db: &Connection,
    label: &str,
    read: impl FnOnce() -> Result<T, AppError>,
) -> Result<T, AppError> {
    let deadline = read_deadline(db, label);
    let result = read();
    if deadline.fired() {
        Err(AppError::Invalid(READ_TIMEOUT))
    } else {
        result
    }
}

#[cfg(test)]
pub(crate) static READ_DEADLINE: std::sync::Mutex<Option<(&str, Duration)>> =
    std::sync::Mutex::new(None);

fn read_deadline<'a>(db: &'a Connection, label: &str) -> super::search::Budget<'a> {
    #[cfg(not(test))]
    let _ = label;
    #[cfg(test)]
    {
        let mut hook = READ_DEADLINE.lock().unwrap();
        if hook.as_ref().is_some_and(|(name, _)| *name == label) {
            let duration = hook.take().unwrap().1;
            return super::search::Budget::checking_every(db, duration, 1);
        }
    }
    super::search::Budget::start(db, Duration::from_millis(250))
}

#[derive(Clone)]
pub(crate) struct Segment {
    pub kind: &'static str,
    pub text: String,
}
#[derive(Clone)]
pub(crate) struct Difference {
    pub segments: Vec<Segment>,
    pub fallback: bool,
    pub old: String,
    pub new: String,
}
#[derive(Default)]
pub(crate) struct Budget {
    count: usize,
    bytes: usize,
    deadline: Option<Instant>,
}
impl Budget {
    fn reserve(&mut self, bytes: usize) -> bool {
        if self.bytes.saturating_add(bytes) > 2 * 1024 * 1024 {
            false
        } else {
            self.bytes += bytes;
            true
        }
    }
    fn comparison_available(&self) -> bool {
        self.count < 100
            && self.bytes < 2 * 1024 * 1024
            && self
                .deadline
                .is_none_or(|deadline| Instant::now() < deadline)
    }
    fn retain(&mut self, value: &str, copies: usize) -> String {
        let remaining = (2 * 1024 * 1024_usize).saturating_sub(self.bytes) / copies;
        let mut end = remaining.min(value.len());
        if value.len() > remaining {
            end = end.min(
                value
                    .char_indices()
                    .nth(2000)
                    .map_or(value.len(), |(index, _)| index),
            );
        }
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        self.bytes += end * copies;
        value[..end].to_owned()
    }
    pub(crate) fn difference(&mut self, old: &str, new: &str) -> Difference {
        // HTML escaping can expand a byte to six bytes; every change also has
        // a wrapper. Account for both, rather than budgeting only source text.
        let size = old.len().saturating_add(new.len()).saturating_mul(20);
        // Leave room in the authenticated five-second request window for reads and
        // rendering even when every individual comparison exhausts its deadline.
        let deadline = *self
            .deadline
            .get_or_insert_with(|| Instant::now() + Duration::from_millis(250));
        let allowed = self.count < 100
            && self.bytes.saturating_add(size) <= 2 * 1024 * 1024
            && Instant::now() < deadline;
        if allowed {
            self.count += 1;
        }
        let mut result = difference(
            old,
            new,
            allowed.then(|| deadline.min(Instant::now() + Duration::from_millis(50))),
        );
        if result.fallback {
            result.old = self.retain(&result.old, 6);
            result.new = self.retain(&result.new, 6);
        } else {
            self.bytes += size;
        }
        result
    }
}
pub(crate) fn difference(old: &str, new: &str, deadline: Option<Instant>) -> Difference {
    let mut result = Difference {
        segments: Vec::new(),
        fallback: true,
        old: old.chars().take(2000).collect(),
        new: new.chars().take(2000).collect(),
    };
    let Some(deadline) = deadline else {
        return result;
    };
    if Instant::now() >= deadline {
        return result;
    }
    let diff = similar::TextDiff::configure()
        .deadline(deadline)
        .diff_unicode_words(old, new);
    if Instant::now() >= deadline {
        return result;
    }
    for change in diff.iter_all_changes() {
        let kind = match change.tag() {
            similar::ChangeTag::Delete => "delete",
            similar::ChangeTag::Insert => "insert",
            similar::ChangeTag::Equal => "equal",
        };
        if let Some(last) = result.segments.last_mut().filter(|last| last.kind == kind) {
            last.text.push_str(change.value());
        } else {
            result.segments.push(Segment {
                kind,
                text: change.value().to_owned(),
            });
        }
    }
    result.old.clear();
    result.new.clear();
    result.fallback = false;
    result
}

#[derive(Clone, Copy)]
enum RowVisibility {
    Visible,
    Omitted,
}

#[derive(Clone)]
pub(crate) struct Advice {
    pub id: i64,
    pub page: i64,
    pub kind: String,
    pub origin: String,
    pub rationale: String,
    pub report: Option<i64>,
    pub target: SuggestionTarget,
    pub suggested: String,
    pub lookup: String,
    pub state: String,
    pub difference: Difference,
    pub target_label: String,
    pub cursor: i64,
    pub lookup_truncated: bool,
    pub suggested_truncated: bool,
    pub anchor: String,
    pub target_links: Vec<(i64, i64)>,
    pub compact: bool,
    pub compact_targets: String,
    visibility: RowVisibility,
}
impl Advice {
    pub(crate) fn row_omitted(&self) -> bool {
        matches!(self.visibility, RowVisibility::Omitted)
    }
    fn write_back_link(&mut self, db: &Connection) -> Result<String, AppError> {
        if let Some(unit) = targets(&self.target).first() {
            self.cursor = db.query_row(
                &format!(
                    "{} SELECT position-1 FROM ordered WHERE id=?2",
                    super::materials::ORDERED_UNITS
                ),
                params![self.page, unit.unit],
                |r| r.get(0),
            )?;
        }
        Ok(self.back_link())
    }
    pub(crate) fn actionable(&self) -> bool {
        self.state == "open" && self.kind != "note"
    }
    pub(crate) fn back_link(&self) -> String {
        if let Some(target) = targets(&self.target).first() {
            format!(
                "/admin/materials/pages/{}?units={}#unit-{}",
                self.page, self.cursor, target.unit
            )
        } else {
            format!(
                "/admin/materials/pages/{}?units=0&notes={}#page-notes",
                self.page,
                self.id - 1
            )
        }
    }
}
fn targets(target: &SuggestionTarget) -> Vec<super::SuggestionUnit> {
    match target {
        SuggestionTarget::Unit { target } => vec![target.clone()],
        SuggestionTarget::Units { targets } => targets.clone(),
        _ => Vec::new(),
    }
}
fn load(db: &Connection, id: i64, preview: bool) -> Result<Advice, AppError> {
    load_many(db, &[id], preview)?
        .pop()
        .ok_or(AppError::SourceNotFound)
}
struct StoredAdvice {
    id: i64,
    kind: String,
    origin: String,
    rationale: String,
    json: String,
    suggested: String,
    report: Option<i64>,
    applied: bool,
    dismissed: bool,
    source: i64,
    suggested_truncated: bool,
}
fn load_many(db: &Connection, ids: &[i64], preview: bool) -> Result<Vec<Advice>, AppError> {
    load_many_with_budget(db, ids, preview, None)
}
fn load_many_with_budget(
    db: &Connection,
    ids: &[i64],
    preview: bool,
    screen_page: Option<i64>,
) -> Result<Vec<Advice>, AppError> {
    let revisions = screen_page
        .map(|page| {
            let mut query = db.prepare("SELECT id,revision FROM source_units WHERE page_id=?1")?;
            query
                .query_map([page], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?
                .collect::<Result<std::collections::BTreeMap<_, _>, rusqlite::Error>>()
        })
        .transpose()?;
    let mut lookup_rows = screen_page.map(|_| 4096);
    let suggested_column = text("coalesce(suggested_text,'')", 80000);
    let suggested_column = if preview {
        format!("substr({suggested_column},1,2000)")
    } else {
        suggested_column
    };
    let sql = format!(
        "SELECT {},{},{},{},{},report_id,EXISTS(SELECT 1 FROM source_suggestion_applications WHERE suggestion_id=s.id),EXISTS(SELECT 1 FROM source_suggestion_dismissals WHERE suggestion_id=s.id),source_id,id,length(coalesce(suggested_text,''))>{} FROM source_suggestions s WHERE id IN (SELECT value FROM json_each(?1)) ORDER BY id",
        text("kind", 100),
        text("origin", 400),
        if preview {
            format!("substr({},1,200)", text("rationale", 20000))
        } else {
            text("rationale", 20000)
        },
        text("targets", 65536),
        suggested_column,
        if preview { 2000 } else { 20000 },
    );
    let mut query = db.prepare(&sql)?;
    let stored = query
        .query_map([serde_json::to_string(ids)?], |r| {
            Ok(StoredAdvice {
                kind: read_text(r, 0)?,
                origin: read_text(r, 1)?,
                rationale: read_text(r, 2)?,
                json: read_text(r, 3)?,
                suggested: read_text(r, 4)?,
                report: r.get(5)?,
                applied: r.get(6)?,
                dismissed: r.get(7)?,
                source: r.get(8)?,
                id: r.get(9)?,
                suggested_truncated: r.get(10)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    stored
        .into_iter()
        .map(|row| {
            load_evidence(
                db,
                row,
                preview,
                &mut lookup_rows,
                screen_page.zip(revisions.as_ref()),
            )
        })
        .collect()
}
fn load_evidence(
    db: &Connection,
    stored: StoredAdvice,
    preview: bool,
    lookup_rows: &mut Option<usize>,
    screen_revisions: Option<(i64, &std::collections::BTreeMap<i64, i64>)>,
) -> Result<Advice, AppError> {
    let StoredAdvice {
        id,
        kind,
        origin,
        rationale,
        json,
        suggested,
        report,
        applied,
        dismissed,
        source,
        suggested_truncated,
    } = stored;
    let target: SuggestionTarget = serde_json::from_str(&json)?;
    let mut lookup = String::new();
    let mut stale = false;
    let mut lookup_truncated = false;
    let limit: usize = if preview { 2000 } else { 20000 };
    let mut page = match &target {
        SuggestionTarget::Page { page, .. } | SuggestionTarget::Region { page, .. } => *page,
        _ => 0,
    };
    let units = targets(&target);
    let read_lookup = lookup_rows.as_mut().is_none_or(|remaining| {
        if units.len() <= *remaining {
            *remaining -= units.len();
            true
        } else {
            false
        }
    });
    if !units.is_empty() && !read_lookup {
        let (screen_page, revisions) = screen_revisions.ok_or(AppError::Worker)?;
        page = screen_page;
        lookup_truncated = true;
        for target in &units {
            stale |= revisions.get(&target.unit) != Some(&target.revision);
        }
    } else if !units.is_empty() {
        (page, stale, lookup, lookup_truncated) = unit_evidence(db, &units, source, limit)?;
    }
    let belongs: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM source_pages WHERE id=?1 AND source_id=?2)",
        params![page, source],
        |r| r.get(0),
    )?;
    if !belongs {
        return Err(AppError::SourceNotFound);
    }
    let mut target_label = units
        .iter()
        .take(8)
        .map(|t| format!("unit {} (revision {})", t.unit, t.revision))
        .collect::<Vec<_>>()
        .join(", ");
    if units.len() > 8 {
        write!(target_label, ", and {} more targets", units.len() - 8)
            .expect("writing to a String cannot fail");
    }
    Ok(Advice {
        id,
        page,
        kind,
        origin,
        rationale,
        report,
        target,
        suggested,
        lookup,
        state: advice_state(applied, dismissed, stale).into(),
        difference: difference("", "", None),
        target_label,
        cursor: 0,
        lookup_truncated,
        suggested_truncated,
        anchor: format!("suggestion-{id}"),
        target_links: Vec::new(),
        compact: false,
        visibility: RowVisibility::Visible,
        compact_targets: units
            .iter()
            .take(8)
            .map(|unit| unit.unit.to_string())
            .collect::<Vec<_>>()
            .join(","),
    })
}

fn advice_state(applied: bool, dismissed: bool, stale: bool) -> &'static str {
    if applied {
        "applied"
    } else if dismissed {
        "dismissed"
    } else if stale {
        "stale"
    } else {
        "open"
    }
}

fn unit_evidence(
    db: &Connection,
    units: &[super::SuggestionUnit],
    source: i64,
    limit: usize,
) -> Result<(i64, bool, String, bool), AppError> {
    // SQL computes offsets before substr; only the aggregate prefix crosses
    // the allocation boundary. Screen callers charge every visited target row.
    let lookup_column = text(LOOKUP_TEXT, 262_144);
    let sql = format!(
        "WITH wanted AS (SELECT CAST(key AS INTEGER) position,json_extract(value,'$.unit') unit,json_extract(value,'$.revision') expected FROM json_each(?1)), evidence AS (SELECT w.position,u.page_id,u.revision!=w.expected stale,{lookup_column} value FROM wanted w JOIN source_units u ON u.id=w.unit JOIN source_pages p ON p.id=u.page_id LEFT JOIN source_search_text t ON t.unit_id=u.id WHERE p.source_id=?2), measured AS (SELECT *,coalesce(sum(length(value)+1) OVER (ORDER BY position ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING),0) offset FROM evidence) SELECT max(page_id),max(stale),coalesce(group_concat(CASE WHEN offset<?3 THEN substr(value,1,?3-offset) END,char(10)),''),coalesce(sum(length(value)+1)-1,0)>?3,count(*),sum(value IS NULL) FROM measured"
    );
    let (page, stale, value, truncated, count, invalid): (i64, bool, String, bool, usize, usize) =
        db.query_row(
            &sql,
            params![serde_json::to_string(units)?, source, limit],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    read_text(r, 2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )?;
    if count != units.len() || invalid > 0 {
        return Err(AppError::Invalid("Invalid suggestion target evidence."));
    }
    Ok((page, stale, value, truncated))
}

pub(crate) struct Overlay {
    pub unit: i64,
    pub cursor: i64,
    pub points: String,
    pub suggestion: i64,
}
pub(crate) struct Card {
    pub unit: i64,
    pub cursor: i64,
    pub lookup: String,
    pub advice: Vec<Advice>,
    pub next: Option<i64>,
    pub preview: bool,
    pub form_omitted: bool,
}
pub(crate) enum ScreenReads {
    Complete,
    Deadline,
}
pub(crate) struct Screen {
    pub reads: ScreenReads,
    pub geometry: bool,
    pub width: f64,
    pub height: f64,
    pub overlay: Vec<Overlay>,
    pub overlay_omitted: bool,
    pub regions_omitted: bool,
    pub cards: Vec<Card>,
    pub notes: Vec<Advice>,
    pub notes_next: Option<i64>,
    pub next_unit: Option<(i64, i64)>,
    pub next_page: Option<i64>,
    pub primary: super::PrimaryPageState,
}
impl Screen {
    pub(crate) fn reads_reduced(&self) -> bool {
        matches!(self.reads, ScreenReads::Deadline)
    }
}
fn points(polygon: &[[f64; 2]; 4]) -> String {
    polygon
        .iter()
        .map(|[x, y]| format!("{x},{y}"))
        .collect::<Vec<_>>()
        .join(" ")
}
fn advice_ids(db: &Connection, page: i64, unit: i64, after: i64) -> Result<Vec<i64>, AppError> {
    let mut q=db.prepare("SELECT s.id FROM source_suggestions s JOIN source_pages p ON p.source_id=s.source_id WHERE p.id=?1 AND s.id>?3 AND ((?2>0 AND (json_extract(s.targets,'$.target.unit')=?2 OR EXISTS(SELECT 1 FROM json_each(s.targets,'$.targets') t WHERE json_extract(t.value,'$.unit')=?2))) OR (?2=0 AND json_extract(s.targets,'$.page')=?1)) ORDER BY s.id LIMIT 21")?;
    Ok(q.query_map(params![page, unit, after], |r| r.get(0))?
        .collect::<Result<_, _>>()?)
}
fn advice_list(
    db: &Connection,
    page: i64,
    unit: i64,
    after: i64,
) -> Result<(Vec<Advice>, Option<i64>), AppError> {
    let mut ids = advice_ids(db, page, unit, after)?;
    let more = ids.len() > 20;
    ids.truncate(20);
    let next = more.then(|| *ids.last().expect("twenty advice ids"));
    let mut rows = Vec::new();
    for mut row in load_many(db, &ids, true)? {
        row.anchor = format!("suggestion-{}-unit-{unit}", row.id);
        rows.push(row);
    }
    Ok((rows, next))
}

fn screen_advice(
    db: &Connection,
    page: i64,
    units: &[i64],
    after: i64,
) -> Result<ScreenAdvice, AppError> {
    // Target envelopes guarantee that a merge's units share a page. Resolve its
    // first unit through the primary-key index before expanding its JSON list.
    let mut selected = units.to_vec();
    selected.push(0);
    let mut query = db.prepare("WITH page_suggestions AS MATERIALIZED (SELECT s.id,s.targets FROM source_suggestions s WHERE s.source_id=(SELECT source_id FROM source_pages WHERE id=?1) AND (json_extract(s.targets,'$.page')=?1 OR EXISTS(SELECT 1 FROM source_units u WHERE u.id=coalesce(json_extract(s.targets,'$.target.unit'),json_extract(s.targets,'$.targets[0].unit')) AND u.page_id=?1))), expanded AS (SELECT s.id,coalesce(json_extract(t.value,'$.unit'),json_extract(s.targets,'$.target.unit'),0) unit,json_extract(s.targets,'$.page') page FROM page_suggestions s LEFT JOIN json_each(s.targets,'$.targets') t), ranked AS (SELECT id,unit,row_number() OVER (PARTITION BY unit ORDER BY id) ordinal FROM expanded WHERE unit IN (SELECT value FROM json_each(?2)) AND (unit>0 OR (page=?1 AND id>?3))) SELECT unit,id FROM ranked WHERE ordinal<=21 ORDER BY unit,id")?;
    let ids = query
        .query_map(
            params![page, serde_json::to_string(&selected)?, after],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let unique = ids
        .iter()
        .map(|(_, id)| *id)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let cache = load_many_with_budget(db, &unique, true, Some(page))?
        .into_iter()
        .map(|row| (row.id, row))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut seen = std::collections::BTreeSet::new();
    let mut rows = std::collections::BTreeMap::new();
    // The input is display order, which can differ from unit-id order for owners.
    for (unit, id) in selected
        .iter()
        .flat_map(|unit| ids.iter().filter(move |(u, _)| u == unit))
        .copied()
    {
        let entry = rows.entry(unit).or_insert_with(|| (Vec::new(), None));
        if entry.0.len() == 20 {
            entry.1 = entry.0.last().map(|row: &Advice| row.id);
            continue;
        }
        let compact = !seen.insert(id);
        let mut row = cache[&id].clone();
        row.compact = compact;
        if compact {
            row.lookup.clear();
            row.suggested.clear();
            row.rationale.clear();
            if let SuggestionTarget::Units { targets } = &mut row.target {
                targets.clear();
            }
        }
        row.anchor = format!("suggestion-{id}-unit-{unit}");
        entry.0.push(row);
    }
    Ok(rows)
}

fn compare_screen(screen: &mut Screen, mut budget: Budget) {
    for card in &mut screen.cards {
        for row in &mut card.advice {
            compare_card(row, &mut budget);
        }
    }
    for row in &mut screen.notes {
        compare_card(row, &mut budget);
    }
}

// Fixed markup, both CSRF and origin copies, target labels, identifiers and
// controls. Rationale and comparison text are charged separately.
const ADVICE_ROW_BYTES: usize = 8 * 1024;

fn reserve_rows(rows: &mut [Advice], next: &mut Option<i64>, budget: &mut Budget) {
    for index in 0..rows.len() {
        if !budget.reserve(ADVICE_ROW_BYTES) {
            *next = Some(if index == 0 {
                rows[0].id - 1
            } else {
                rows[index - 1].id
            });
            for row in &mut rows[index..] {
                row.visibility = RowVisibility::Omitted;
            }
            break;
        }
    }
}

fn compare_card(row: &mut Advice, budget: &mut Budget) {
    if row.row_omitted() {
        row.lookup.clear();
        row.suggested.clear();
        row.rationale.clear();
        return;
    }
    row.rationale = budget.retain(&row.rationale.chars().take(200).collect::<String>(), 6);
    row.difference = if row.compact {
        difference("", "", None)
    } else if row.lookup_truncated || row.suggested_truncated || !budget.comparison_available() {
        let mut result = difference(&row.lookup, &row.suggested, None);
        result.old = budget.retain(&result.old, 6);
        result.new = budget.retain(&result.new, 6);
        result
    } else {
        budget.difference(&row.lookup, &row.suggested)
    };
    row.lookup.clear();
    row.suggested.clear();
}

impl Store {
    pub(crate) async fn material_screen(
        &self,
        auth: RequestAuth,
        page: i64,
        units: Vec<i64>,
        notes_after: i64,
    ) -> Result<Screen, AppError> {
        if notes_after < 0 {
            return Err(AppError::Invalid("Cursors must be nonnegative."));
        }
        let auth_for_recheck = auth.clone();
        let (mut screen, budget) = self.run_bounded_auth(move |db| {
            let tx=db.transaction()?; auth.recheck_owner(&tx)?;
            let result = read_with_deadline(&tx, "screen", || {
            let bounds = match super::documents::polygon_bounds(&tx,page) {
                Ok((width,height)) if width.is_finite() && height.is_finite() && (1.0..=4000.0).contains(&width) && (1.0..=4000.0).contains(&height) => Some((width,height)),
                Ok(_) | Err(AppError::Invalid("Page geometry is missing or invalid.")) => None,
                Err(error) => return Err(error),
            };
            let geometry = bounds.is_some();
            let (width,height) = bounds.unwrap_or_default();
            let (document,pdf):(Option<i64>,i64)=tx.query_row("SELECT m.document_id,p.pdf_page FROM source_pages p LEFT JOIN source_document_members m ON m.source_id=p.source_id WHERE p.id=?1",[page],|r|Ok((r.get(0)?,r.get(1)?)))?;
            let document=document.unwrap_or(0);
            let primary=super::documents::primary_state(&tx,document,pdf)?;
            let count:i64=tx.query_row("SELECT count(*) FROM source_units WHERE page_id=?1",[page],|r|r.get(0))?;
            let mut overlay=Vec::new();
            if geometry && count<=2000 {
                let mut q=tx.prepare(&format!("{} SELECT id,polygon,position-1 FROM ordered ORDER BY position LIMIT 2000",super::materials::ORDERED_UNITS))?;
                for item in q.query_map([page],|r|Ok((r.get::<_,i64>(0)?,read_text(r,1)?,r.get(2)?)))? {
                    let (unit,json,cursor)=item?;
                    if let Ok(polygon)=serde_json::from_str::<[[f64;2];4]>(&json) { overlay.push(Overlay{unit,cursor,points:points(&polygon),suggestion:0}); }
                }
            }
            let mut budget=Budget::default();
            let mut advice=screen_advice(&tx,page,&units,notes_after)?;
             // Screen structure, unit controls and bounded overlay data.
             budget.reserve(512 * 1024);
             // Reserve the first retained form and suggestion markup before any
             // optional lookup text or additional correction controls consume space.
             budget.reserve(1280 * 1024);
             // Notes have no separate list route: protect their cursor page first.
             for unit in std::iter::once(0).chain(units.iter().copied()) {
                 if let Some((rows,next)) = advice.get_mut(&unit) {
                     reserve_rows(rows,next,&mut budget);
                 }
             }
            let mut cards=Vec::new();
            let cursors = {
                let mut query = tx.prepare(&format!("{} SELECT id,position-1 FROM ordered WHERE id IN (SELECT value FROM json_each(?2))",super::materials::ORDERED_UNITS))?;
                query.query_map(params![page,serde_json::to_string(&units)?], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?)))?.collect::<Result<std::collections::BTreeMap<_,_>,_>>()?
            };
            for unit in units {
                let (lookup,correction_bytes):(String,usize)=tx.query_row(&format!("SELECT {},length(CAST(u.correction AS BLOB)) FROM source_units u LEFT JOIN source_search_text t ON t.unit_id=u.id WHERE u.id=?1 AND u.page_id=?2",text(LOOKUP_TEXT,262_144)),params![unit,page],|r|Ok((read_text(r,0)?,r.get(1)?)))?;
                // A failed unit form is reloaded as the first card. Reserve the
                // complete escaped POST bound there so an invalid draft fits intact.
                // Form-urlencoded escapes the HTML-special ASCII bytes as three
                 // bytes; Askama needs at most six. State and revision each
                 // render twice (revision's label is truncated). Four times the
                 // wire bound covers every submitted field at two copies.
                 let form_omitted = !cards.is_empty() && !budget.reserve(correction_bytes.saturating_mul(6));
                let retained=budget.retain(&lookup,12);
                let preview=retained.len()!=lookup.len();
                let (advice,next)=advice.remove(&unit).unwrap_or_default();
                let cursor=*cursors.get(&unit).ok_or(AppError::SourceNotFound)?;
                cards.push(Card{unit,cursor,lookup:retained,advice,next,preview,form_omitted});
            }
            let (notes,notes_next)=advice.remove(&0).unwrap_or_default();
            let mut regions_omitted = false;
            if geometry {
                let mut query = tx.prepare(&format!("SELECT id,{} FROM source_suggestions WHERE source_id=(SELECT source_id FROM source_pages WHERE id=?1) AND kind='missing_text' AND json_extract(targets,'$.page')=?1 ORDER BY id LIMIT 2001",text("targets",65536)))?;
                for (index, row) in query.query_map([page], |r| Ok((r.get::<_,i64>(0)?,read_text(r,1)?)))?.enumerate() {
                    let (id,json) = row?;
                    if index == 2000 { regions_omitted = true; break; }
                    if let SuggestionTarget::Region { polygon, .. }=serde_json::from_str(&json)? { overlay.push(Overlay{unit:0,cursor:0,points:points(&polygon),suggestion:id}); }
                }
            }
            let next_unit=tx.query_row(&format!("{} SELECT id,position-1 FROM ordered WHERE state='unreviewed' ORDER BY position LIMIT 1",super::materials::ORDERED_UNITS),[page],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            let next_page=tx.query_row("SELECT p.id FROM source_pages p WHERE p.source_id=(SELECT source_id FROM source_pages WHERE id=?1) AND p.pdf_page>(SELECT pdf_page FROM source_pages WHERE id=?1) AND p.complete=0 ORDER BY p.pdf_page LIMIT 1",[page],|r|r.get(0)).optional()?;
            Ok((Screen{reads:ScreenReads::Complete,geometry,width,height,overlay,overlay_omitted:count>2000,regions_omitted,cards,notes,notes_next,next_unit,next_page,primary},budget))
             });
            // Drop the progress handler before authority checks and the reduced
            // response. Only reads run under the handler; no interrupted writes.
            let result = match result {
                 Err(AppError::Invalid(READ_TIMEOUT)) => read_with_deadline(&tx, "reduced", || {
                    let (document,pdf):(i64,i64)=tx.query_row("SELECT coalesce(m.document_id,0),p.pdf_page FROM source_pages p LEFT JOIN source_document_members m ON m.source_id=p.source_id WHERE p.id=?1",[page],|r|Ok((r.get(0)?,r.get(1)?)))?;
                    let primary=super::documents::primary_state(&tx,document,pdf)?;
                    Ok((Screen{reads:ScreenReads::Deadline,geometry:false,width:0.0,height:0.0,overlay:Vec::new(),overlay_omitted:true,regions_omitted:true,cards:Vec::new(),notes:Vec::new(),notes_next:None,next_unit:None,next_page:None,primary},Budget::default()))
                 }),
                other => other,
            }?;
            auth.recheck_owner(&tx)?; tx.commit()?;
            Ok(result)
        }).await?;
        screen = tokio::task::spawn_blocking(move || {
            compare_screen(&mut screen, budget);
            screen
        })
        .await
        .map_err(|_| AppError::Worker)?;
        self.material_recheck(auth_for_recheck).await?;
        Ok(screen)
    }
    pub(crate) async fn material_advice_list(
        &self,
        auth: RequestAuth,
        page: i64,
        unit: i64,
        after: i64,
    ) -> Result<(Vec<Advice>, Option<i64>), AppError> {
        if after < 0 {
            return Err(AppError::Invalid("Cursors must be nonnegative."));
        }
        let auth_for_recheck = auth.clone();
        let (mut rows, next) = self
            .run_bounded_auth(move |db| {
                let tx = db.transaction()?;
                auth.recheck_owner(&tx)?;
                let rows = read_with_deadline(&tx, "list", || advice_list(&tx, page, unit, after))?;
                auth.recheck_owner(&tx)?;
                tx.commit()?;
                Ok(rows)
            })
            .await?;
        rows = tokio::task::spawn_blocking(move || {
            let mut budget = Budget::default();
            budget.reserve(128 * 1024);
            budget.reserve(rows.len() * ADVICE_ROW_BYTES);
            for row in &mut rows {
                compare_card(row, &mut budget);
            }
            rows
        })
        .await
        .map_err(|_| AppError::Worker)?;
        self.material_recheck(auth_for_recheck).await?;
        Ok((rows, next))
    }
}
impl Store {
    pub(crate) async fn material_advice(
        &self,
        auth: RequestAuth,
        id: i64,
    ) -> Result<Advice, AppError> {
        self.run_bounded_auth(move |db| {
             let tx = db.transaction()?;
             auth.recheck_owner(&tx)?;
             let advice = read_with_deadline(&tx, "detail", || {
             let mut advice = load(&tx, id, false)?;
            {
                let mut query = tx.prepare(&format!("{} SELECT id,position-1 FROM ordered WHERE id IN (SELECT json_extract(value,'$.unit') FROM json_each((SELECT targets FROM source_suggestions WHERE id=?2),'$.targets') UNION SELECT json_extract(targets,'$.target.unit') FROM source_suggestions WHERE id=?2) ORDER BY position", super::materials::ORDERED_UNITS))?;
                advice.target_links = query.query_map(params![advice.page,id], |r| Ok((r.get(0)?,r.get(1)?)))?.collect::<Result<_,_>>()?;
            }
            if let Some(unit) = targets(&advice.target).first() {
                advice.cursor = tx.query_row(
                    &format!(
                        "{} SELECT position-1 FROM ordered WHERE id=?2",
                        super::materials::ORDERED_UNITS
                    ),
                    params![advice.page, unit.unit],
                    |r| r.get(0),
                )?;
            }
            // The HTTP renderer reserves full texts and the submitted draft
             // before deciding whether a comparison fits, outside this mutex.
             Ok(advice)
             })?;
             auth.recheck_owner(&tx)?;
            tx.commit()?;
            Ok(advice)
        })
        .await
    }
    pub(crate) async fn material_accept(
        &self,
        auth: RequestAuth,
        csrf: CsrfAdmission,
        id: i64,
        rationale: String,
        edit: Option<String>,
    ) -> Result<String, AppError> {
        self.run_bounded_auth(move |db| {
            with_domain_request(auth.clone(), || {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                csrf.recheck_owner_write(&tx, &auth)?;
                let mut advice = load(&tx, id, false)?;
                super::suggestions::check_open(&tx, id)?;
                if advice.state == "stale" {
                    return Err(AppError::Conflict);
                }
                if advice.kind == "note" {
                    return Err(AppError::Invalid("Notes cannot be accepted."));
                }
                let actor = format!("account:{}", auth.account_id());
                let chosen = edit.as_ref().unwrap_or(&advice.suggested).clone();
                let edited = chosen != advice.suggested;
                let mut app = SuggestionApplication {
                    suggestion_id: id,
                    event_id: 0,
                    operation_id: None,
                    owner_unit_id: None,
                    edited,
                };
                match &advice.target {
                    SuggestionTarget::Unit { target } => {
                        if advice.kind == "reject" && edit.is_some() {
                            return Err(AppError::Invalid("Rejection has no editable text."));
                        }
                        app.event_id = apply_unit(&tx, &advice, target, chosen, actor, rationale)?;
                        audit(&tx, &auth, app.event_id)?;
                    }
                    SuggestionTarget::Region { .. } | SuggestionTarget::Units { .. } => {
                        let operation = matches!(advice.target, SuggestionTarget::Units { .. })
                            .then(super::documents::new_operation_id);
                        let polygon = if operation.is_some() {
                            merge_rectangle(&tx, id)?
                        } else if let SuggestionTarget::Region { polygon, .. } = &advice.target {
                            *polygon
                        } else {
                            unreachable!()
                        };
                        let revision = tx.query_row(
                            "SELECT revision FROM source_pages WHERE id=?1",
                            [advice.page],
                            |r| r.get(0),
                        )?;
                        let unit = OwnerUnit {
                            revision,
                            polygon,
                            text: chosen,
                            actor: actor.clone(),
                            rationale: rationale.clone(),
                        };
                        super::documents::validate_owner(&unit)?;
                        let event = super::documents::add_owner_operation(
                            &tx,
                            advice.page,
                            &unit,
                            operation.as_deref(),
                        )?;
                        audit(&tx, &auth, event)?;
                        app.owner_unit_id = Some(tx.query_row(
                            "SELECT unit_id FROM source_review_events WHERE id=?1",
                            [event],
                            |r| r.get(0),
                        )?);
                        app.event_id = event;
                        for target in targets(&advice.target) {
                            let review = UnitReview {
                                revision: target.revision,
                                state: "rejected".into(),
                                correction: String::new(),
                                actor: actor.clone(),
                                rationale: rationale.clone(),
                            };
                            app.event_id = super::store::review_unit_operation(
                                &tx,
                                advice.page,
                                target.unit,
                                &review,
                                operation.as_deref(),
                            )?;
                            audit(&tx, &auth, app.event_id)?;
                        }
                        app.operation_id = operation;
                    }
                    SuggestionTarget::Page { .. } => {
                        return Err(AppError::Invalid("Notes cannot be accepted."));
                    }
                }
                super::suggestions::record_application(&tx, &app)?;
                let back_link = advice.write_back_link(&tx)?;
                csrf.recheck_owner_write(&tx, &auth)?;
                tx.commit()?;
                Ok(back_link)
            })
        })
        .await
    }
    pub(crate) async fn material_dismiss(
        &self,
        auth: RequestAuth,
        csrf: CsrfAdmission,
        id: i64,
        reason: String,
    ) -> Result<String, AppError> {
        self.run_bounded_auth(move |db| with_domain_request(auth.clone(), || {
            let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            csrf.recheck_owner_write(&tx, &auth)?;
            if reason.chars().count()>5000 || reason.contains('\0') { return Err(AppError::Invalid("Dismissal reason exceeds 5,000 characters or contains NUL.")); }
            let mut advice=load(&tx,id,false)?; super::suggestions::check_open(&tx,id)?;
            tx.execute("INSERT INTO source_suggestion_dismissals(suggestion_id,actor,reason) VALUES(?1,?2,?3)",params![id,format!("account:{}",auth.account_id()),(!reason.trim().is_empty()).then_some(reason)])?;
            let event=tx.last_insert_rowid();
            tx.execute("INSERT INTO source_suggestion_web_events(event_id,actor_id,actor_epoch,session_digest) VALUES(?1,?2,?3,?4)",params![event,auth.account_id(),auth.epoch(),auth.digest()])?;
            let back_link = advice.write_back_link(&tx)?;
            csrf.recheck_owner_write(&tx, &auth)?; tx.commit()?; Ok(back_link)
        })).await
    }
}

fn apply_unit(
    tx: &Transaction<'_>,
    advice: &Advice,
    target: &super::SuggestionUnit,
    correction: String,
    actor: String,
    rationale: String,
) -> Result<i64, AppError> {
    let review = UnitReview {
        revision: target.revision,
        state: if advice.kind == "reject" {
            "rejected"
        } else {
            "corrected"
        }
        .into(),
        correction,
        actor,
        rationale,
    };
    super::store::validate_unit(&review)?;
    super::store::review_unit(tx, advice.page, target.unit, &review)
}

// Read SQLite's binary values directly. Deserializing polygon JSON through serde_json
// without float_roundtrip can differ by an ulp from json_extract, which the trigger compares exactly.
fn merge_rectangle(db: &Connection, id: i64) -> Result<[[f64; 2]; 4], AppError> {
    let (x,y,right,bottom):(f64,f64,f64,f64)=db.query_row("SELECT min(json_extract(p.value,'$[0]')),min(json_extract(p.value,'$[1]')),max(json_extract(p.value,'$[0]')),max(json_extract(p.value,'$[1]')) FROM source_suggestions s,json_each(s.targets,'$.targets') t JOIN source_units u ON u.id=json_extract(t.value,'$.unit'),json_each(u.polygon) p WHERE s.id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    if x >= right || y >= bottom {
        return Err(AppError::Invalid("Merge rectangle has zero area."));
    }
    Ok([[x, y], [right, y], [right, bottom], [x, bottom]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use askama::Template;

    #[derive(Template)]
    #[template(path = "material_difference.html")]
    struct Render {
        advice: TestAdvice,
    }
    struct TestAdvice {
        difference: Difference,
    }

    #[derive(Template)]
    #[template(path = "material_advice.html")]
    struct RowRender {
        advice: Advice,
        csrf: String,
    }

    #[test]
    fn maximum_suggestion_row_markup_fits_fixed_charge() {
        let max = i64::MAX;
        let target_label = (0..8)
            .map(|_| format!("unit {max} (revision {max})"))
            .collect::<Vec<_>>()
            .join(", ")
            + ", and 1016 more targets";
        let row = Advice {
            id: max,
            page: max,
            kind: "missing_text".into(),
            origin: "\"".repeat(100),
            rationale: String::new(),
            report: Some(max),
            target: SuggestionTarget::Unit {
                target: super::super::SuggestionUnit {
                    unit: max,
                    revision: max,
                },
            },
            suggested: String::new(),
            lookup: String::new(),
            state: "open".into(),
            difference: difference("", "", None),
            target_label,
            cursor: max,
            lookup_truncated: true,
            suggested_truncated: true,
            anchor: format!("suggestion-{max}-unit-{max}"),
            target_links: Vec::new(),
            compact: false,
            visibility: RowVisibility::Visible,
            compact_targets: std::iter::repeat_n(max.to_string(), 8)
                .collect::<Vec<_>>()
                .join(","),
        };
        let rendered = RowRender {
            advice: row,
            csrf: "a".repeat(64),
        }
        .render()
        .unwrap();
        eprintln!(
            "maximum-width suggestion fixed markup: {} bytes; allowance {ADVICE_ROW_BYTES}",
            rendered.len()
        );
        assert!(rendered.len() <= ADVICE_ROW_BYTES);
    }

    #[test]
    fn exhausted_target_budget_marks_off_page_target_stale() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE source_pages(id INTEGER PRIMARY KEY,source_id INTEGER); INSERT INTO source_pages VALUES(1,1);").unwrap();
        let revisions = [(1, 1)].into_iter().collect();
        let stored = StoredAdvice {
            id: 1,
            kind: "merge".into(),
            origin: "fixture".into(),
            rationale: "Checked".into(),
            json: serde_json::to_string(&SuggestionTarget::Units {
                targets: vec![
                    super::super::SuggestionUnit {
                        unit: 1,
                        revision: 1,
                    },
                    super::super::SuggestionUnit {
                        unit: 2,
                        revision: 1,
                    },
                ],
            })
            .unwrap(),
            suggested: "Combined".into(),
            report: None,
            applied: false,
            dismissed: false,
            source: 1,
            suggested_truncated: false,
        };
        let advice = load_evidence(&db, stored, true, &mut Some(0), Some((1, &revisions)))
            .expect("one invalid target must not fail the screen");
        assert_eq!(advice.state, "stale");
        assert!(!advice.actionable());
        assert!(advice.lookup_truncated);
    }

    #[test]
    fn exhausted_screen_target_budget_reads_revisions_without_lookup_storage() {
        let db = Connection::open_in_memory().unwrap();
        // Deliberately no lookup columns/table: revision-only advice must not
        // access them, but must still detect a stale second merge target.
        db.execute_batch("CREATE TABLE source_pages(id INTEGER PRIMARY KEY,source_id INTEGER); INSERT INTO source_pages VALUES(1,1);").unwrap();
        let revisions = [(1, 1), (2, 2)].into_iter().collect();
        let target = SuggestionTarget::Units {
            targets: vec![
                super::super::SuggestionUnit {
                    unit: 1,
                    revision: 1,
                },
                super::super::SuggestionUnit {
                    unit: 2,
                    revision: 1,
                },
            ],
        };
        let stored = StoredAdvice {
            id: 1,
            kind: "merge".into(),
            origin: "tool:fixture".into(),
            rationale: "Checked".into(),
            json: serde_json::to_string(&target).unwrap(),
            suggested: "Combined".into(),
            report: None,
            applied: false,
            dismissed: false,
            source: 1,
            suggested_truncated: false,
        };
        let advice = load_evidence(&db, stored, true, &mut Some(0), Some((1, &revisions))).unwrap();
        assert_eq!(advice.state, "stale");
        assert!(advice.lookup_truncated);
        assert_eq!(advice.lookup, "");
        assert!(!advice.suggested_truncated);
    }

    #[test]
    fn unicode_word_differences_escape_html_and_entities() {
        let result = Budget::default().difference("città <script> &lt;", "città <img> &amp;");
        assert!(!result.fallback);
        for (excluded, expected) in [
            ("insert", "città <script> &lt;"),
            ("delete", "città <img> &amp;"),
        ] {
            assert_eq!(
                result
                    .segments
                    .iter()
                    .filter(|s| s.kind != excluded)
                    .map(|s| s.text.as_str())
                    .collect::<String>(),
                expected,
                "difference segments must preserve exact input text"
            );
        }
        assert!(
            result
                .segments
                .iter()
                .any(|s| s.kind == "equal" && s.text.contains("città"))
        );
        let rendered = Render {
            advice: TestAdvice { difference: result },
        }
        .render()
        .unwrap();
        assert!(!rendered.contains("<script>"));
        assert!(!rendered.contains("<img>"));
        assert!(rendered.contains("<ins>"));
        assert!(rendered.contains("<del>"));
        let parsed = scraper::Html::parse_fragment(&rendered);
        assert!(
            parsed
                .select(&scraper::Selector::parse("*").unwrap())
                .all(|n| matches!(n.value().name(), "html" | "div" | "ins" | "del"))
        );
    }
    #[test]
    fn lookup_budget_fallback_is_a_two_thousand_character_preview() {
        let mut budget = Budget::default();
        assert!(budget.reserve(2 * 1024 * 1024 - 100_000));
        let preview = budget.retain(&"🦀".repeat(20000), 2);
        assert_eq!(preview.chars().count(), 2000);
        assert!(budget.bytes <= 2 * 1024 * 1024);
    }
    #[test]
    fn repetitive_input_deadline_and_response_budget_fall_back() {
        let old = "a ".repeat(5000) + &"b ".repeat(5000);
        let new = "b ".repeat(5000) + &"a ".repeat(5000);
        let result = difference(&old, &new, Some(Instant::now()));
        assert!(result.fallback);
        assert_eq!(result.old.chars().count(), 2000);
        let mut expired = Budget {
            deadline: Some(Instant::now()),
            ..Budget::default()
        };
        let result = expired.difference(&old, &new);
        assert!(
            result.fallback,
            "an exhausted comparison deadline must deterministically fall back"
        );
        assert_eq!(
            expired.count, 0,
            "expired work must not enter the comparison engine"
        );
        let mut budget = Budget::default();
        for _ in 0..100 {
            assert!(!budget.difference("old", "new").fallback);
        }
        assert!(budget.difference("old", "new").fallback);
        let mut budget = Budget::default();
        for _ in 0..1000 {
            budget.difference(&old, &new);
        }
        assert!(budget.bytes <= 2 * 1024 * 1024);
        assert!(budget.count <= 100);
    }
}
