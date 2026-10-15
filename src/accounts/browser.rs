//! Authenticated browser transport shares operator transitions, not OS authority.
use super::{
    Accounts, Summary, disable_target, event, identity, is_owner, issue_to, read_summary, reserve,
    reset_target, summary,
};
use crate::{
    AppError,
    auth::{RequestAuth, with_domain_request},
    pagination::Listing,
};
use rusqlite::TransactionBehavior;
use zeroize::Zeroizing;

pub(crate) struct Issued {
    pub account: Summary,
    pub token: Zeroizing<Vec<u8>>,
    pub expires: i64,
    pub purpose: &'static str,
}
impl Accounts {
    pub(crate) async fn browser_list(
        &self,
        auth: RequestAuth,
        after: i64,
    ) -> Result<Listing<Summary>, AppError> {
        if after < 0 {
            return Err(AppError::Invalid("Account cursor must be nonnegative."));
        }
        self.store.run_bounded_auth(move|db|{
            let tx=db.transaction()?;auth.recheck_owner(&tx)?;
            let rows={let mut q=tx.prepare("SELECT a.id,a.login,a.display_name,a.state,a.epoch,EXISTS(SELECT 1 FROM account_owner o WHERE o.account_id=a.id) FROM accounts a WHERE a.id>?1 ORDER BY a.id LIMIT 51")?;q.query_map([after],read_summary)?.collect::<Result<Vec<_>,_>>()?};
            auth.recheck_owner(&tx)?;tx.commit()?;Ok(Listing::new(rows,|row|row.id))
        }).await
    }
    pub(crate) async fn browser_invite(
        &self,
        auth: RequestAuth,
        login: String,
        display: String,
        expected: i64,
    ) -> Result<Issued, AppError> {
        identity(&login, &display)?;
        self.store
            .run_bounded_auth(move |db| {
                with_domain_request(auth.clone(), || {
                    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    let now = auth.recheck_owner(&tx)?;
                    let (id, epoch, action) = reserve(&tx, &login, &display, Some(expected))?;
                    let token = issue_to(&tx, id, epoch, "enroll", now, |_| Ok(()))?;
                    browser_event(&tx, &auth, id, epoch, now, action)?;
                    let account = summary(&tx, id)?;
                    tx.commit()?;
                    Ok(Issued {
                        account,
                        token,
                        expires: now + 86400,
                        purpose: "enroll",
                    })
                })
            })
            .await
    }
    pub(crate) async fn browser_reset(
        &self,
        auth: RequestAuth,
        id: i64,
        expected: i64,
    ) -> Result<Issued, AppError> {
        self.store
            .run_bounded_auth(move |db| {
                with_domain_request(auth.clone(), || {
                    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    let now = auth.recheck_owner(&tx)?;
                    if is_owner(&tx, id)? {
                        return Err(AppError::Invalid(
                            "Owner reset issuance requires the local CLI.",
                        ));
                    }
                    let epoch = reset_target(&tx, id, expected)?;
                    let token = issue_to(&tx, id, epoch, "reset", now, |_| Ok(()))?;
                    browser_event(&tx, &auth, id, epoch, now, "reset_issue")?;
                    let account = summary(&tx, id)?;
                    tx.commit()?;
                    Ok(Issued {
                        account,
                        token,
                        expires: now + 3600,
                        purpose: "reset",
                    })
                })
            })
            .await
    }
    pub(crate) async fn browser_disable(
        &self,
        auth: RequestAuth,
        id: i64,
        expected: i64,
    ) -> Result<(), AppError> {
        self.store
            .run_bounded_auth(move |db| {
                with_domain_request(auth.clone(), || {
                    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    let now = auth.recheck_owner(&tx)?;
                    if is_owner(&tx, id)? {
                        return Err(AppError::Invalid(
                            "Owner disablement requires the local CLI.",
                        ));
                    }
                    let epoch = disable_target(&tx, id, expected)?;
                    browser_event(&tx, &auth, id, epoch, now, "disable")?;
                    tx.commit()?;
                    Ok(())
                })
            })
            .await
    }
}
fn browser_event(
    db: &rusqlite::Connection,
    auth: &RequestAuth,
    id: i64,
    epoch: i64,
    now: i64,
    action: &str,
) -> Result<(), AppError> {
    event(db, id, epoch, now, action)?;
    let event_id = db.last_insert_rowid();
    db.execute("INSERT INTO account_web_events(event_id,actor_id,actor_epoch,session_digest) VALUES(?1,?2,?3,?4)",rusqlite::params![event_id,auth.account_id(),auth.epoch(),auth.digest()])?;
    Ok(())
}
