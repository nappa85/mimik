//! Bounded review advice reads and authenticated, atomic application of stored advice.
use super::{
    OwnerUnit, SuggestionApplication, SuggestionTarget, UnitReview,
    materials::{read_text, text},
};
use crate::{
    AppError, Store,
    auth::{CsrfAdmission, RequestAuth, with_domain_request},
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::time::{Duration, Instant};

pub(crate) struct Segment {
    pub kind: &'static str,
    pub text: String,
}
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
        let size = old.len().saturating_add(new.len());
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
            result.old = self.retain(&result.old, 1);
            result.new = self.retain(&result.new, 1);
        } else {
            self.bytes += size;
        }
        result
    }
}
fn difference(old: &str, new: &str, deadline: Option<Instant>) -> Difference {
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
    result.segments = diff
        .iter_all_changes()
        .map(|change| Segment {
            kind: match change.tag() {
                similar::ChangeTag::Delete => "delete",
                similar::ChangeTag::Insert => "insert",
                similar::ChangeTag::Equal => "equal",
            },
            text: change.value().to_owned(),
        })
        .collect();
    result.old.clear();
    result.new.clear();
    result.fallback = false;
    result
}

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
    pub anchor: String,
    pub target_links: Vec<(i64, i64)>,
}
impl Advice {
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
    let suggested_column = text("coalesce(suggested_text,'')", 80000);
    let suggested_column = if preview {
        format!("substr({suggested_column},1,2000)")
    } else {
        suggested_column
    };
    let sql = format!(
        "SELECT {},{},{},{},{},report_id,EXISTS(SELECT 1 FROM source_suggestion_applications WHERE suggestion_id=s.id),EXISTS(SELECT 1 FROM source_suggestion_dismissals WHERE suggestion_id=s.id),source_id FROM source_suggestions s WHERE id=?1",
        text("kind", 100),
        text("origin", 400),
        text("rationale", 20000),
        text("targets", 65536),
        suggested_column
    );
    let (kind, origin, rationale, json, suggested, report, applied, dismissed, source): (
        String,
        String,
        String,
        String,
        String,
        Option<i64>,
        bool,
        bool,
        i64,
    ) = db
        .query_row(&sql, [id], |r| {
            Ok((
                read_text(r, 0)?,
                read_text(r, 1)?,
                read_text(r, 2)?,
                read_text(r, 3)?,
                read_text(r, 4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
            ))
        })
        .optional()?
        .ok_or(AppError::SourceNotFound)?;
    let target: SuggestionTarget = serde_json::from_str(&json)?;
    let mut lookup = String::new();
    let mut stale = false;
    let mut lookup_truncated = false;
    let limit: usize = if preview { 2000 } else { 20000 };
    let mut length = 0;
    let lookup_column = text(
        "CASE u.state WHEN 'accepted' THEN coalesce(t.search_text,'') WHEN 'corrected' THEN u.correction ELSE '' END",
        262_144,
    );
    let mut page = match &target {
        SuggestionTarget::Page { page, .. } | SuggestionTarget::Region { page, .. } => *page,
        _ => 0,
    };
    for unit in targets(&target) {
        let (p, revision, value, full_length): (i64,i64,String,usize) = db.query_row(&format!("SELECT u.page_id,u.revision,substr({lookup_column},1,?3),length({lookup_column}) FROM source_units u JOIN source_pages p ON p.id=u.page_id LEFT JOIN source_search_text t ON t.unit_id=u.id WHERE u.id=?1 AND p.source_id=?2"),params![unit.unit,source,limit],|r|Ok((r.get(0)?,r.get(1)?,read_text(r,2)?,r.get(3)?)))?;
        page = p;
        stale |= revision != unit.revision;
        lookup_truncated |= length + full_length + usize::from(!lookup.is_empty()) > limit;
        // A multi-target detail remains bounded, even for 1,024 accepted units.
        if !lookup.is_empty() && length < limit {
            lookup.push('\n');
            length += 1;
        }
        let remaining = limit.saturating_sub(length);
        lookup.extend(value.chars().take(remaining));
        length += full_length.min(remaining);
    }
    let belongs: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM source_pages WHERE id=?1 AND source_id=?2)",
        params![page, source],
        |r| r.get(0),
    )?;
    if !belongs {
        return Err(AppError::SourceNotFound);
    }
    let target_label = targets(&target)
        .iter()
        .map(|t| format!("unit {} (revision {})", t.unit, t.revision))
        .collect::<Vec<_>>()
        .join(", ");
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
        anchor: format!("suggestion-{id}"),
        target_links: Vec::new(),
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
pub(crate) struct Screen {
    pub width: f64,
    pub height: f64,
    pub overlay: Vec<Overlay>,
    pub overlay_omitted: bool,
    pub cards: Vec<Card>,
    pub notes: Vec<Advice>,
    pub notes_next: Option<i64>,
    pub next_unit: Option<(i64, i64)>,
    pub next_page: Option<i64>,
    pub document: i64,
    pub primary: super::PrimaryPageState,
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
    budget: &mut Budget,
) -> Result<(Vec<Advice>, Option<i64>), AppError> {
    let mut ids = advice_ids(db, page, unit, after)?;
    let more = ids.len() > 20;
    ids.truncate(20);
    let next = more.then(|| *ids.last().expect("twenty advice ids"));
    let mut rows = Vec::new();
    for id in ids {
        let preview = !budget.comparison_available();
        let mut row = load(db, id, preview)?;
        row.anchor = format!("suggestion-{id}-unit-{unit}");
        row.difference = if preview {
            let mut result = difference(&row.lookup, &row.suggested, None);
            result.old = budget.retain(&result.old, 1);
            result.new = budget.retain(&result.new, 1);
            result
        } else {
            budget.difference(&row.lookup, &row.suggested)
        };
        row.lookup.clear();
        row.suggested.clear();
        rows.push(row);
    }
    Ok((rows, next))
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
        self.run_bounded_auth(move |db| {
            let tx=db.transaction()?; auth.recheck_owner(&tx)?;
            let (width,height)=super::documents::polygon_bounds(&tx,page).unwrap_or((1.0,1.0));
            let (document,pdf):(Option<i64>,i64)=tx.query_row("SELECT m.document_id,p.pdf_page FROM source_pages p LEFT JOIN source_document_members m ON m.source_id=p.source_id WHERE p.id=?1",[page],|r|Ok((r.get(0)?,r.get(1)?)))?;
            let document=document.unwrap_or(0);
            let primary=super::documents::primary_state(&tx,document,pdf)?;
            let count:i64=tx.query_row("SELECT count(*) FROM source_units WHERE page_id=?1",[page],|r|r.get(0))?;
            let mut overlay=Vec::new();
            if count<=2000 {
                let mut q=tx.prepare(&format!("{} SELECT id,polygon,position-1 FROM ordered ORDER BY position LIMIT 2000",super::materials::ORDERED_UNITS))?;
                for item in q.query_map([page],|r|Ok((r.get::<_,i64>(0)?,read_text(r,1)?,r.get(2)?)))? {
                    let (unit,json,cursor)=item?;
                    if let Ok(polygon)=serde_json::from_str::<[[f64;2];4]>(&json) { overlay.push(Overlay{unit,cursor,points:points(&polygon),suggestion:0}); }
                }
            }
            let mut budget=Budget::default();
            let mut cards=Vec::new();
            for unit in units {
                let (lookup,correction_bytes):(String,usize)=tx.query_row(&format!("SELECT {},length(CAST(u.correction AS BLOB)) FROM source_units u LEFT JOIN source_search_text t ON t.unit_id=u.id WHERE u.id=?1 AND u.page_id=?2",text("CASE u.state WHEN 'accepted' THEN coalesce(t.search_text,'') WHEN 'corrected' THEN u.correction ELSE '' END",262_144)),params![unit,page],|r|Ok((read_text(r,0)?,r.get(1)?)))?;
                // A failed unit form is reloaded as the first card. Reserve the
                // complete POST bound there so even an invalid draft fits intact.
                let form_omitted = !budget.reserve(if cards.is_empty() {320 * 1024} else {correction_bytes});
                let retained=budget.retain(&lookup,2);
                let preview=retained.len()!=lookup.len();
                let (advice,next)=advice_list(&tx,page,unit,0,&mut budget)?;
                let cursor=tx.query_row(&format!("{} SELECT position-1 FROM ordered WHERE id=?2",super::materials::ORDERED_UNITS),params![page,unit],|r|r.get(0))?;
                cards.push(Card{unit,cursor,lookup:retained,advice,next,preview,form_omitted});
            }
            let (notes,notes_next)=advice_list(&tx,page,0,notes_after,&mut budget)?;
            for row in &notes {
                if let SuggestionTarget::Region { polygon, .. }=&row.target { overlay.push(Overlay{unit:0,cursor:0,points:points(polygon),suggestion:row.id}); }
            }
            let next_unit=tx.query_row(&format!("{} SELECT id,position-1 FROM ordered WHERE state='unreviewed' ORDER BY position LIMIT 1",super::materials::ORDERED_UNITS),[page],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            let next_page=tx.query_row("SELECT p.id FROM source_pages p WHERE p.source_id=(SELECT source_id FROM source_pages WHERE id=?1) AND p.pdf_page>(SELECT pdf_page FROM source_pages WHERE id=?1) AND p.complete=0 ORDER BY p.pdf_page LIMIT 1",[page],|r|r.get(0)).optional()?;
            auth.recheck_owner(&tx)?; tx.commit()?;
            Ok(Screen{width,height,overlay,overlay_omitted:count>2000,cards,notes,notes_next,next_unit,next_page,document,primary})
        }).await
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
        self.run_bounded_auth(move |db| {
            let tx = db.transaction()?;
            auth.recheck_owner(&tx)?;
            let rows = advice_list(&tx, page, unit, after, &mut Budget::default())?;
            auth.recheck_owner(&tx)?;
            tx.commit()?;
            Ok(rows)
        })
        .await
    }
}
fn audit(tx: &Transaction<'_>, auth: &RequestAuth, event: i64) -> Result<(), AppError> {
    tx.execute("INSERT INTO source_web_events(event_id,actor_id,actor_epoch,session_digest) VALUES(?1,?2,?3,?4)",params![event,auth.account_id(),auth.epoch(),auth.digest()])?;
    Ok(())
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
            advice.difference = Budget::default().difference(&advice.lookup, &advice.suggested);
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
    ) -> Result<i64, AppError> {
        self.run_bounded_auth(move |db| {
            with_domain_request(auth.clone(), || {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                auth.recheck_owner(&tx)?;
                csrf.recheck(&tx)?;
                let advice = load(&tx, id, false)?;
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
                auth.recheck_owner(&tx)?;
                csrf.recheck(&tx)?;
                tx.commit()?;
                Ok(advice.page)
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
    ) -> Result<i64, AppError> {
        self.run_bounded_auth(move |db| with_domain_request(auth.clone(), || {
            let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            auth.recheck_owner(&tx)?; csrf.recheck(&tx)?;
            if reason.chars().count()>5000 || reason.contains('\0') { return Err(AppError::Invalid("Dismissal reason exceeds 5,000 characters or contains NUL.")); }
            let advice=load(&tx,id,false)?; super::suggestions::check_open(&tx,id)?;
            tx.execute("INSERT INTO source_suggestion_dismissals(suggestion_id,actor,reason) VALUES(?1,?2,?3)",params![id,format!("account:{}",auth.account_id()),(!reason.trim().is_empty()).then_some(reason)])?;
            let event=tx.last_insert_rowid();
            tx.execute("INSERT INTO source_suggestion_web_events(event_id,actor_id,actor_epoch,session_digest) VALUES(?1,?2,?3,?4)",params![event,auth.account_id(),auth.epoch(),auth.digest()])?;
            auth.recheck_owner(&tx)?; csrf.recheck(&tx)?; tx.commit()?; Ok(advice.page)
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
        let result = difference(&old, &new, Some(Instant::now() + Duration::from_millis(50)));
        assert!(
            result.fallback,
            "repetitive input must fall back at the live deadline"
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
