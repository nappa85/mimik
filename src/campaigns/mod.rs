//! Campaign grants and private sheets are distinct from the public prototype.
mod domain;
mod sheets;
mod snapshots;
#[cfg(test)]
mod tests;
mod transport;
pub(crate) mod values;
pub(crate) mod web;

use crate::{AppError, auth::RequestAuth};
use rusqlite::{Connection, OptionalExtension, params};

pub(crate) use crate::pagination::Listing;

#[derive(Clone)]
pub(crate) struct Campaign {
    pub id: i64,
    pub creator_id: i64,
    pub title: String,
    pub game_id: String,
    pub edition_id: String,
    pub creator: String,
}

pub(crate) struct Member {
    pub account_id: i64,
    pub display: String,
    pub revision: i64,
    pub role: String,
}

pub(crate) struct JoinRequest {
    pub account_id: i64,
    pub display: String,
    pub revision: i64,
}

pub(crate) fn campaign(db: &Connection, id: i64) -> Result<Campaign, AppError> {
    db.query_row("SELECT c.id,c.creator_id,c.title,c.game_id,c.edition_id,a.display_name FROM campaigns c JOIN accounts a ON a.id=c.creator_id WHERE c.id=?1",[id],|r|Ok(Campaign{id:r.get(0)?,creator_id:r.get(1)?,title:r.get(2)?,game_id:r.get(3)?,edition_id:r.get(4)?,creator:r.get(5)?})).optional()?.ok_or(AppError::NotFound)
}

pub(crate) fn role(
    db: &Connection,
    campaign: i64,
    account: i64,
) -> Result<Option<String>, AppError> {
    Ok(db.query_row("SELECT m.role FROM campaign_members m JOIN accounts a ON a.id=m.account_id WHERE m.campaign_id=?1 AND m.account_id=?2 AND m.state='active' AND a.state='enabled'",params![campaign,account],|r|r.get(0)).optional()?)
}

fn master(db: &Connection, campaign: i64, auth: &RequestAuth) -> Result<(), AppError> {
    if role(db, campaign, auth.account_id())?.as_deref() != Some("master") {
        return Err(AppError::NotFound);
    }
    Ok(())
}

fn event(
    db: &Connection,
    campaign: i64,
    auth: &RequestAuth,
    target: i64,
    action: &str,
) -> Result<(), AppError> {
    let now = auth.recheck(db)?;
    db.execute("INSERT INTO campaign_events(campaign_id,actor_id,actor_epoch,target_id,action,occurred_at) VALUES(?1,?2,?3,?4,?5,?6)",params![campaign,auth.account_id(),auth.epoch(),target,action,now])?;
    Ok(())
}

pub(crate) fn name_notes(name: &str, notes: &str) -> Result<String, AppError> {
    let name = name.trim();
    if name.is_empty()
        || name.chars().count() > 100
        || name.contains('\0')
        || notes.chars().count() > 20_000
        || notes.contains('\0')
    {
        return Err(AppError::Invalid(
            "Name must contain 1–100 characters; notes at most 20,000 characters.",
        ));
    }
    Ok(name.into())
}
