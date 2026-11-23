use super::{
    Campaign, JoinRequest, Listing, Member, campaign, event, master, name_notes, role, snapshots,
};
use crate::{AppError, Store, auth::RequestAuth};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

pub(crate) struct Workspace {
    pub campaigns: Vec<Campaign>,
    pub rulesets: Vec<(String, String)>,
    pub next: Option<i64>,
}

#[derive(Clone, Copy, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct DetailPage {
    #[serde(rename = "members_after")]
    pub members: i64,
    #[serde(rename = "requests_after")]
    pub requests: i64,
    #[serde(rename = "versions_after")]
    pub versions: i64,
    #[serde(rename = "sheets_after")]
    pub sheets: i64,
}

/// Inspects the pins a departure validated, before they move into the database worker
/// (tests only).
#[cfg(test)]
pub(super) type DeparturePinsHook = Box<
    dyn FnOnce(&std::collections::BTreeMap<i64, crate::definitions::private_pin::ImmutablePin>)
        + Send,
>;
#[cfg(test)]
pub(super) static DEPARTURE_PINS: std::sync::Mutex<Option<DeparturePinsHook>> =
    std::sync::Mutex::new(None);

impl DetailPage {
    pub(crate) fn valid(self) -> bool {
        [self.members, self.requests, self.versions, self.sheets]
            .into_iter()
            .all(|id| id >= 0)
    }
}

pub(crate) struct Detail {
    pub campaign: Campaign,
    pub role: String,
    pub request_state: String,
    pub members: Vec<Member>,
    pub requests: Vec<JoinRequest>,
    pub versions: Vec<(i64, i64, String)>,
    pub members_next: Option<i64>,
    pub requests_next: Option<i64>,
    pub versions_next: Option<i64>,
}

impl Store {
    pub(crate) async fn workspace(
        &self,
        auth: RequestAuth,
        after: i64,
    ) -> Result<Workspace, AppError> {
        if after < 0 {
            return Err(AppError::Invalid("Cursor must be nonnegative."));
        }
        self.run_campaign(auth.clone(),move |db| {
            let tx=db.transaction()?;
            auth.recheck(&tx)?;
            let campaigns={let mut q=tx.prepare("SELECT c.id,c.creator_id,c.title,c.game_id,c.edition_id,a.display_name FROM campaigns c JOIN accounts a ON a.id=c.creator_id WHERE c.id>?1 ORDER BY c.id LIMIT 51")?;
                q.query_map([after],|r|Ok(Campaign{id:r.get(0)?,creator_id:r.get(1)?,title:r.get(2)?,game_id:r.get(3)?,edition_id:r.get(4)?,creator:r.get(5)?}))?.collect::<Result<Vec<_>,_>>()?};
            let rulesets={let mut q=tx.prepare("SELECT game_id,edition_id FROM registry_editions ORDER BY game_id,edition_id LIMIT 50")?;q.query_map([],|r|Ok((r.get(0)?,r.get(1)?)))?.collect::<Result<Vec<_>,_>>()?};
            auth.recheck(&tx)?;
            tx.commit()?;
            let page=Listing::new(campaigns,|campaign|campaign.id);
            Ok(Workspace{campaigns:page.rows,rulesets,next:page.next})
        }).await
    }

    #[cfg(test)]
    pub(crate) async fn campaign_detail(
        &self,
        auth: RequestAuth,
        id: i64,
        after: i64,
    ) -> Result<Detail, AppError> {
        self.campaign_detail_page(
            auth,
            id,
            DetailPage {
                members: after,
                ..DetailPage::default()
            },
        )
        .await
    }

    pub(crate) async fn campaign_detail_page(
        &self,
        auth: RequestAuth,
        id: i64,
        page: DetailPage,
    ) -> Result<Detail, AppError> {
        if !page.valid() {
            return Err(AppError::Invalid("Cursors must be nonnegative."));
        }
        self.run_campaign(auth.clone(),move |db| {
            let tx=db.transaction()?;
            auth.recheck(&tx)?;
            let campaign=campaign(&tx,id)?;
            let role=role(&tx,id,auth.account_id())?.unwrap_or_default();
            let request_state=tx.query_row("SELECT state FROM campaign_requests WHERE campaign_id=?1 AND account_id=?2",params![id,auth.account_id()],|r|r.get(0)).optional()?.unwrap_or_default();
            let members=if role.is_empty(){Vec::new()}else{
                let mut q=tx.prepare("SELECT m.account_id,a.display_name,m.revision,m.role FROM campaign_members m JOIN accounts a ON a.id=m.account_id WHERE m.campaign_id=?1 AND m.state='active' AND m.account_id>?2 AND NOT EXISTS(SELECT 1 FROM account_owner o WHERE o.account_id=a.id) ORDER BY m.account_id LIMIT 51")?;
                q.query_map(params![id,page.members],|r|Ok(Member{account_id:r.get(0)?,display:r.get(1)?,revision:r.get(2)?,role:r.get(3)?}))?.collect::<Result<Vec<_>,_>>()?};
            let requests=if role=="master"{
                let mut q=tx.prepare("SELECT r.account_id,a.display_name,r.revision FROM campaign_requests r JOIN accounts a ON a.id=r.account_id WHERE r.campaign_id=?1 AND r.state='pending' AND r.account_id>?2 AND NOT EXISTS(SELECT 1 FROM account_owner o WHERE o.account_id=a.id) ORDER BY r.account_id LIMIT 51")?;
                q.query_map(params![id,page.requests],|r|Ok(JoinRequest{account_id:r.get(0)?,display:r.get(1)?,revision:r.get(2)?}))?.collect::<Result<Vec<_>,_>>()?}else{Vec::new()};
            let versions=if role.is_empty(){Vec::new()}else{
                let mut q=tx.prepare("SELECT v.id,v.version,v.definition_id FROM definition_versions v WHERE v.game_id=?1 AND v.edition_id=?2 AND v.id>?3 AND EXISTS(SELECT 1 FROM definition_approvals a WHERE a.version_id=v.id AND a.id=(SELECT max(id) FROM definition_approvals WHERE version_id=v.id) AND NOT EXISTS(SELECT 1 FROM definition_withdrawals w WHERE w.approval_id=a.id)) ORDER BY v.id LIMIT 51")?;
                q.query_map(params![campaign.game_id,campaign.edition_id,page.versions],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?.collect::<Result<Vec<_>,_>>()?};
            auth.recheck(&tx)?;tx.commit()?;
            let members=Listing::new(members,|row|row.account_id);
            let requests=Listing::new(requests,|row|row.account_id);
            let versions=Listing::new(versions,|row|row.0);
            Ok(Detail{campaign,role,request_state,members:members.rows,requests:requests.rows,versions:versions.rows,members_next:members.next,requests_next:requests.next,versions_next:versions.next})
        }).await
    }

    pub(crate) async fn create_campaign(
        &self,
        auth: RequestAuth,
        title: String,
        game: String,
        edition: String,
    ) -> Result<i64, AppError> {
        let title = name_notes(&title, "")?;
        self.run_campaign(auth.clone(),move |db| {
            let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            auth.recheck(&tx)?;
            let id:i64=tx.query_row("SELECT coalesce(max(id),0) FROM campaigns",[],|r|r.get(0))?;
            let total:i64=tx.query_row("SELECT count(*) FROM campaigns",[],|r|r.get(0))?;
            let created:i64=tx.query_row("SELECT count(*) FROM campaigns WHERE creator_id=?1",[auth.account_id()],|r|r.get(0))?;
            if total>=128 || created>=32{return Err(AppError::Invalid("Campaign creation capacity reached."));}
            let id=id.checked_add(1).ok_or(AppError::Conflict)?;
            tx.execute("INSERT INTO campaigns(id,creator_id,game_id,edition_id,title,revision) VALUES(?1,?2,?3,?4,?5,1)",params![id,auth.account_id(),game,edition,title])?;
            tx.execute("INSERT INTO campaign_members VALUES(?1,?2,1,'active','master')",params![id,auth.account_id()])?;
            event(&tx,id,&auth,auth.account_id(),"create")?;
            tx.commit()?;Ok(id)
        }).await
    }

    pub(crate) async fn request_join(&self, auth: RequestAuth, id: i64) -> Result<(), AppError> {
        self.run_campaign(auth.clone(),move |db| {
            let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            auth.recheck(&tx)?;campaign(&tx,id)?;
            if role(&tx,id,auth.account_id())?.is_some(){return Err(AppError::Conflict);}
            let old:Option<(i64,String)>=tx.query_row("SELECT revision,state FROM campaign_requests WHERE campaign_id=?1 AND account_id=?2",params![id,auth.account_id()],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            if let Some((revision,state))=old {
                if state=="pending" {return Err(AppError::Conflict);}
                let next=revision.checked_add(1).ok_or(AppError::Conflict)?;
                tx.execute("UPDATE campaign_requests SET revision=?3,state='pending' WHERE campaign_id=?1 AND account_id=?2",params![id,auth.account_id(),next])?;
            }else{tx.execute("INSERT INTO campaign_requests VALUES(?1,?2,1,'pending')",params![id,auth.account_id()])?;}
            event(&tx,id,&auth,auth.account_id(),"request")?;tx.commit()?;Ok(())
        }).await
    }

    pub(crate) async fn decide_join(
        &self,
        auth: RequestAuth,
        id: i64,
        target: i64,
        revision: i64,
        accept: bool,
    ) -> Result<(), AppError> {
        self.run_campaign(auth.clone(),move |db| {
            let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            auth.recheck(&tx)?;master(&tx,id,&auth)?;
            if accept && tx.query_row("SELECT EXISTS(SELECT 1 FROM account_owner WHERE account_id=?1)",[target],|row|row.get::<_,bool>(0))? {
                return Err(AppError::Invalid("Owner is administration-only; enroll a separate player account."));
            }
            let next=revision.checked_add(1).ok_or(AppError::Conflict)?;
            if tx.execute("UPDATE campaign_requests SET revision=?4,state=?5 WHERE campaign_id=?1 AND account_id=?2 AND revision=?3 AND state='pending'",params![id,target,revision,next,if accept{"accepted"}else{"declined"}])?!=1{return Err(AppError::Conflict);}
            if accept{
                let old:Option<(i64,String)>=tx.query_row("SELECT revision,state FROM campaign_members WHERE campaign_id=?1 AND account_id=?2",params![id,target],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
                if let Some((revision,state))=old {
                    if state!="inactive"{return Err(AppError::Conflict);}
                    let next=revision.checked_add(1).ok_or(AppError::Conflict)?;
                    tx.execute("UPDATE campaign_members SET revision=?3,state='active',role='player' WHERE campaign_id=?1 AND account_id=?2",params![id,target,next])?;
                }else{tx.execute("INSERT INTO campaign_members VALUES(?1,?2,1,'active','player')",params![id,target])?;}
            }
            event(&tx,id,&auth,target,if accept{"accept"}else{"decline"})?;tx.commit()?;Ok(())
        }).await
    }

    pub(crate) async fn change_role(
        &self,
        auth: RequestAuth,
        id: i64,
        target: i64,
        revision: i64,
        promote: bool,
    ) -> Result<(), AppError> {
        self.run_campaign(auth.clone(),move |db| {
            let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            auth.recheck(&tx)?;
            if campaign(&tx,id)?.creator_id!=auth.account_id() || target==auth.account_id(){return Err(AppError::NotFound);}
            master(&tx,id,&auth)?;
            let next=revision.checked_add(1).ok_or(AppError::Conflict)?;
            if tx.execute("UPDATE campaign_members SET revision=?4,role=?5 WHERE campaign_id=?1 AND account_id=?2 AND revision=?3 AND state='active' AND role=?6",params![id,target,revision,next,if promote{"master"}else{"player"},if promote{"player"}else{"master"}])?!=1{return Err(AppError::Conflict);}
            event(&tx,id,&auth,target,if promote{"promote"}else{"demote"})?;tx.commit()?;Ok(())
        }).await
    }

    pub(crate) async fn depart(
        &self,
        auth: RequestAuth,
        id: i64,
        target: i64,
        revision: i64,
    ) -> Result<(), AppError> {
        // The runtime permit remains worker-owned through all streamed freezes.
        let permit = crate::definitions::admit().map_err(|_| AppError::AuthStorageBusy)?;
        let preflight = auth.clone();
        let (raw,permit)=self.run_bounded_auth(move|db|{
            let tx=db.transaction()?;preflight.recheck(&tx)?;
            if target!=preflight.account_id(){master(&tx,id,&preflight)?;}
            let versions={let mut q=tx.prepare("SELECT DISTINCT json_extract(pin,'$.version_id') FROM private_sheets WHERE campaign_id=?1 AND custodian_id=?2 AND state='active' AND kind='runtime' LIMIT 33")?;q.query_map(params![id,target],|r|r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?};
            if versions.len()>32{return Err(AppError::Invalid("Private sheet capacity corruption."));}
            let raw=versions.into_iter().map(|version|crate::definitions::private_pin::load(&tx,version)).collect::<Result<Vec<_>,_>>()?;
            preflight.recheck(&tx)?;tx.commit()?;Ok((raw,permit))
        }).await?;
        let (pins, permit) = tokio::task::spawn_blocking(move || {
            let pins = raw
                .into_iter()
                .map(|raw| {
                    crate::definitions::private_pin::validate_for_write(raw)
                        .map(|pin| (pin.raw.id, pin))
                })
                .collect::<Result<std::collections::BTreeMap<_, _>, _>>()?;
            Ok::<_, AppError>((pins, permit))
        })
        .await
        .map_err(|_| AppError::Worker)??;
        #[cfg(test)]
        if let Some(hook) = DEPARTURE_PINS.lock().unwrap().take() {
            hook(&pins);
        }
        self.run_campaign(auth.clone(),move |db| {
            let _permit=permit;
            let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            auth.recheck(&tx)?;
            if campaign(&tx,id)?.creator_id==target{return Err(AppError::NotFound);}
            if target!=auth.account_id(){master(&tx,id,&auth)?;let target_role:Option<String>=tx.query_row("SELECT role FROM campaign_members WHERE campaign_id=?1 AND account_id=?2 AND state='active'",params![id,target],|r|r.get(0)).optional()?;if target_role.as_deref()!=Some("player"){return Err(AppError::NotFound);}}
            let next=revision.checked_add(1).ok_or(AppError::Conflict)?;
            if tx.execute("UPDATE campaign_members SET revision=?4,state='inactive' WHERE campaign_id=?1 AND account_id=?2 AND revision=?3 AND state='active'",params![id,target,revision,next])?!=1{return Err(AppError::Conflict);}
            let ids={let mut q=tx.prepare("SELECT id FROM private_sheets WHERE campaign_id=?1 AND custodian_id=?2 AND state='active' ORDER BY id LIMIT 33")?;q.query_map(params![id,target],|r|r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?};
            if ids.len()>32{return Err(AppError::Invalid("Corrupt private sheet capacity."));}
            for sheet in ids {
                let mut saved=snapshots::load(&tx,sheet)?;
                let snapshots::Pin::Runtime{version_id,..}=saved.pin;let pin=Some(pins.get(&version_id).ok_or(AppError::Conflict)?);
                saved.state="frozen".into();
                snapshots::write(&tx,&auth,saved,"freeze",pin)?;
            }
            event(&tx,id,&auth,target,if target==auth.account_id(){"leave"}else{"remove"})?;tx.commit()?;Ok(())
        }).await
    }
}
