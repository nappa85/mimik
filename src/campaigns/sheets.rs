use super::{
    Listing, campaign, role,
    snapshots::{self, Pin, PrivateSnapshot},
};
use crate::{
    AppError, Store,
    auth::RequestAuth,
    definitions::{
        WiringStatus,
        private_pin::{self, ImmutablePin},
    },
};
use rusqlite::{TransactionBehavior, params};
use tokio::sync::OwnedSemaphorePermit;

pub(crate) struct SheetSummary {
    pub id: i64,
    pub name: String,
    pub state: String,
    pub revision: i64,
}
pub(crate) struct HistorySummary {
    pub revision: i64,
    pub operation: String,
    pub actor: String,
    pub occurred_at: i64,
}
pub(crate) struct SheetView {
    pub snapshot: PrivateSnapshot,
    pub immutable: Option<ImmutablePin>,
    pub warning: String,
    pub editable: bool,
}

#[cfg(test)]
pub(super) static AFTER_PRIVATE_VALIDATION: std::sync::Mutex<
    Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
> = std::sync::Mutex::new(None);

pub(super) fn match_pin(snapshot: &PrivateSnapshot, pin: &ImmutablePin) -> Result<(), AppError> {
    match &snapshot.pin {
        Pin::Runtime {
            version_id,
            version,
            content_hash,
            schema,
        } if *schema == 1
            && *version_id == pin.raw.id
            && *version == pin.raw.version
            && *content_hash == pin.raw.hash
            && snapshot.game_id == pin.raw.game
            && snapshot.edition_id == pin.raw.edition
            && snapshot.kind == "runtime" =>
        {
            Ok(())
        }
        _ => Err(AppError::Invalid("Runtime interpretation pin mismatch.")),
    }
}

impl Store {
    #[cfg(test)]
    pub(crate) async fn private_list(
        &self,
        auth: RequestAuth,
        campaign_id: Option<i64>,
        after: i64,
    ) -> Result<Vec<SheetSummary>, AppError> {
        Ok(self.private_list_page(auth, campaign_id, after).await?.rows)
    }

    pub(crate) async fn private_list_page(
        &self,
        auth: RequestAuth,
        campaign_id: Option<i64>,
        after: i64,
    ) -> Result<Listing<SheetSummary>, AppError> {
        if after < 0 {
            return Err(AppError::Invalid("Cursor must be nonnegative."));
        }
        self.run_bounded_auth(move |db| {
            let tx=db.transaction()?;auth.recheck(&tx)?;
            let is_master=if let Some(id)=campaign_id{role(&tx,id,auth.account_id())?.as_deref()==Some("master")}else{false};
            let list={let mut q=tx.prepare("SELECT id,CASE WHEN length(CAST(snapshot AS BLOB))<=1048576 THEN CASE WHEN length(json_extract(snapshot,'$.name'))<=100 THEN json_extract(snapshot,'$.name') END END,CASE WHEN length(state)<=6 THEN state END,revision FROM private_sheets WHERE id>?1 AND (?2 IS NULL OR campaign_id=?2) AND (custodian_id=?3 OR ?4) ORDER BY id LIMIT 51")?;
                q.query_map(params![after,campaign_id,auth.account_id(),is_master],|r|Ok(SheetSummary{id:r.get(0)?,name:r.get(1)?,state:r.get(2)?,revision:r.get(3)?}))?.collect::<Result<Vec<_>,_>>()?};
            auth.recheck(&tx)?;tx.commit()?;Ok(Listing::new(list,|row|row.id))
        }).await
    }

    pub(crate) async fn private_read(
        &self,
        auth: RequestAuth,
        id: i64,
        history: Option<i64>,
        permit: OwnedSemaphorePermit,
    ) -> Result<(SheetView, OwnedSemaphorePermit), AppError> {
        let admitted_auth = auth.clone();
        let (snapshot,raw,warning,editable,permit)=self.run_bounded_auth(move |db|{
            let tx=db.transaction()?;auth.recheck(&tx)?;snapshots::authorize_read(&tx,&auth,id)?;
            let snapshot=snapshots::raw_load(&tx,id,history)?;
            let editable=if history.is_some(){false}else{match snapshots::authorize_edit(&tx,&auth,&snapshot.identity){Ok(())=>true,Err(AppError::NotFound)=>false,Err(error)=>return Err(error)}};
            let raw=if let Pin::Runtime{version_id,schema:1,..}=snapshot.identity.pin{Some(private_pin::load(&tx,version_id)?)}else{None};
            let warning=if !snapshots::supported(&snapshot.identity){"Unsupported immutable interpretation pin. Retained canonical data is read-only; it has not been reinterpreted.".into()}else if history.is_some(){"Historical revision: a retained read-only snapshot.".into()}else if snapshot.identity.state=="frozen"{"Frozen on departure/removal. Rejoining does not reactivate this sheet.".into()}else if !editable{"Read-only: active custody/current campaign grants and an enabled supervising Master are required to edit.".into()}else{String::new()};
            auth.recheck(&tx)?;tx.commit()?;Ok((snapshot,raw,warning,editable,permit))
        }).await?;
        // Immutable definition decoding/compilation is outside the database mutex.
        let (view, permit) = tokio::task::spawn_blocking(move || {
            let snapshot = snapshot.decode()?;
            let immutable = raw.map(private_pin::validate).transpose()?;
            if let Some(pin) = &immutable {
                match_pin(&snapshot, pin)?;
                pin.validate_values(&snapshot.data)?;
            }
            #[cfg(test)]
            if let Some(hook) = AFTER_PRIVATE_VALIDATION.lock().unwrap().take() {
                hook();
            }
            Ok::<_, AppError>((
                SheetView {
                    snapshot,
                    immutable,
                    warning,
                    editable,
                },
                permit,
            ))
        })
        .await
        .map_err(|_| AppError::Worker)??;
        self.private_badge(admitted_auth, view, permit, false, history)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn private_history(
        &self,
        auth: RequestAuth,
        id: i64,
        after: i64,
    ) -> Result<Vec<HistorySummary>, AppError> {
        Ok(self.private_history_page(auth, id, after).await?.rows)
    }

    pub(crate) async fn private_history_page(
        &self,
        auth: RequestAuth,
        id: i64,
        after: i64,
    ) -> Result<Listing<HistorySummary>, AppError> {
        if after < 0 {
            return Err(AppError::Invalid("Cursor must be nonnegative."));
        }
        self.run_campaign(auth.clone(),move |db|{
            let tx=db.transaction()?;auth.recheck(&tx)?;snapshots::authorize_read(&tx,&auth,id)?;
            let rows={let mut q=tx.prepare("SELECT h.revision,h.operation,a.display_name,h.occurred_at FROM private_sheet_history h JOIN accounts a ON a.id=h.actor_id WHERE h.sheet_id=?1 AND h.revision>?2 ORDER BY h.revision LIMIT 51")?;q.query_map(params![id,after],|r|Ok(HistorySummary{revision:r.get(0)?,operation:r.get(1)?,actor:r.get(2)?,occurred_at:r.get(3)?}))?.collect::<Result<Vec<_>,_>>()?};
            auth.recheck(&tx)?;tx.commit()?;Ok(Listing::new(rows,|row|row.revision))
        }).await
    }

    pub(crate) async fn private_create_context(
        &self,
        auth: RequestAuth,
        id: i64,
        version: i64,
        permit: OwnedSemaphorePermit,
    ) -> Result<(SheetView, OwnedSemaphorePermit), AppError> {
        let admitted_auth = auth.clone();
        let (snapshot,raw,permit)=self.run_bounded_auth(move |db|{
            let tx=db.transaction()?;auth.recheck(&tx)?;
            let campaign=campaign(&tx,id)?;
            if role(&tx,id,auth.account_id())?.is_none(){return Err(AppError::NotFound);}
            let (pin,kind,data,raw)=if version==0{
                let key=crate::registry::RulesetKey{game_id:campaign.game_id.clone(),edition_id:campaign.edition_id.clone()};
                let adapter=crate::games::adapter_for_ruleset(&key).ok_or(AppError::Invalid("Capability unavailable: select an approved runtime definition."))?;
                let loaded=crate::games::native_available(&tx,&key)?;
                if !loaded{return Err(AppError::Invalid("Capability unavailable: load the exact implemented Italian source pair."));}
                (Pin::Native{adapter:adapter.id.into(),schema:1,calculation_version:1},"native".into(),serde_json::to_value(crate::mork_borg::Sheet::default())?,None)
            }else{
                let raw=private_pin::load(&tx,version)?;
                if raw.game!=campaign.game_id || raw.edition!=campaign.edition_id{return Err(AppError::NotFound);}
                (Pin::Runtime{version_id:version,version:raw.version,content_hash:raw.hash.clone(),schema:1},"runtime".into(),serde_json::json!({}),Some(raw))
            };
            let snapshot=PrivateSnapshot{encoding:1,sheet_id:0,campaign_id:id,game_id:campaign.game_id,edition_id:campaign.edition_id,custodian_id:auth.account_id(),kind,pin,revision:0,state:"active".into(),name:String::new(),notes:String::new(),data};
            snapshots::authorize_edit(&tx,&auth,&snapshot)?;auth.recheck(&tx)?;tx.commit()?;Ok((snapshot,raw,permit))
        }).await?;
        let (view, permit) = tokio::task::spawn_blocking(move || {
            let immutable = raw.map(private_pin::validate).transpose()?;
            Ok::<_, AppError>((
                SheetView {
                    snapshot,
                    immutable,
                    warning: String::new(),
                    editable: true,
                },
                permit,
            ))
        })
        .await
        .map_err(|_| AppError::Worker)??;
        self.private_badge(admitted_auth, view, permit, true, None)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn private_save(
        &self,
        auth: RequestAuth,
        view: SheetView,
        permit: OwnedSemaphorePermit,
    ) -> Result<i64, AppError> {
        self.private_save_keeping(auth, view, permit)
            .await
            .map(|(id, _)| id)
            .map_err(|(error, _)| error)
    }

    /// Saves a private sheet and hands the runtime permit back, also when the
    /// database step rejects the save, so the rejection can re-render the draft under the
    /// same permit. A worker or authorization failure returns no permit.
    pub(crate) async fn private_save_keeping(
        &self,
        auth: RequestAuth,
        mut view: SheetView,
        permit: OwnedSemaphorePermit,
    ) -> Result<(i64, OwnedSemaphorePermit), (AppError, Option<OwnedSemaphorePermit>)> {
        let outcome = self.run_campaign(auth.clone(),move |db|{
            let result=(||{
            let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;auth.recheck(&tx)?;
            let create=view.snapshot.sheet_id==0;
            if create {snapshots::authorize_edit(&tx,&auth,&view.snapshot)?;}else{
                let old=snapshots::load(&tx,view.snapshot.sheet_id)?;
                snapshots::authorize_edit(&tx,&auth,&old)?;
                if old.revision!=view.snapshot.revision{return Err(AppError::Conflict);}
            }
            if let Some(pin)=&view.immutable{
                match_pin(&view.snapshot,pin)?;pin.unchanged(&tx)?;
                if create && pin.readiness(&tx)?!=WiringStatus::Current{return Err(AppError::Invalid("Capability unavailable: wiring approval is stale, withdrawn, or evidence is unavailable."));}
            }else if create {
                let loaded:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM source_rulesets WHERE game_id=?1 AND edition_id=?2)",params![view.snapshot.game_id,view.snapshot.edition_id],|r|r.get(0))?;
                if !loaded{return Err(AppError::Invalid("Capability unavailable: exact native source is not loaded."));}
            }
            if create {
                let total:i64=tx.query_row("SELECT count(*) FROM private_sheets",[],|r|r.get(0))?;
                let retained:i64=tx.query_row("SELECT count(*) FROM private_sheets WHERE campaign_id=?1 AND custodian_id=?2",params![view.snapshot.campaign_id,view.snapshot.custodian_id],|r|r.get(0))?;
                if total>=2048 || retained>=32{return Err(AppError::Invalid("Private sheet retained capacity reached; frozen sheets remain retained."));}
                let max:i64=tx.query_row("SELECT coalesce(max(id),0) FROM private_sheets",[],|r|r.get(0))?;
                view.snapshot.sheet_id=max.checked_add(1).ok_or(AppError::Conflict)?;
            }
            let result=snapshots::write(&tx,&auth,view.snapshot,if create{"create"}else{"edit"},view.immutable.as_ref())?;
            tx.commit()?;Ok(result.sheet_id)
            })();
            Ok((result,permit))
        }).await;
        match outcome {
            Ok((Ok(id), permit)) => Ok((id, permit)),
            Ok((Err(error), permit)) => Err((error, Some(permit))),
            Err(error) => Err((error, None)),
        }
    }

    async fn private_badge(
        &self,
        auth: RequestAuth,
        mut view: SheetView,
        permit: OwnedSemaphorePermit,
        create: bool,
        history: Option<i64>,
    ) -> Result<(SheetView, OwnedSemaphorePermit), AppError> {
        // Validation happens outside the connection mutex. The final read boundary
        // rechecks its exact resource, grants and readiness together after that gap.
        let canonical = serde_json::to_string(&view.snapshot)?;
        let hash = snapshots::hash(canonical.as_bytes());
        self.run_bounded_auth(move|db|{
            let tx=db.transaction()?;auth.recheck(&tx)?;
            if create {
                let current = campaign(&tx, view.snapshot.campaign_id)?;
                if current.game_id != view.snapshot.game_id
                    || current.edition_id != view.snapshot.edition_id
                    || auth.account_id() != view.snapshot.custodian_id {
                    return Err(AppError::NotFound);
                }
                snapshots::authorize_edit(&tx, &auth, &view.snapshot)?;
                if view.immutable.is_none() && !crate::games::native_available(&tx, &crate::registry::RulesetKey {
                    game_id: current.game_id,
                    edition_id: current.edition_id,
                })? {
                    return Err(AppError::Invalid("Capability unavailable: exact native source is not loaded."));
                }
            } else {
                snapshots::authorize_read(&tx, &auth, view.snapshot.sheet_id)?;
                let current = snapshots::raw_load(&tx, view.snapshot.sheet_id, history)?;
                if !current.corresponds(&view.snapshot, &canonical, &hash)? {
                    return Err(AppError::Conflict);
                }
                view.editable = history.is_none() && match snapshots::authorize_edit(&tx, &auth, &view.snapshot) {
                    Ok(()) => true,
                    Err(AppError::NotFound) => false,
                    Err(error) => return Err(error),
                };
            }
            if let Some(pin)=&view.immutable{
                let status=pin.readiness(&tx)?;
                if status!=WiringStatus::Current{if !view.warning.is_empty(){view.warning.push(' ');}view.warning.push_str("Wiring approval is stale, withdrawn, or evidence is unavailable. Structural interpretation remains available independently of approval; active writes require current grants and supervision. New creation is unavailable.");if create{view.editable=false;}}
            }
            auth.recheck(&tx)?;tx.commit()?;Ok((view,permit))
        }).await
    }
}
