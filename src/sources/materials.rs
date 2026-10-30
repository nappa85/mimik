//! Owner-bound material operations; prototype/local authority is never inherited.
use super::{PageReview, PageSummary, PageView, ReviewEvent, UnitReview, UnitView, sanitized};
use crate::{
    AppError, Store,
    auth::{CsrfAdmission, RequestAuth, with_domain_request},
    pagination::Listing,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

#[cfg(test)]
pub(crate) static AFTER_CROP: std::sync::Mutex<Option<Box<dyn Fn() + Send>>> =
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
    // Authenticated wrappers for the review screens of the next step; only tests call them
    // until its routes exist.
    #[cfg(test)]
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
                csrf.recheck(&tx)?;
                let event = super::documents::add_owner(&tx, page, &unit)?;
                audit(&tx, &auth, event)?;
                auth.recheck_owner(&tx)?;
                csrf.recheck(&tx)?;
                tx.commit()?;
                Ok(event)
            })
        })
        .await
    }

    #[cfg(test)]
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
                auth.recheck_owner(&tx)?; csrf.recheck(&tx)?;
                let events = super::documents::set_primary(&tx, document, &pages, source, &format!("account:{}",auth.account_id()), &rationale, |tx,event| {
                    tx.execute("INSERT INTO source_primary_web_events(event_id,actor_id,actor_epoch,session_digest) VALUES(?1,?2,?3,?4)",params![event,auth.account_id(),auth.epoch(),auth.digest()])?;
                    Ok(())
                })?;
                auth.recheck_owner(&tx)?; csrf.recheck(&tx)?;
                tx.commit()?; Ok(events)
            })
        }).await
    }

    #[cfg(test)]
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
        let operation = super::documents::new_operation_id();
        self.run_bounded_auth(move |db| {
            with_domain_request(auth.clone(), || {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                auth.recheck_owner(&tx)?;
                csrf.recheck(&tx)?;
                let events = super::documents::review_batch(
                    &tx,
                    &decisions,
                    &operation,
                    &rationale,
                    |tx, event| audit(tx, &auth, event),
                )?;
                auth.recheck_owner(&tx)?;
                csrf.recheck(&tx)?;
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
            let mut page=tx.query_row(&format!("SELECT p.id,s.id,{},{},p.pdf_page,{},p.complete,p.revision FROM source_pages p JOIN sources s ON s.id=p.source_id WHERE p.id=?1",text("s.title",200),text("s.edition",400),text("p.printed_label",200)),[id],|r|Ok(PageView{id:r.get(0)?,source_id:r.get(1)?,title:read_text(r,2)?,edition:read_text(r,3)?,pdf_page:r.get(4)?,printed_label:read_text(r,5)?,complete:r.get(6)?,revision:r.get(7)?,units:Vec::new(),events:Vec::new()})).optional()?.ok_or(AppError::SourceNotFound)?;
            let rows={let mut q=tx.prepare(&format!("SELECT id,{},{},{},{},{},{},revision FROM source_units WHERE page_id=?1 AND id>?2 ORDER BY id LIMIT 51",text("block_id",512),text("kind",100),text("polygon",1024),text("raw_html",65536),text("state",32),text("correction",80000)))?;
                q.query_map(params![id,units],|r|{let raw=read_text(r,4)?;let clean=sanitized(&raw).map_err(|e|rusqlite::Error::FromSqlConversionFailure(4,rusqlite::types::Type::Text,Box::new(e)))?;Ok(UnitView{id:r.get(0)?,block_id:read_text(r,1)?,kind:read_text(r,2)?,polygon:read_text(r,3)?,sanitized_html:clean,state:read_text(r,5)?,correction:read_text(r,6)?,revision:r.get(7)?})})?.collect::<Result<Vec<_>,_>>()?};
            let events={let mut q=tx.prepare(&format!("SELECT id,unit_id,{},{},{},{},{},previous_revision,operation_id FROM source_review_events WHERE page_id=?1 AND id>?2 ORDER BY id LIMIT 51",text("created_at",64),text("actor",400),text("state",32),text("rationale",20000),text("correction",80000)))?;
                q.query_map(params![id,history],|r|Ok((r.get::<_,i64>(0)?,ReviewEvent{unit_id:r.get(1)?,created_at:read_text(r,2)?,actor:read_text(r,3)?,state:read_text(r,4)?,rationale:read_text(r,5)?,correction:read_text(r,6)?,previous_revision:r.get(7)?,operation_id:r.get(8)?})))?.collect::<Result<Vec<_>,_>>()?};
            auth.recheck_owner(&tx)?;tx.commit()?;
            let units=Listing::new(rows,|r|r.id);let events=Listing::new(events,|r|r.0);
            page.units=units.rows;page.events=events.rows.into_iter().map(|r|r.1).collect();
            Ok(MaterialPage{page,units_next:units.next,history_next:events.next})
        }).await
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
        mut review: UnitReview,
    ) -> Result<(), AppError> {
        review.actor = format!("account:{}", auth.account_id());
        super::store::validate_unit(&review)?;
        self.run_bounded_auth(move |db| {
            with_domain_request(auth.clone(), || {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                auth.recheck_owner(&tx)?;
                csrf.recheck(&tx)?;
                let event = super::store::review_unit(&tx, page, unit, &review)?;
                audit(&tx, &auth, event)?;
                auth.recheck_owner(&tx)?;
                csrf.recheck(&tx)?;
                tx.commit()?;
                Ok(())
            })
        })
        .await
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
                csrf.recheck(&tx)?;
                let event = super::store::complete_page(&tx, page, &review)?;
                audit(&tx, &auth, event)?;
                auth.recheck_owner(&tx)?;
                csrf.recheck(&tx)?;
                tx.commit()?;
                Ok(())
            })
        })
        .await
    }
}
fn audit(db: &Connection, auth: &RequestAuth, event: i64) -> Result<(), AppError> {
    db.execute("INSERT INTO source_web_events(event_id,actor_id,actor_epoch,session_digest) VALUES(?1,?2,?3,?4)",params![event,auth.account_id(),auth.epoch(),auth.digest()])?;
    Ok(())
}
