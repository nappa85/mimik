//! Owner-bound material operations; prototype/local authority is never inherited.
use super::{PageReview, PageSummary, PageView, ReviewEvent, UnitReview, UnitView, sanitized};
use crate::{
    AppError, Store,
    auth::{CsrfAdmission, RequestAuth, with_domain_request},
    pagination::Listing,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

/// Where the review screen continues after a saved unit decision.
pub(crate) enum ReviewNext {
    /// The next unreviewed unit on the same page, with the card-window cursor that shows it.
    Unit { unit: i64, cursor: i64 },
    /// Every unit on the page is reviewed; the next page of the source with unreviewed units.
    Page(i64),
    /// Nothing is left to review on this page or after it.
    Done,
    /// The lookup failed (deadline, busy pool, storage error); the save itself succeeded.
    Unknown,
}

// Extracted rows retain bundle insertion order. Merge owners precede their first
// target; other owner units sort last. Cursors are ordinals in this stable order.
pub(super) const ORDERED_UNITS: &str = "WITH ordered AS (SELECT u.*,row_number() OVER (ORDER BY CASE WHEN u.origin='extracted' THEN u.id ELSE coalesce((SELECT min(json_extract(t.value,'$.unit')) FROM source_suggestion_applications a JOIN source_suggestions s ON s.id=a.suggestion_id,json_each(s.targets,'$.targets') t WHERE a.owner_unit_id=u.id AND s.kind='merge'),9223372036854775807) END,CASE u.origin WHEN 'owner' THEN 0 ELSE 1 END,u.id) position FROM source_units u WHERE u.page_id=?1)";

pub(super) const LOOKUP_TEXT: &str = "CASE u.state WHEN 'accepted' THEN coalesce(t.search_text,'') WHEN 'corrected' THEN u.correction WHEN 'unreviewed' THEN coalesce(t.search_text,'') ELSE '' END";

#[cfg(test)]
pub(crate) static AFTER_CROP: std::sync::Mutex<Option<Box<dyn Fn() + Send>>> =
    std::sync::Mutex::new(None);
#[cfg(test)]
pub(crate) static DETAIL_COMPARISON: std::sync::Mutex<Option<Box<dyn Fn() + Send>>> =
    std::sync::Mutex::new(None);
#[cfg(test)]
pub(crate) static AFTER_SUGGESTION_COMMIT: std::sync::Mutex<Option<Box<dyn Fn() + Send>>> =
    std::sync::Mutex::new(None);

pub(crate) struct Material {
    pub id: i64,
    pub title: String,
    pub edition: String,
    pub game: String,
    pub edition_id: String,
    pub hash: String,
    pub total: i64,
    pub imported: i64,
    pub complete: i64,
    pub finalized: bool,
}
pub(crate) struct MaterialPage {
    pub page: PageView,
    pub units_next: Option<i64>,
    pub history_next: Option<i64>,
}
pub(crate) struct PrimaryMaterialPage {
    pub page: PageSummary,
    pub primary: super::PrimaryPageState,
    pub selected: bool,
    pub submitted: Option<super::PrimaryPageState>,
}

// The CASE guards execute in SQLite, before a string crosses the allocation boundary.
// NULL signals corrupt/oversized evidence; it is never substituted or truncated.
pub(crate) fn text(column: &str, limit: usize) -> String {
    format!(
        "CASE WHEN typeof({column})='text' AND length(CAST({column} AS BLOB))<={limit} THEN {column} END"
    )
}
pub(crate) fn read_text(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<String> {
    row.get::<_, Option<String>>(index)?.ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Null,
            "Stored text exceeds the supported byte limit or has an invalid type.".into(),
        )
    })
}
fn cursor(after: i64) -> Result<(), AppError> {
    if after < 0 {
        Err(AppError::Invalid("Material cursors must be nonnegative."))
    } else {
        Ok(())
    }
}

impl Store {
    pub(crate) async fn material_primary_identity(
        &self,
        auth: RequestAuth,
        page: i64,
    ) -> Result<(i64, i64), AppError> {
        self.run_bounded_auth(move |db| {
            let tx = db.transaction()?;
            auth.recheck_owner(&tx)?;
            let identity = tx.query_row("SELECT coalesce(m.document_id,0),p.pdf_page FROM source_pages p LEFT JOIN source_document_members m ON m.source_id=p.source_id WHERE p.id=?1", [page], |r| Ok((r.get(0)?,r.get(1)?))).optional()?.ok_or(AppError::SourceNotFound)?;
            auth.recheck_owner(&tx)?;
            tx.commit()?;
            Ok(identity)
        }).await
    }
    pub(crate) async fn material_add_owner_unit(
        &self,
        auth: RequestAuth,
        csrf: CsrfAdmission,
        page: i64,
        mut unit: super::OwnerUnit,
    ) -> Result<i64, AppError> {
        unit.actor = format!("account:{}", auth.account_id());
        super::documents::validate_owner(&unit)?;
        self.run_bounded_auth(move |db| {
            with_domain_request(auth.clone(), || {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                auth.recheck_owner(&tx)?;
                csrf.recheck_owner_write(&tx, &auth)?;
                let event = super::documents::add_owner(&tx, page, &unit)?;
                audit(&tx, &auth, event)?;
                auth.recheck_owner(&tx)?;
                csrf.recheck_owner_write(&tx, &auth)?;
                tx.commit()?;
                Ok(event)
            })
        })
        .await
    }

    /// The grid prefilled from the selected units: one column per unit in reading order, one row per
    /// nonempty line (corrected text where the unit was corrected, the extraction otherwise).
    pub(crate) async fn material_table_prefill(
        &self,
        auth: RequestAuth,
        page: i64,
        units: Vec<i64>,
    ) -> Result<TablePrefill, AppError> {
        if units.is_empty()
            || units.len() > super::tables::MAX_COLUMNS
            || units
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != units.len()
        {
            return Err(AppError::Invalid(
                "Choose 1-16 different units for a table.",
            ));
        }
        self.run_bounded_auth(move |db| {
            let tx = db.transaction()?;
            auth.recheck_owner(&tx)?;
            let revision: i64 = tx
                .query_row("SELECT revision FROM source_pages WHERE id=?1", [page], |r| r.get(0))
                .optional()?
                .ok_or(AppError::SourceNotFound)?;
            let sql = format!(
                "{ORDERED_UNITS} SELECT position,revision,CASE WHEN state='corrected' THEN {} ELSE {} END FROM ordered WHERE id=?2",
                text("correction", 65_536),
                text("raw_text", 65_536)
            );
            let mut rows = Vec::with_capacity(units.len());
            let mut total = 0_usize;
            super::review::read_with_deadline(&tx, "table", || {
                for unit in &units {
                    let (position, unit_revision, text): (i64, i64, String) = tx
                        .query_row(&sql, params![page, unit], |r| {
                            Ok((r.get(0)?, r.get(1)?, read_text(r, 2)?))
                        })
                        .optional()?
                        .ok_or(AppError::SourceNotFound)?;
                    total += text.len();
                    if total > super::tables::MAX_HTML_BYTES {
                        return Err(AppError::Invalid(
                            "The selected units hold more than 64 KiB of text; select fewer or shorter units.",
                        ));
                    }
                    rows.push((position, *unit, unit_revision, text));
                }
                Ok(())
            })?;
            rows.sort_unstable();
            auth.recheck_owner(&tx)?;
            tx.commit()?;
            let columns: Vec<String> = rows.iter().map(|r| r.3.clone()).collect();
            let grid = super::tables::columns_to_grid(&columns)?;
            Ok(TablePrefill {
                page_revision: revision,
                units: rows.iter().map(|r| (r.1, r.2)).collect(),
                grid: super::tables::format_grid(&grid),
            })
        })
        .await
    }

    /// Create an owner table from selected units: one immediate transaction rechecks the Owner,
    /// CSRF, page revision and every unit revision, adds the table unit and rejects the originals.
    pub(crate) async fn material_create_table(
        &self,
        auth: RequestAuth,
        csrf: CsrfAdmission,
        page: i64,
        mut table: super::OwnerTable,
    ) -> Result<(i64, Vec<i64>), AppError> {
        table.actor = format!("account:{}", auth.account_id());
        super::documents::validate_table(&table)?;
        self.run_bounded_auth(move |db| {
            with_domain_request(auth.clone(), || {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                auth.recheck_owner(&tx)?;
                csrf.recheck_owner_write(&tx, &auth)?;
                let created = super::documents::add_owner_table(&tx, page, &table, |tx, event| {
                    audit(tx, &auth, event)
                })?;
                auth.recheck_owner(&tx)?;
                csrf.recheck_owner_write(&tx, &auth)?;
                tx.commit()?;
                Ok(created)
            })
        })
        .await
    }

    pub(crate) async fn material_set_primary_source(
        &self,
        auth: RequestAuth,
        csrf: CsrfAdmission,
        document: i64,
        pages: Vec<super::PrimaryPageState>,
        source: i64,
        rationale: String,
    ) -> Result<Vec<i64>, AppError> {
        self.run_bounded_auth(move |db| {
            with_domain_request(auth.clone(), || {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                csrf.recheck_owner_write(&tx, &auth)?;
                let events = super::documents::set_primary(&tx, document, &pages, source, &format!("account:{}",auth.account_id()), &rationale, |tx,event| {
                    tx.execute("INSERT INTO source_primary_web_events(event_id,actor_id,actor_epoch,session_digest) VALUES(?1,?2,?3,?4)",params![event,auth.account_id(),auth.epoch(),auth.digest()])?;
                    Ok(())
                })?;
                csrf.recheck_owner_write(&tx, &auth)?;
                tx.commit()?; Ok(events)
            })
        }).await
    }

    pub(crate) async fn material_review_units_atomic(
        &self,
        auth: RequestAuth,
        csrf: CsrfAdmission,
        mut decisions: Vec<super::UnitDecision>,
        rationale: String,
    ) -> Result<Vec<i64>, AppError> {
        for decision in &mut decisions {
            decision.review.actor = format!("account:{}", auth.account_id());
        }
        super::documents::validate_batch(&decisions, &rationale)?;
        let operation = (decisions.len() > 1).then(super::documents::new_operation_id);
        self.run_bounded_auth(move |db| {
            with_domain_request(auth.clone(), || {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                auth.recheck_owner(&tx)?;
                csrf.recheck_owner_write(&tx, &auth)?;
                let events = if let Some(operation) = &operation {
                    super::documents::review_batch(
                        &tx,
                        &decisions,
                        operation,
                        &rationale,
                        |tx, event| audit(tx, &auth, event),
                    )?
                } else {
                    let decision = &decisions[0];
                    let event = super::store::review_unit(
                        &tx,
                        decision.page,
                        decision.unit,
                        &decision.review,
                    )?;
                    audit(&tx, &auth, event)?;
                    vec![event]
                };
                auth.recheck_owner(&tx)?;
                csrf.recheck_owner_write(&tx, &auth)?;
                tx.commit()?;
                Ok(events)
            })
        })
        .await
    }

    pub(crate) async fn material_sources(
        &self,
        auth: RequestAuth,
        after: i64,
    ) -> Result<Listing<Material>, AppError> {
        cursor(after)?;
        self.run_bounded_auth(move |db| {
            let tx=db.transaction()?; auth.recheck_owner(&tx)?;
            let sql=format!("SELECT s.id,{},{},CASE WHEN r.source_id IS NULL THEN '' ELSE {} END,CASE WHEN r.source_id IS NULL THEN '' ELSE {} END,{},s.total_pages,(SELECT count(*) FROM source_pages p WHERE p.source_id=s.id),(SELECT count(*) FROM source_pages p WHERE p.source_id=s.id AND p.complete=1),EXISTS(SELECT 1 FROM source_finalizations f WHERE f.source_id=s.id) FROM sources s LEFT JOIN source_rulesets r ON r.source_id=s.id WHERE s.id>?1 ORDER BY s.id LIMIT 51",text("s.title",200),text("s.edition",400),text("r.game_id",64),text("r.edition_id",64),text("s.pdf_hash",64));
            let rows={let mut q=tx.prepare(&sql)?; q.query_map([after], |r| Ok(Material{id:r.get(0)?,title:read_text(r,1)?,edition:read_text(r,2)?,game:read_text(r,3)?,edition_id:read_text(r,4)?,hash:read_text(r,5)?,total:r.get(6)?,imported:r.get(7)?,complete:r.get(8)?,finalized:r.get(9)?}))?.collect::<Result<Vec<_>,_>>()?};
            auth.recheck_owner(&tx)?;tx.commit()?; Ok(Listing::new(rows, |r|r.id))
        }).await
    }
    pub(crate) async fn material_pages(
        &self,
        auth: RequestAuth,
        source: i64,
        after: i64,
    ) -> Result<Listing<PageSummary>, AppError> {
        cursor(after)?;
        self.run_bounded_auth(move |db|{
            let tx=db.transaction()?;auth.recheck_owner(&tx)?;
            if !tx.query_row("SELECT EXISTS(SELECT 1 FROM sources WHERE id=?1)",[source],|r|r.get::<_,bool>(0))?{return Err(AppError::SourceNotFound);}
            let rows={let mut q=tx.prepare(&format!("SELECT id,pdf_page,complete,{} FROM source_pages WHERE source_id=?1 AND id>?2 ORDER BY id LIMIT 51",text("printed_label",200)))?;q.query_map(params![source,after],|r|Ok(PageSummary{id:r.get(0)?,pdf_page:r.get(1)?,complete:r.get(2)?,printed_label:read_text(r,3)?}))?.collect::<Result<Vec<_>,_>>()?};
            auth.recheck_owner(&tx)?;tx.commit()?;Ok(Listing::new(rows,|r|r.id))
        }).await
    }
    pub(crate) async fn material_primary_pages(
        &self,
        auth: RequestAuth,
        source: i64,
        after: i64,
    ) -> Result<(i64, Listing<PrimaryMaterialPage>), AppError> {
        let listing = self.material_pages(auth.clone(), source, after).await?;
        self.run_bounded_auth(move |db| {
            let tx = db.transaction()?;
            auth.recheck_owner(&tx)?;
            let document = tx
                .query_row(
                    "SELECT document_id FROM source_document_members WHERE source_id=?1",
                    [source],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?
                .unwrap_or(0);
            let mut rows = Vec::new();
            for page in listing.rows {
                let primary = super::documents::primary_state(&tx, document, page.pdf_page)?;
                rows.push(PrimaryMaterialPage {
                    page,
                    primary,
                    selected: false,
                    submitted: None,
                });
            }
            auth.recheck_owner(&tx)?;
            tx.commit()?;
            Ok((
                document,
                Listing {
                    rows,
                    next: listing.next,
                },
            ))
        })
        .await
    }
    pub(crate) async fn material_page(
        &self,
        auth: RequestAuth,
        id: i64,
        units: i64,
        history: i64,
    ) -> Result<MaterialPage, AppError> {
        cursor(units)?;
        cursor(history)?;
        self.run_bounded_auth(move|db|{
            let tx=db.transaction()?;auth.recheck_owner(&tx)?;
            let result=super::review::read_with_deadline(&tx,"page",|| {
            let mut page=tx.query_row(&format!("SELECT p.id,s.id,{},{},p.pdf_page,{},p.complete,p.revision FROM source_pages p JOIN sources s ON s.id=p.source_id WHERE p.id=?1",text("s.title",200),text("s.edition",400),text("p.printed_label",200)),[id],|r|Ok(PageView{id:r.get(0)?,source_id:r.get(1)?,title:read_text(r,2)?,edition:read_text(r,3)?,pdf_page:r.get(4)?,printed_label:read_text(r,5)?,complete:r.get(6)?,revision:r.get(7)?,units:Vec::new(),events:Vec::new()})).optional()?.ok_or(AppError::SourceNotFound)?;
            let rows={let mut q=tx.prepare(&format!("{ORDERED_UNITS} SELECT id,{},{},{},{},{},{},revision,position FROM ordered WHERE position>?2 ORDER BY position LIMIT 51",text("block_id",512),text("kind",100),text("polygon",1024),text("raw_html",65536),text("state",32),text("correction",80000)))?;
                q.query_map(params![id,units],|r|{let raw=read_text(r,4)?;let clean=sanitized(&raw).map_err(|e|rusqlite::Error::FromSqlConversionFailure(4,rusqlite::types::Type::Text,Box::new(e)))?;Ok((r.get::<_,i64>(8)?,UnitView{id:r.get(0)?,block_id:read_text(r,1)?,kind:read_text(r,2)?,polygon:read_text(r,3)?,sanitized_html:clean,state:read_text(r,5)?,correction:read_text(r,6)?,revision:r.get(7)?}))})?.collect::<Result<Vec<_>,_>>()?};
            let events={let mut q=tx.prepare(&format!("SELECT id,unit_id,{},{},{},{},{},previous_revision,operation_id,(SELECT t.unit_id FROM source_owner_tables t WHERE t.first_event_id=source_review_events.id),(SELECT group_concat(unit_id,', ') FROM (SELECT o.unit_id FROM source_review_events o WHERE o.operation_id=source_review_events.operation_id AND source_review_events.id IN (SELECT first_event_id FROM source_owner_tables) ORDER BY o.id)) FROM source_review_events WHERE page_id=?1 AND id>?2 ORDER BY id LIMIT 51",text("created_at",64),text("actor",400),text("state",32),text("rationale",20000),text("correction",80000)))?;
                q.query_map(params![id,history],|r|Ok((r.get::<_,i64>(0)?,ReviewEvent{unit_id:r.get(1)?,created_at:read_text(r,2)?,actor:read_text(r,3)?,state:read_text(r,4)?,rationale:read_text(r,5)?,correction:read_text(r,6)?,previous_revision:r.get(7)?,operation_id:r.get(8)?,table:r.get::<_,Option<i64>>(9)?.zip(r.get::<_,Option<String>>(10)?)})))?.collect::<Result<Vec<_>,_>>()?};
            let units=Listing::new(rows,|r|r.0);let events=Listing::new(events,|r|r.0);
            page.units=units.rows.into_iter().map(|r|r.1).collect();page.events=events.rows.into_iter().map(|r|r.1).collect();
            Ok(MaterialPage{page,units_next:units.next,history_next:events.next})
            })?;
            auth.recheck_owner(&tx)?;tx.commit()?;
            Ok(result)
        }).await
    }
    pub(crate) async fn material_unit_cursor(
        &self,
        auth: RequestAuth,
        page: i64,
        unit: i64,
    ) -> Result<i64, AppError> {
        self.run_bounded_auth(move |db| {
            let tx = db.transaction()?;
            auth.recheck_owner(&tx)?;
            let cursor = super::review::read_with_deadline(&tx, "cursor", || {
                tx.query_row(
                    &format!("{ORDERED_UNITS} SELECT position-1 FROM ordered WHERE id=?2"),
                    params![page, unit],
                    |r| r.get(0),
                )
                .optional()?
                .ok_or(AppError::SourceNotFound)
            })?;
            auth.recheck_owner(&tx)?;
            tx.commit()?;
            Ok(cursor)
        })
        .await
    }
    /// Where the Owner should continue after saving a unit decision: the next unit still
    /// unreviewed on this page (after the saved one, else from the start), else the next page of
    /// the source with unreviewed units, else the page summary.
    pub(crate) async fn material_next_after_review(
        &self,
        auth: RequestAuth,
        page: i64,
        unit: i64,
    ) -> Result<ReviewNext, AppError> {
        self.run_bounded_auth(move |db| {
            let tx = db.transaction()?;
            auth.recheck_owner(&tx)?;
            let next = super::review::read_with_deadline(&tx, "next", || {
                let on_page = |after: i64| {
                    tx.query_row(
                        &format!("{ORDERED_UNITS} SELECT id,position FROM ordered WHERE state='unreviewed' AND position>?2 ORDER BY position LIMIT 1"),
                        params![page, after],
                        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
                    )
                    .optional()
                };
                let saved: i64 = tx
                    .query_row(
                        &format!("{ORDERED_UNITS} SELECT position FROM ordered WHERE id=?2"),
                        params![page, unit],
                        |r| r.get(0),
                    )
                    .optional()?
                    .unwrap_or(0);
                let found = match on_page(saved)? {
                    Some(row) => Some(row),
                    None => on_page(0)?,
                };
                if let Some((id, position)) = found {
                    return Ok(ReviewNext::Unit {
                        unit: id,
                        // The default card window already shows positions 1..=50.
                        cursor: if position <= 50 { 0 } else { position - 1 },
                    });
                }
                let next_page = tx
                    .query_row(
                        "SELECT p.id FROM source_pages p JOIN source_pages c ON c.id=?1 AND c.source_id=p.source_id WHERE p.pdf_page>c.pdf_page AND EXISTS(SELECT 1 FROM source_units u WHERE u.page_id=p.id AND u.state='unreviewed') ORDER BY p.pdf_page LIMIT 1",
                        [page],
                        |r| r.get(0),
                    )
                    .optional()?;
                Ok(next_page.map_or(ReviewNext::Done, ReviewNext::Page))
            })?;
            auth.recheck_owner(&tx)?;
            tx.commit()?;
            Ok(next)
        })
        .await
    }
    /// Whether a saved-notice link is true: the unit is on this page (or, for `page`, on another
    /// page of the source), is now in the stated state, and for `done` nothing unreviewed remains
    /// on this page or after it.
    pub(crate) async fn material_saved_matches(
        &self,
        auth: RequestAuth,
        page: i64,
        unit: i64,
        state: &'static str,
        then: &'static str,
    ) -> Result<bool, AppError> {
        self.run_bounded_auth(move |db| {
            let tx = db.transaction()?;
            auth.recheck_owner(&tx)?;
            let found = tx
                .query_row(
                    "SELECT 1 FROM source_units u JOIN source_pages up ON up.id=u.page_id JOIN source_pages cp ON cp.id=?1 WHERE u.id=?2 AND u.state=?3 AND up.source_id=cp.source_id AND (up.id=cp.id OR ?4='page') AND (?4<>'done' OR NOT EXISTS(SELECT 1 FROM source_units x JOIN source_pages xp ON xp.id=x.page_id WHERE xp.source_id=cp.source_id AND xp.pdf_page>=cp.pdf_page AND x.state='unreviewed'))",
                    params![page, unit, state, then],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            auth.recheck_owner(&tx)?;
            tx.commit()?;
            Ok(found)
        })
        .await
    }
    /// Whether `unit` is an owner table on `page` (a "created" notice link must be true).
    pub(crate) async fn material_is_owner_table(
        &self,
        auth: RequestAuth,
        page: i64,
        unit: i64,
    ) -> Result<bool, AppError> {
        self.run_bounded_auth(move |db| {
            let tx = db.transaction()?;
            auth.recheck_owner(&tx)?;
            let found = tx
                .query_row(
                    "SELECT 1 FROM source_owner_tables t JOIN source_units u ON u.id=t.unit_id WHERE u.id=?1 AND u.page_id=?2",
                    params![unit, page],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            auth.recheck_owner(&tx)?;
            tx.commit()?;
            Ok(found)
        })
        .await
    }
    pub(crate) async fn material_image(
        &self,
        auth: RequestAuth,
        id: i64,
    ) -> Result<Vec<u8>, AppError> {
        self.run_bounded_auth(move|db|{
            let tx=db.transaction()?;auth.recheck_owner(&tx)?;
            let bytes=tx.query_row("SELECT CASE WHEN typeof(png)='blob' AND length(png)<=16777216 THEN png END FROM source_pages WHERE id=?1",[id],|r|r.get::<_,Option<Vec<u8>>>(0)).optional()?.ok_or(AppError::SourceNotFound)?.ok_or(AppError::Invalid("Original page image exceeds the supported byte limit."))?;
            auth.recheck_owner(&tx)?;tx.commit()?;Ok(bytes)
        }).await
    }
    pub(crate) async fn material_recheck(&self, auth: RequestAuth) -> Result<(), AppError> {
        self.run_bounded_auth(move |db| {
            let tx = db.transaction()?;
            auth.recheck_owner(&tx)?;
            tx.commit()?;
            Ok(())
        })
        .await
    }
    pub(crate) async fn material_review_unit(
        &self,
        auth: RequestAuth,
        csrf: CsrfAdmission,
        page: i64,
        unit: i64,
        review: UnitReview,
    ) -> Result<(), AppError> {
        let rationale = review.rationale.clone();
        self.material_review_units_atomic(
            auth,
            csrf,
            vec![super::UnitDecision { page, unit, review }],
            rationale,
        )
        .await
        .map(|_| ())
    }
    pub(crate) async fn material_complete(
        &self,
        auth: RequestAuth,
        csrf: CsrfAdmission,
        page: i64,
        mut review: PageReview,
    ) -> Result<(), AppError> {
        review.actor = format!("account:{}", auth.account_id());
        super::store::validate_page(&review)?;
        self.run_bounded_auth(move |db| {
            with_domain_request(auth.clone(), || {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                auth.recheck_owner(&tx)?;
                csrf.recheck_owner_write(&tx, &auth)?;
                let event = super::store::complete_page(&tx, page, &review)?;
                audit(&tx, &auth, event)?;
                auth.recheck_owner(&tx)?;
                csrf.recheck_owner_write(&tx, &auth)?;
                tx.commit()?;
                Ok(())
            })
        })
        .await
    }
}
pub(super) fn audit(db: &Connection, auth: &RequestAuth, event: i64) -> Result<(), AppError> {
    db.execute("INSERT INTO source_web_events(event_id,actor_id,actor_epoch,session_digest) VALUES(?1,?2,?3,?4)",params![event,auth.account_id(),auth.epoch(),auth.digest()])?;
    Ok(())
}

/// What the create-table form needs: the tokens to submit back and the editable grid text.
pub(crate) struct TablePrefill {
    pub page_revision: i64,
    /// Selected units in reading order with the revision the Owner saw.
    pub units: Vec<(i64, i64)>,
    pub grid: String,
}
