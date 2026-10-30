//! Document selection and owner decisions share the existing review/event machinery.
use super::{UnitReview, review_identity};
use crate::{AppError, Store};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

#[cfg(test)]
pub(crate) fn rewind_schema(db: &Connection) -> Result<(), AppError> {
    // Callers may already have selected an older migration fixture version.
    let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let review: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='source_suggestion_reports')",
        [],
        |r| r.get(0),
    )?;
    if review {
        db.execute_batch(include_str!("../../tests/support/source_review_rewind.sql"))?;
        let sql = include_str!("../../migrations/024_source_documents.sql")
            .split_once("CREATE TABLE source_suggestions (")
            .ok_or(AppError::Invalid("Missing historical suggestion schema."))?
            .1;
        let (table, triggers) = sql
            .split_once("CREATE INDEX source_suggestions_source")
            .ok_or(AppError::Invalid("Missing historical suggestion guards."))?;
        db.execute_batch(&format!("CREATE TABLE source_suggestions ({table} INSERT INTO source_suggestions SELECT * FROM legacy_suggestions; DROP TABLE legacy_suggestions; CREATE INDEX source_suggestions_source{triggers}"))?;
    }
    let present: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('source_units') WHERE name='origin')",
        [],
        |r| r.get(0),
    )?;
    if present {
        db.execute_batch(include_str!(
            "../../tests/support/source_documents_rewind.sql"
        ))?;
    }
    db.pragma_update(None, "user_version", version)?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrimaryPageState {
    pub pdf_page: i64,
    pub source_id: Option<i64>,
    /// Latest decision id also detects an intervening A → B → A selection.
    pub event_id: Option<i64>,
}

#[derive(Clone)]
pub struct OwnerUnit {
    pub revision: i64,
    pub polygon: [[f64; 2]; 4],
    pub text: String,
    pub actor: String,
    pub rationale: String,
}

#[derive(Clone)]
pub struct UnitDecision {
    pub page: i64,
    pub unit: i64,
    pub review: UnitReview,
}

pub(crate) fn register_import(db: &Connection, source: i64) -> Result<(), AppError> {
    db.execute("INSERT INTO source_documents(pdf_sha256) SELECT pdf_hash FROM sources WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM source_documents WHERE pdf_sha256=sources.pdf_hash)", [source])?;
    db.execute("INSERT INTO source_document_members SELECT s.id,d.id FROM sources s JOIN source_documents d ON d.pdf_sha256=s.pdf_hash WHERE s.id=?1", [source])?;
    db.execute("INSERT INTO source_primary_events(document_id,pdf_page,source_id,actor,rationale) SELECT m.document_id,p.pdf_page,p.source_id,'import','First imported reading' FROM source_pages p JOIN source_document_members m ON m.source_id=p.source_id WHERE p.source_id=?1 AND NOT EXISTS(SELECT 1 FROM source_primary_pages v WHERE v.document_id=m.document_id AND v.pdf_page=p.pdf_page) AND NOT EXISTS(SELECT 1 FROM source_pages o JOIN source_document_members om ON om.source_id=o.source_id WHERE om.document_id=m.document_id AND o.pdf_page=p.pdf_page AND o.source_id<>p.source_id)", [source])?;
    Ok(())
}

/// Actor recorded for decisions made through the operator command line.
pub const OPERATOR_ACTOR: &str = "operator:cli";

/// Parse a page list such as `3,5-7` into distinct ascending PDF pages (at most 1,024).
///
/// # Errors
/// Empty, malformed, non-positive, repeated or overlapping pages, or too many of them.
pub fn parse_page_list(spec: &str) -> Result<Vec<i64>, AppError> {
    const BAD: AppError =
        AppError::Invalid("Pages look like 3,5-7: positive, ascending, distinct, at most 1,024.");
    let mut pages = Vec::new();
    for part in spec.split(',') {
        let (first, last) = match part.trim().split_once('-') {
            Some((a, b)) => (a.trim(), b.trim()),
            None => (part.trim(), part.trim()),
        };
        let (first, last): (i64, i64) = (
            first.parse().map_err(|_| BAD)?,
            last.parse().map_err(|_| BAD)?,
        );
        if first <= 0
            || last < first
            || pages.last().is_some_and(|prev| *prev >= first)
            || last - first >= 1024
        {
            return Err(BAD);
        }
        pages.extend(first..=last);
        if pages.len() > 1024 {
            return Err(BAD);
        }
    }
    Ok(pages)
}

pub(super) fn primary_state(
    db: &Connection,
    document: i64,
    page: i64,
) -> Result<PrimaryPageState, AppError> {
    Ok(db.query_row("SELECT (SELECT source_id FROM source_primary_pages WHERE document_id=?1 AND pdf_page=?2),(SELECT max(id) FROM source_primary_events WHERE document_id=?1 AND pdf_page=?2)", params![document,page], |r| Ok(PrimaryPageState { pdf_page: page, source_id:r.get(0)?,event_id:r.get(1)? }))?)
}

pub(super) fn set_primary(
    tx: &Transaction<'_>,
    document: i64,
    pages: &[PrimaryPageState],
    source: i64,
    actor: &str,
    rationale: &str,
    mut sidecar: impl FnMut(&Transaction<'_>, i64) -> Result<(), AppError>,
) -> Result<Vec<i64>, AppError> {
    review_identity(actor, rationale)?;
    if pages.is_empty() || pages.len() > 1024 {
        return Err(AppError::Invalid("Choose 1–1,024 distinct pages."));
    }
    let mut seen = std::collections::BTreeSet::new();
    for page in pages {
        if page.pdf_page <= 0 || !seen.insert(page.pdf_page) {
            return Err(AppError::Invalid("Choose distinct positive PDF pages."));
        }
        let covered: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM source_document_members m JOIN source_pages p ON p.source_id=m.source_id JOIN sources s ON s.id=m.source_id JOIN source_documents d ON d.id=m.document_id AND d.pdf_sha256=s.pdf_hash JOIN source_finalizations f ON f.source_id=s.id WHERE m.document_id=?1 AND m.source_id=?2 AND p.pdf_page=?3)",params![document,source,page.pdf_page],|r|r.get(0))?;
        if !covered {
            return Err(AppError::NotFound);
        }
        if primary_state(tx, document, page.pdf_page)? != *page {
            return Err(AppError::Conflict);
        }
    }
    let mut events = Vec::with_capacity(pages.len());
    for page in pages {
        tx.execute("INSERT INTO source_primary_events(document_id,pdf_page,source_id,previous_source_id,actor,rationale) VALUES(?1,?2,?3,?4,?5,?6)",params![document,page.pdf_page,source,page.source_id,actor,rationale])?;
        let event = tx.last_insert_rowid();
        sidecar(tx, event)?;
        events.push(event);
    }
    Ok(events)
}

impl Store {
    /// Current selection tokens for every covered page, including unresolved legacy overlaps.
    ///
    /// # Errors
    /// Unknown documents or database failures.
    pub async fn source_primary_state(
        &self,
        document: i64,
    ) -> Result<Vec<PrimaryPageState>, AppError> {
        self.run(move |db| {
            let tx=db.transaction()?;
            if !tx.query_row("SELECT EXISTS(SELECT 1 FROM source_documents WHERE id=?1)",[document],|r|r.get::<_,bool>(0))? {return Err(AppError::NotFound);}
            let pages={let mut q=tx.prepare("SELECT DISTINCT p.pdf_page FROM source_pages p JOIN source_document_members m ON m.source_id=p.source_id WHERE m.document_id=?1 ORDER BY p.pdf_page")?; q.query_map([document],|r|r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?};
            let states=pages.into_iter().map(|page|primary_state(&tx,document,page)).collect::<Result<_,_>>()?;
            tx.commit()?;
            Ok(states)
        }).await
    }

    /// Select readings with expected source and event tokens for every affected page.
    ///
    /// # Errors
    /// Invalid identity/pages, stale selection, missing coverage or database failures.
    pub async fn set_primary_source(
        &self,
        document: i64,
        pages: Vec<PrimaryPageState>,
        source: i64,
        actor: String,
        rationale: String,
    ) -> Result<Vec<i64>, AppError> {
        self.run(move |db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let events = set_primary(&tx, document, &pages, source, &actor, &rationale, |_, _| {
                Ok(())
            })?;
            tx.commit()?;
            Ok(events)
        })
        .await
    }

    /// Operator recovery path for unresolved pages: select `source` for `pages` of the document
    /// with this PDF hash, against the current selection state, as actor `operator:cli`.
    ///
    /// # Errors
    /// Invalid hash/pages/rationale, unknown document, a source that does not cover a page, or
    /// database failures.
    pub async fn operator_set_primary_source(
        &self,
        pdf_sha256: String,
        pages: Vec<i64>,
        source: i64,
        rationale: String,
    ) -> Result<Vec<i64>, AppError> {
        if pdf_sha256.len() != 64
            || !pdf_sha256
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(AppError::Invalid(
                "The document PDF hash is 64 lowercase hex digits.",
            ));
        }
        self.run(move |db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let document: i64 = tx
                .query_row(
                    "SELECT id FROM source_documents WHERE pdf_sha256=?1",
                    [&pdf_sha256],
                    |r| r.get(0),
                )
                .optional()?
                .ok_or(AppError::NotFound)?;
            let states = pages
                .iter()
                .map(|page| primary_state(&tx, document, *page))
                .collect::<Result<Vec<_>, _>>()?;
            let events = set_primary(
                &tx,
                document,
                &states,
                source,
                OPERATOR_ACTOR,
                &rationale,
                |_, _| Ok(()),
            )?;
            tx.commit()?;
            Ok(events)
        })
        .await
    }

    /// Insert an owner transcription and immediately record its correction decision.
    ///
    /// # Errors
    /// Invalid text/geometry, stale page, non-finalized source or database failures.
    pub async fn add_owner_unit(&self, page: i64, unit: OwnerUnit) -> Result<i64, AppError> {
        validate_owner(&unit)?;
        self.run(move |db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let event = add_owner(&tx, page, &unit)?;
            tx.commit()?;
            Ok(event)
        })
        .await
    }

    /// Apply a bounded batch with a server-generated operation id and one rationale. The hook runs after each
    /// review event, before the next unit changes, so latest-event attribution remains valid.
    ///
    /// # Errors
    /// Invalid/duplicate decisions, stale revisions, hook errors or database failures roll back all writes.
    pub async fn review_units_atomic<F>(
        &self,
        decisions: Vec<UnitDecision>,
        rationale: String,
        sidecar: F,
    ) -> Result<Vec<i64>, AppError>
    where
        F: FnMut(&Transaction<'_>, i64) -> Result<(), AppError> + Send + 'static,
    {
        validate_batch(&decisions, &rationale)?;
        let operation_id = new_operation_id();
        self.run(move |db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let events = review_batch(&tx, &decisions, &operation_id, &rationale, sidecar)?;
            tx.commit()?;
            Ok(events)
        })
        .await
    }
}

pub(super) fn validate_owner(unit: &OwnerUnit) -> Result<(), AppError> {
    let review = owner_review(unit);
    super::store::validate_unit(&review)?;
    // Bound raw HTML after escaping, using the same 64 KiB import/display contract.
    if owner_html(&unit.text).len() > 65536 {
        return Err(AppError::Invalid("Escaped owner text exceeds 64 KiB."));
    }
    Ok(())
}

fn owner_review(unit: &OwnerUnit) -> UnitReview {
    UnitReview {
        revision: 1,
        state: "corrected".into(),
        correction: unit.text.clone(),
        actor: unit.actor.clone(),
        rationale: unit.rationale.clone(),
    }
}

fn owner_html(text: &str) -> String {
    format!(
        "<p>{}</p>",
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&#39;")
    )
}

pub(super) fn validate_polygon(
    db: &Connection,
    page: i64,
    polygon: &[[f64; 2]; 4],
) -> Result<(), AppError> {
    let bounds: Option<(Option<f64>,Option<f64>)>=db.query_row("SELECT CASE WHEN json_type(g.value,'$.width_points') IN ('integer','real') THEN json_extract(g.value,'$.width_points') END,CASE WHEN json_type(g.value,'$.height_points') IN ('integer','real') THEN json_extract(g.value,'$.height_points') END FROM source_pages p JOIN sources s ON s.id=p.source_id,json_each(CASE WHEN length(s.prepared_json)<=8388608 AND json_valid(s.prepared_json) THEN s.prepared_json ELSE '{}' END,'$.pages') g WHERE p.id=?1 AND json_extract(g.value,'$.pdf_page')=p.pdf_page",[page],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let (Some(width), Some(height)) =
        bounds.ok_or(AppError::Invalid("Page geometry is missing or invalid."))?
    else {
        return Err(AppError::Invalid("Page geometry is missing or invalid."));
    };
    let mut turns = [0.0; 4];
    for i in 0..4 {
        let [x, y] = polygon[i];
        if !x.is_finite()
            || !y.is_finite()
            || !(1.0..=4000.0).contains(&width)
            || !(1.0..=4000.0).contains(&height)
            || !(0.0..=width).contains(&x)
            || !(0.0..=height).contains(&y)
        {
            return Err(AppError::Invalid("Polygon must be inside its page."));
        }
        let b = polygon[(i + 1) % 4];
        let c = polygon[(i + 2) % 4];
        turns[i] = (b[0] - x) * (c[1] - b[1]) - (b[1] - y) * (c[0] - b[0]);
    }
    if !turns.iter().all(|t| *t > 0.0) && !turns.iter().all(|t| *t < 0.0) {
        return Err(AppError::Invalid(
            "Polygon must be nondegenerate and convex.",
        ));
    }
    Ok(())
}

pub(super) fn add_owner(
    tx: &Transaction<'_>,
    page: i64,
    unit: &OwnerUnit,
) -> Result<i64, AppError> {
    let revision = tx
        .query_row(
            "SELECT revision FROM source_pages WHERE id=?1",
            [page],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .ok_or(AppError::NotFound)?;
    if revision != unit.revision {
        return Err(AppError::Conflict);
    }
    super::store::next_revision(revision)?;
    validate_polygon(tx, page, &unit.polygon)?;
    let html = owner_html(&unit.text);
    let clean = super::sanitized(&html)?;
    let raw_text = html2text::from_read(clean.as_bytes(), 120)
        .map_err(|_| AppError::Invalid("Owner text could not be projected."))?;
    let search = super::search::search_text(&html)?;
    // SQL may have inserted a reserved ordinal ahead of the allocator; never reuse it.
    let ordinal: i64 = tx.query_row(
        "SELECT coalesce(max(CAST(substr(block_id,7) AS INTEGER)),0) FROM source_units WHERE page_id=?1 AND origin='owner'",
        [page],
        |r| r.get(0),
    )?;
    let ordinal = ordinal
        .checked_add(1)
        .ok_or(AppError::Invalid("Owner unit identities are exhausted."))?;
    tx.execute("INSERT INTO source_units(page_id,block_id,kind,polygon,raw_html,raw_text,origin) VALUES(?1,?2,'Text',?3,?4,?5,'owner')",params![page,format!("owner:{ordinal}"),serde_json::to_string(&unit.polygon)?,html,raw_text])?;
    let id = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO source_search_text(unit_id,normaliser,search_text) VALUES(?1,?2,?3)",
        params![id, super::search::SEARCH_NORMALISER, search],
    )?;
    super::store::review_unit(tx, page, id, &owner_review(unit))
}

/// A fresh 128-bit identifier naming one batch; callers cannot choose or reuse it.
pub(super) fn new_operation_id() -> String {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).expect("the operating system provides randomness");
    format!("batch-{}", hex::encode(bytes))
}

pub(super) fn validate_batch(decisions: &[UnitDecision], rationale: &str) -> Result<(), AppError> {
    review_identity("batch", rationale)?;
    if decisions.is_empty() || decisions.len() > 1024 {
        return Err(AppError::Invalid("Choose 1–1,024 unit decisions."));
    }
    let mut seen = std::collections::BTreeSet::new();
    for decision in decisions {
        if !seen.insert(decision.unit) {
            return Err(AppError::Invalid("A batch cannot decide a unit twice."));
        }
        let mut review = decision.review.clone();
        review.rationale = rationale.into();
        super::store::validate_unit(&review)?;
    }
    Ok(())
}

pub(super) fn review_batch(
    tx: &Transaction<'_>,
    decisions: &[UnitDecision],
    operation: &str,
    rationale: &str,
    mut sidecar: impl FnMut(&Transaction<'_>, i64) -> Result<(), AppError>,
) -> Result<Vec<i64>, AppError> {
    for decision in decisions {
        let current = tx
            .query_row(
                "SELECT revision FROM source_units WHERE id=?1 AND page_id=?2",
                params![decision.unit, decision.page],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .ok_or(AppError::NotFound)?;
        if current != decision.review.revision {
            return Err(AppError::Conflict);
        }
    }
    let mut events = Vec::with_capacity(decisions.len());
    for decision in decisions {
        let mut review = decision.review.clone();
        review.rationale = rationale.into();
        let event = super::store::review_unit_operation(
            tx,
            decision.page,
            decision.unit,
            &review,
            Some(operation),
        )?;
        sidecar(tx, event)?;
        events.push(event);
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_numeric_page_dimensions_are_invalid_not_internal_errors() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let polygon = [[1.0, 1.0], [20.0, 1.0], [20.0, 20.0], [1.0, 20.0]];
        for dims in [r#""wide""#, "null", "[1]", "true"] {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(&dir.path().join("dims.sqlite3")).unwrap();
            let outcome = rt.block_on(store.run(move |db| {
                db.execute_batch(&format!("INSERT INTO sources (id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages) VALUES (1,'b','s','t','mork-borg','e','it','aa',x'00','{{}}','{{\"pages\":[{{\"pdf_page\":1,\"width_points\":{dims},\"height_points\":300}}]}}',1); INSERT INTO source_pages (id,source_id,pdf_page,png) VALUES (1,1,1,x'00');"))?;
                Ok(validate_polygon(db, 1, &polygon))
            }));
            assert!(
                matches!(outcome, Ok(Err(AppError::Invalid(_)))),
                "{dims}: {outcome:?}"
            );
        }
    }
}
