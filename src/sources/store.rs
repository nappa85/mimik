use super::{
    Evidence, EvidenceSearch, PageReview, PageSummary, PageView, ReviewEvent, SourceSummary,
    UnitReview, UnitView, review_identity, sanitized,
};
use crate::{AppError, Store};
use rusqlite::{OptionalExtension, params};

impl Store {
    pub(super) async fn source_region_data(
        &self,
        page: i64,
        unit: i64,
        permit: tokio::sync::OwnedSemaphorePermit,
    ) -> Result<super::region::RegionInput, AppError> {
        self.region_data(None, page, unit, permit).await
    }

    pub(crate) async fn material_region_data(
        &self,
        auth: crate::auth::RequestAuth,
        page: i64,
        unit: i64,
        permit: tokio::sync::OwnedSemaphorePermit,
    ) -> Result<super::region::RegionInput, AppError> {
        self.region_data(Some(auth), page, unit, permit).await
    }

    async fn region_data(
        &self,
        auth: Option<crate::auth::RequestAuth>,
        page: i64,
        unit: i64,
        permit: tokio::sync::OwnedSemaphorePermit,
    ) -> Result<super::region::RegionInput, AppError> {
        self.run_bounded_auth(move |db| {
            struct RawRegion { image: Option<Vec<u8>>, prepared: Option<Vec<u8>>, pdf_page: i64, polygon: Option<String> }
            let tx = db.transaction()?;
            if let Some(auth) = &auth { auth.recheck_owner(&tx)?; }
            let limit = i64::try_from(super::region::INPUT_LIMIT).map_err(|_| AppError::Invalid("Region input limit invalid."))?;
            // CASE prevents oversized blobs being copied out of SQLite; the joined
            // statement binds all immutable artifacts to the same page/unit.
            let data = tx.query_row(
                "SELECT CASE WHEN typeof(p.png)='blob' AND length(p.png)<=?3 THEN p.png END, CASE WHEN typeof(s.prepared_json)='blob' AND length(s.prepared_json)<=?3 THEN s.prepared_json END, p.pdf_page, CASE WHEN typeof(u.polygon)='text' AND length(CAST(u.polygon AS BLOB))<=1024 THEN u.polygon END FROM source_pages p JOIN sources s ON s.id=p.source_id JOIN source_units u ON u.page_id=p.id WHERE p.id=?1 AND u.id=?2",
                params![page, unit, limit], |row| Ok(RawRegion { image: row.get(0)?, prepared: row.get(1)?, pdf_page: row.get(2)?, polygon: row.get(3)? }),
            ).optional()?;
            let data = data.ok_or(AppError::NotFound)?;
            if let Some(auth) = &auth { auth.recheck_owner(&tx)?; }
            tx.commit()?;
            Ok(super::region::RegionInput { image: data.image.ok_or(AppError::Invalid("Region image type or size invalid; inspect full page."))?, prepared: data.prepared.ok_or(AppError::Invalid("Region metadata type or size invalid; inspect full page."))?, pdf_page: data.pdf_page, polygon: data.polygon.ok_or(AppError::Invalid("Region coordinates type or byte size invalid; inspect full page."))?, permit })
        }).await
    }

    /// List registered sources and page review coverage.
    ///
    /// # Errors
    /// Returns database/worker failures.
    pub async fn sources(&self) -> Result<Vec<SourceSummary>, AppError> {
        self.run(|db| {
            let mut query = db.prepare("SELECT s.id,s.title,s.system_id,s.edition,s.pdf_hash,s.total_pages,r.game_id,r.edition_id,EXISTS(SELECT 1 FROM source_finalizations f WHERE f.source_id=s.id) FROM sources s LEFT JOIN source_rulesets r ON r.source_id=s.id ORDER BY s.id DESC")?;
            let rows = query.query_map([], |row| Ok(SourceSummary { id: row.get(0)?, title: row.get(1)?, system_id: row.get(2)?, edition: row.get(3)?, pdf_hash: row.get(4)?, total_pages:row.get(5)?, pages: Vec::new(), ruleset: row.get::<_,Option<String>>(6)?.map(|game_id| row.get(7).map(|edition_id| crate::registry::RulesetKey { game_id,edition_id })).transpose()?, finalized: row.get(8)? }))?;
            let mut sources = rows.collect::<Result<Vec<_>, _>>()?;
            for source in &mut sources {
                let mut pages = db.prepare("SELECT id,pdf_page,complete,printed_label FROM source_pages WHERE source_id=?1 ORDER BY pdf_page")?;
                source.pages = pages.query_map([source.id], |row| Ok(PageSummary { id: row.get(0)?, pdf_page: row.get(1)?, complete: row.get(2)?, printed_label: row.get(3)? }))?.collect::<Result<Vec<_>, _>>()?;
            }
            Ok(sources)
        }).await
    }

    /// Load a page, its safe display units and append-only decision history.
    ///
    /// # Errors
    /// Fails for an unknown page or storage/worker errors.
    pub async fn source_page(&self, id: i64) -> Result<PageView, AppError> {
        self.run(move |db| {
            let mut page = db.query_row("SELECT p.id,s.id,s.title,s.edition,p.pdf_page,p.printed_label,p.complete,p.revision FROM source_pages p JOIN sources s ON s.id=p.source_id WHERE p.id=?1", [id], |row| Ok(PageView { id: row.get(0)?, source_id: row.get(1)?, title: row.get(2)?, edition: row.get(3)?, pdf_page: row.get(4)?, printed_label: row.get(5)?, complete: row.get(6)?, revision: row.get(7)?, units: Vec::new(), events: Vec::new() })).optional()?.ok_or(AppError::NotFound)?;
            let mut query = db.prepare("SELECT id,block_id,kind,polygon,raw_html,state,correction,revision FROM source_units WHERE page_id=?1 ORDER BY id")?;
            page.units = query.query_map([id], |row| { let raw: String = row.get(4)?; let clean = sanitized(&raw).map_err(|error| rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(error)))?; Ok(UnitView { id: row.get(0)?, block_id: row.get(1)?, kind: row.get(2)?, polygon: row.get(3)?, sanitized_html: clean, state: row.get(5)?, correction: row.get(6)?, revision: row.get(7)? }) })?.collect::<Result<Vec<_>, _>>()?;
            let mut history = db.prepare("SELECT unit_id,created_at,actor,state,rationale,correction,previous_revision FROM source_review_events WHERE page_id=?1 ORDER BY id DESC LIMIT 100")?;
            page.events = history.query_map([id], |row| Ok(ReviewEvent { unit_id: row.get(0)?, created_at: row.get(1)?, actor: row.get(2)?, state: row.get(3)?, rationale: row.get(4)?, correction: row.get(5)?, previous_revision: row.get(6)? }))?.collect::<Result<Vec<_>, _>>()?;
            Ok(page)
        }).await
    }

    /// Read an original page image by database identifier, never a browser path.
    ///
    /// # Errors
    /// Fails for unknown pages or storage errors.
    pub async fn source_image(&self, id: i64) -> Result<Vec<u8>, AppError> {
        self.run(move |db| {
            db.query_row("SELECT png FROM source_pages WHERE id=?1", [id], |row| {
                row.get(0)
            })
            .optional()?
            .ok_or(AppError::NotFound)
        })
        .await
    }

    /// Append an explicit unit decision and invalidate the page's completeness.
    ///
    /// # Errors
    /// Rejects invalid decisions/identity, stale revisions, unknown units or storage failures.
    pub async fn review_source_unit(
        &self,
        page: i64,
        unit: i64,
        review: UnitReview,
    ) -> Result<(), AppError> {
        validate_unit(&review)?;
        self.run(move |db| {
            let tx = db.transaction()?;
            review_unit(&tx, page, unit, &review)?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// Confirm page completeness after resolving all units and checking the original image.
    ///
    /// # Errors
    /// Rejects unresolved units, missing labels/reviewer rationale, stale revisions or storage errors.
    pub async fn complete_source_page(&self, id: i64, review: PageReview) -> Result<(), AppError> {
        validate_page(&review)?;
        self.run(move |db| {
            let tx = db.transaction()?;
            complete_page(&tx, id, &review)?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// Lookup by an exact legacy/display label, resolving registered identities first.
    ///
    /// # Errors
    /// Rejects empty/oversized queries or returns storage failures.
    pub async fn search_evidence(
        &self,
        system: String,
        edition: String,
        query: String,
    ) -> Result<EvidenceSearch, AppError> {
        if query.trim().is_empty()
            || query.chars().count() > 200
            || !crate::registry::valid_id(&system)
            || edition.trim().is_empty()
            || edition.chars().count() > 100
        {
            return Err(AppError::Invalid(
                "Provide an exact system/edition and a 1–200 character query.",
            ));
        }
        self.run(move |db| {
            let tx = db.transaction()?;
            let mut identity = crate::registry::alias(&tx, &system, &edition)?;
            if identity.is_none() {
                identity = tx.query_row("SELECT game_id,edition_id FROM registry_editions WHERE game_id=?1 AND label=?2", params![system,edition], |row| Ok(crate::registry::RulesetKey { game_id: row.get(0)?, edition_id: row.get(1)? })).optional()?;
            }
            let result = if let Some(key) = identity {
                lookup(&tx, &key.game_id, &key.edition_id, &query, true)?
            } else {
                lookup(&tx, &system, &edition, &query, false)?
            };
            tx.commit()?;
            Ok(result)
        }).await
    }

    /// Lookup only the explicitly selected stable registered pair; never infer labels.
    ///
    /// # Errors
    /// Rejects malformed IDs/queries or storage failures. Unknown pairs yield no sources.
    pub async fn search_ruleset_evidence(
        &self,
        game_id: String,
        edition_id: String,
        query: String,
    ) -> Result<EvidenceSearch, AppError> {
        if !crate::registry::valid_id(&game_id)
            || !crate::registry::valid_id(&edition_id)
            || query.trim().is_empty()
            || query.chars().count() > 200
        {
            return Err(AppError::Invalid(
                "Select valid stable game/edition IDs and a 1–200 character query.",
            ));
        }
        self.run(move |db| {
            let tx = db.transaction()?;
            let result = lookup(&tx, &game_id, &edition_id, &query, true)?;
            tx.commit()?;
            Ok(result)
        })
        .await
    }
}

pub(super) fn validate_unit(review: &UnitReview) -> Result<(), AppError> {
    review_identity(&review.actor, &review.rationale)?;
    if !["accepted", "corrected", "rejected"].contains(&review.state.as_str())
        || review.correction.chars().count() > 20_000
        || review.correction.contains('\0')
        || (review.state == "corrected" && review.correction.trim().is_empty())
        || (review.state != "corrected" && !review.correction.is_empty())
    {
        return Err(AppError::Invalid(
            "Choose accepted/corrected/rejected; only corrected decisions require separate nonempty correction text (20,000 characters, no NUL).",
        ));
    }
    Ok(())
}

pub(super) fn review_unit(
    tx: &rusqlite::Transaction<'_>,
    page: i64,
    unit: i64,
    review: &UnitReview,
) -> Result<i64, AppError> {
    let current: i64 = tx
        .query_row(
            "SELECT revision FROM source_units WHERE id=?1 AND page_id=?2",
            params![unit, page],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(AppError::NotFound)?;
    if current != review.revision {
        return Err(AppError::Conflict);
    }
    let page_revision: i64 = tx.query_row(
        "SELECT revision FROM source_pages WHERE id=?1",
        [page],
        |row| row.get(0),
    )?;
    let next_unit = next_revision(current)?;
    next_revision(page_revision)?;
    tx.execute(
        "UPDATE source_units SET state=?1,correction=?2,revision=?3 WHERE id=?4",
        params![review.state, review.correction, next_unit, unit],
    )?;
    tx.execute("INSERT INTO source_review_events (page_id,unit_id,actor,rationale,state,correction,previous_revision) VALUES (?1,?2,?3,?4,?5,?6,?7)", params![page,unit,review.actor,review.rationale,review.state,review.correction,current])?;
    Ok(tx.last_insert_rowid())
}

pub(super) fn validate_page(review: &PageReview) -> Result<(), AppError> {
    review_identity(&review.actor, &review.rationale)?;
    if review.printed_label.trim().is_empty()
        || review.printed_label.chars().count() > 50
        || review.printed_label.contains('\0')
    {
        return Err(AppError::Invalid(
            "Confirm the printed page label (or explicitly use 'unnumbered'); at most 50 characters, no NUL.",
        ));
    }
    Ok(())
}

pub(super) fn complete_page(
    tx: &rusqlite::Transaction<'_>,
    id: i64,
    review: &PageReview,
) -> Result<i64, AppError> {
    let current: i64 = tx
        .query_row(
            "SELECT revision FROM source_pages WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(AppError::NotFound)?;
    if current != review.revision {
        return Err(AppError::Conflict);
    }
    let next = next_revision(current)?;
    let pending: i64 = tx.query_row(
        "SELECT count(*) FROM source_units WHERE page_id=?1 AND state='unreviewed'",
        [id],
        |row| row.get(0),
    )?;
    if pending != 0 {
        return Err(AppError::Invalid(
            "Resolve every unit, then check omissions against the original page before confirming completeness.",
        ));
    }
    tx.execute(
        "UPDATE source_pages SET complete=1,printed_label=?1,revision=?2 WHERE id=?3",
        params![review.printed_label, next, id],
    )?;
    tx.execute("INSERT INTO source_review_events (page_id,actor,rationale,state,correction,previous_revision) VALUES (?1,?2,?3,'page-complete',?4,?5)", params![id,review.actor,review.rationale,review.printed_label,current])?;
    Ok(tx.last_insert_rowid())
}

fn next_revision(current: i64) -> Result<i64, AppError> {
    current
        .checked_add(1)
        .filter(|_| current > 0)
        .ok_or(AppError::Invalid(
            "Source review revision is invalid or exhausted; no changes saved.",
        ))
}

fn lookup(
    db: &rusqlite::Connection,
    system: &str,
    edition: &str,
    query: &str,
    registered: bool,
) -> Result<EvidenceSearch, AppError> {
    let predicate = if registered {
        "r.game_id=?1 AND r.edition_id=?2"
    } else {
        "r.source_id IS NULL AND s.system_id=?1 AND s.edition=?2"
    };
    let sources = "sources s LEFT JOIN source_rulesets r ON r.source_id=s.id";
    let source_count = db.query_row(
        &format!("SELECT count(*) FROM {sources} WHERE {predicate}"),
        params![system, edition],
        |row| row.get(0),
    )?;
    let incomplete_pages = db.query_row(&format!("SELECT count(*) FROM {sources} JOIN source_pages p ON p.source_id=s.id WHERE {predicate} AND p.complete=0"), params![system,edition], |row| row.get(0))?;
    let partial_runs = db.query_row(&format!("SELECT count(*) FROM {sources} WHERE {predicate} AND (s.total_pages=0 OR s.total_pages<>(SELECT count(*) FROM source_pages p WHERE p.source_id=s.id))"), params![system,edition], |row| row.get(0))?;
    let pattern = format!(
        "%{}%",
        query
            .trim()
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let mut statement = db.prepare(&format!(r"SELECT p.id,s.title,p.pdf_page,p.printed_label,CASE WHEN u.state='corrected' THEN u.correction ELSE u.raw_text END AS text FROM {sources} JOIN source_pages p ON p.source_id=s.id JOIN source_units u ON u.page_id=p.id WHERE {predicate} AND p.complete=1 AND u.state IN ('accepted','corrected') AND text LIKE ?3 ESCAPE '\' ORDER BY s.id,p.pdf_page,u.id LIMIT 50"))?;
    let matches = statement
        .query_map(params![system, edition, pattern], |row| {
            Ok(Evidence {
                page_id: row.get(0)?,
                title: row.get(1)?,
                pdf_page: row.get(2)?,
                printed_label: row.get(3)?,
                text: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(EvidenceSearch {
        source_count,
        incomplete_pages,
        partial_runs,
        matches,
    })
}
