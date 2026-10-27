//! Durable operator accounts and secret-free current HTTP identity reconstruction.
pub(crate) mod browser;
mod crypto;
mod files;

use crate::{AppError, Store};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

const MAX_TIME: i64 = 253_402_300_799;

/// Deliberately secret-free account projection.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Summary {
    pub id: i64,
    pub login: String,
    pub display_name: String,
    pub state: String,
    pub epoch: i64,
    pub owner: bool,
}

/// The refusal text when no Owner binding exists (the launcher maps exactly this to the
/// owner-setup message).
pub const OWNER_BINDING_REQUIRED: &str = "Auth mode requires a durable Owner binding.";

/// Account operations share the application's existing connection manager.
#[derive(Clone)]
pub struct Accounts {
    store: Store,
    clock: Arc<dyn Fn() -> Result<i64, AppError> + Send + Sync>,
    sync_fault: files::SyncFault,
    #[cfg(any(test, feature = "auth-test-support"))]
    work_gate: Option<Arc<dyn Fn() + Send + Sync>>,
    #[cfg(any(test, feature = "auth-test-support"))]
    work_dropped: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Accounts {
    #[cfg(test)]
    pub(crate) fn with_test_clock(
        mut self,
        clock: Arc<dyn Fn() -> Result<i64, AppError> + Send + Sync>,
    ) -> Self {
        self.clock = clock;
        self
    }
    /// Authenticate a canonical login, retaining the KDF permit through final recheck.
    /// # Errors
    /// Returns integrity, storage and nonqueued worker-admission failures.
    pub async fn authenticate(
        &self,
        login: String,
        bytes: Zeroizing<Vec<u8>>,
    ) -> Result<Option<Summary>, AppError> {
        if !valid_login(&login) || crypto::password(&bytes).is_err() {
            return Ok(None);
        }
        let permit = crypto::Permit::acquire()?;
        #[cfg(any(test, feature = "auth-test-support"))]
        let permit = self.configure_permit(permit);
        let (snapshot, permit) = self
            .store
            .run_bounded_auth(move |db| {
                let snapshot = db
                    .query_row(
                        "SELECT id,epoch,state,verifier,EXISTS(SELECT 1 FROM account_owner o WHERE o.account_id=accounts.id) FROM accounts WHERE login=?1",
                        [login],
                        |r| {
                            Ok((
                                r.get::<_, i64>(0)?,
                                r.get::<_, i64>(1)?,
                                r.get::<_, String>(2)?,
                                r.get::<_, Option<String>>(3)?,
                                r.get::<_, bool>(4)?,
                            ))
                        },
                    )
                    .optional()?;
                Ok((snapshot, permit))
            })
            .await?;
        if let Some((_, _, _, Some(hash), _)) = &snapshot {
            crypto::admit(hash)?;
        }
        let enabled = snapshot
            .as_ref()
            .is_some_and(|(_, _, state, _, _)| state == "enabled");
        let target_owner = snapshot.as_ref().is_some_and(|(_, _, _, _, owner)| *owner);
        if crypto::password_for(&bytes, target_owner).is_err() {
            return Ok(None);
        }
        let hash = if enabled {
            snapshot
                .as_ref()
                .and_then(|(_, _, _, h, _)| h.clone())
                .ok_or(AppError::Invalid("Stored account verifier is missing."))?
        } else {
            crypto::DUMMY_PHC.into()
        };
        crypto::admit(&hash)?;
        let (matched, permit) = crypto::verify(bytes, hash, permit).await?;
        self.store
            .run_bounded_auth(move |db| {
                let _permit = permit;
                let Some((id, epoch, _, _, expected_owner)) =
                    snapshot.filter(|_| enabled && matched)
                else {
                    return Ok(None);
                };
                Ok(current(db, id, epoch)?.filter(|account| account.owner == expected_owner))
            })
            .await
    }

    /// Reconstruct identity from current durable state; cached roles are never used.
    /// # Errors
    /// Returns storage or nonqueued worker failures.
    pub async fn current(&self, id: i64, epoch: i64) -> Result<Option<Summary>, AppError> {
        self.store
            .run_bounded_auth(move |db| current(db, id, epoch))
            .await
    }

    /// Require the permanent Owner binding, irrespective of its enabled status.
    /// # Errors
    /// Returns absent-binding or storage errors.
    pub async fn require_owner_binding(&self) -> Result<(), AppError> {
        self.store.run_bounded_auth(|db| {
            if !db.query_row("SELECT EXISTS(SELECT 1 FROM account_owner o JOIN accounts a ON a.id=o.account_id WHERE o.singleton=1)",[],|r|r.get::<_,bool>(0))? { return Err(AppError::Invalid(OWNER_BINDING_REQUIRED)); }
            Ok(())
        }).await
    }
    #[must_use]
    pub fn new(store: Store) -> Self {
        Self {
            store,
            clock: Arc::new(|| {
                let time = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|_| AppError::Invalid("Account clock is invalid."))?;
                i64::try_from(time.as_secs())
                    .map_err(|_| AppError::Invalid("Account clock overflow."))
            }),
            sync_fault: files::SyncFault::None,
            #[cfg(any(test, feature = "auth-test-support"))]
            work_gate: None,
            #[cfg(any(test, feature = "auth-test-support"))]
            work_dropped: None,
        }
    }

    #[cfg(any(test, feature = "auth-test-support"))]
    fn configure_permit(&self, mut permit: crypto::Permit) -> crypto::Permit {
        permit.gate.clone_from(&self.work_gate);
        permit.dropped.clone_from(&self.work_dropped);
        permit
    }

    /// Deterministic test rendezvous around detached password work and permit drop.
    /// Available only with the nonshipping integration-test feature.
    #[cfg(any(test, feature = "auth-test-support"))]
    #[must_use]
    pub fn with_auth_test_hooks(
        mut self,
        gate: Arc<dyn Fn() + Send + Sync>,
        dropped: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        self.work_gate = Some(gate);
        self.work_dropped = Some(dropped);
        self
    }

    fn now(&self) -> Result<i64, AppError> {
        let now = (self.clock)()?;
        if !(1..=MAX_TIME).contains(&now) {
            return Err(AppError::Invalid(
                "Account clock is outside supported bounds.",
            ));
        }
        Ok(now)
    }

    /// Create the sole durable initial Owner binding. No replacement is possible.
    /// # Errors
    /// Rejects existing binding, invalid identity/password, busy KDF or storage failures.
    pub async fn bootstrap(
        &self,
        login: String,
        display: String,
        password_file: &Path,
    ) -> Result<Summary, AppError> {
        identity(&login, &display)?;
        let bytes = files::read(password_file, 512)?;
        crypto::password_for(&bytes, true)?;
        let permit = crypto::Permit::acquire()?;
        #[cfg(any(test, feature = "auth-test-support"))]
        let permit = self.configure_permit(permit);
        self.store.run(|db| absent_owner(db)).await?;
        let (hash, permit) = crypto::hash(bytes, permit).await?;
        let this = self.clone();
        self.store.run(move |db| {
            let _permit = permit;
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            absent_owner(&tx)?;
            let now = this.now()?;
            tx.execute("INSERT INTO accounts(login,display_name,state,epoch,verifier) VALUES(?1,?2,'enabled',1,?3)", params![login,display,hash])?;
            let id = tx.last_insert_rowid();
            tx.execute("INSERT INTO account_owner VALUES(1,?1)", [id])?;
            event(&tx, id, 1, now, "bootstrap")?;
            let result = summary(&tx, id)?;
            tx.commit()?;
            Ok(result)
        }).await
    }

    /// Reserve a login or reissue a pending login's enrollment token to a new file.
    /// # Errors
    /// Requires enabled durable Owner; rejects enrolled login and unsafe/colliding output.
    pub async fn invite(
        &self,
        login: String,
        display: String,
        output: &Path,
    ) -> Result<Summary, AppError> {
        identity(&login, &display)?;
        let output = output.to_owned();
        let this = self.clone();
        self.store
            .run(move |db| {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                owner(&tx)?;
                let (id, epoch, action) = reserve(&tx, &login, &display, None)?;
                let now = this.now()?;
                issue(&tx, id, epoch, "enroll", now, &output, this.sync_fault)?;
                event(&tx, id, epoch, now, action)?;
                let result = summary(&tx, id)?;
                tx.commit()?;
                Ok(result)
            })
            .await
    }

    /// Issue a one-hour reset; immediately advances revocation epoch.
    /// # Errors
    /// Requires enabled Owner/target, matching epoch and durable private output.
    pub async fn issue_reset(
        &self,
        id: i64,
        expected_epoch: i64,
        output: &Path,
    ) -> Result<Summary, AppError> {
        let output = output.to_owned();
        let this = self.clone();
        self.store
            .run(move |db| {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                owner(&tx)?;
                let epoch = reset_target(&tx, id, expected_epoch)?;
                let now = this.now()?;
                issue(&tx, id, epoch, "reset", now, &output, this.sync_fault)?;
                event(&tx, id, epoch, now, "reset_issue")?;
                let result = summary(&tx, id)?;
                tx.commit()?;
                Ok(result)
            })
            .await
    }

    /// Irreversibly disable an enabled account, revoking all credentials.
    /// # Errors
    /// Requires enabled durable Owner, enabled target and exact current epoch.
    pub async fn disable(&self, id: i64, expected_epoch: i64) -> Result<Summary, AppError> {
        let this = self.clone();
        self.store
            .run(move |db| {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                owner(&tx)?;
                let epoch = disable_target(&tx, id, expected_epoch)?;
                event(&tx, id, epoch, this.now()?, "disable")?;
                let result = summary(&tx, id)?;
                tx.commit()?;
                Ok(result)
            })
            .await
    }

    /// Redeem a reserved login's one-time enrollment credential.
    /// # Errors
    /// Rejects invalid/superseded/expired credentials before KDF and again at commit.
    pub async fn enroll(
        &self,
        token_file: &Path,
        password_file: &Path,
    ) -> Result<Summary, AppError> {
        self.redeem("enroll", token_file, password_file).await
    }

    /// Complete a reset without conferring any role or re-enabling an account.
    /// # Errors
    /// Rejects invalid/superseded/expired credentials before KDF and again at commit.
    pub async fn complete_reset(
        &self,
        token_file: &Path,
        password_file: &Path,
    ) -> Result<Summary, AppError> {
        self.redeem("reset", token_file, password_file).await
    }

    async fn redeem(
        &self,
        purpose: &'static str,
        token_file: &Path,
        password_file: &Path,
    ) -> Result<Summary, AppError> {
        let digest = token_digest(token_file)?;
        let bytes = files::read(password_file, 512)?;
        self.redeem_bytes(purpose, digest, bytes, None).await
    }

    pub(crate) async fn redeem_bytes(
        &self,
        purpose: &'static str,
        digest: [u8; 32],
        bytes: Zeroizing<Vec<u8>>,
        admission: Option<crate::auth::CsrfAdmission>,
    ) -> Result<Summary, AppError> {
        crypto::password(&bytes)?;
        let permit = crypto::Permit::acquire()?;
        #[cfg(any(test, feature = "auth-test-support"))]
        let permit = self.configure_permit(permit);
        let this = self.clone();
        let preflight = admission.clone();
        let (id, epoch, target_owner) = self
            .store
            .run_bounded_auth(move |db| {
                let now = if let Some(admission) = preflight {
                    admission.recheck(db)?
                } else {
                    this.now()?
                };
                let (id, epoch) = credential(db, &digest, purpose, now)?;
                Ok((id, epoch, is_owner(db, id)?))
            })
            .await?;
        crypto::password_for(&bytes, target_owner)?;
        let (hash, permit) = crypto::hash(bytes, permit).await?;
        let this = self.clone();
        self.store.run_bounded_auth(move |db| {
            let _permit = permit;
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let now=if let Some(admission)=admission {admission.recheck(&tx)?} else {this.now()?};
            if credential(&tx, &digest, purpose, now)? != (id,epoch) || is_owner(&tx,id)?!=target_owner { return Err(AppError::Conflict); }
            let next = next_epoch(epoch)?;
            if tx.execute("UPDATE accounts SET state='enabled',verifier=?1,epoch=?2 WHERE id=?3 AND epoch=?4 AND state=?5", params![hash,next,id,epoch,if purpose=="enroll" {"pending"} else {"enabled"}])? != 1 { return Err(AppError::Conflict); }
            if tx.execute("UPDATE account_credentials SET state='consumed' WHERE digest=?1 AND state='active'", [digest.as_slice()])? != 1 { return Err(AppError::Conflict); }
            revoke(&tx, id)?;
            event(&tx, id, next, now, if purpose=="enroll" {"enroll"} else {"reset_complete"})?;
            let result = summary(&tx, id)?;
            tx.commit()?;
            Ok(result)
        }).await
    }

    /// Verify a password with a coherent final state/epoch recheck. This is not login.
    /// # Errors
    /// Returns integrity/storage/busy errors; disabled/missing accounts do not match.
    pub async fn verify_password(&self, id: i64, password_file: &Path) -> Result<bool, AppError> {
        let bytes = files::read(password_file, 512)?;
        crypto::password(&bytes)?;
        let permit = crypto::Permit::acquire()?;
        #[cfg(any(test, feature = "auth-test-support"))]
        let permit = self.configure_permit(permit);
        let snapshot = self
            .store
            .run(move |db| {
                Ok(db
                    .query_row(
                        "SELECT epoch,verifier,EXISTS(SELECT 1 FROM account_owner o WHERE o.account_id=accounts.id) FROM accounts WHERE id=?1 AND state='enabled'",
                        [id],
                        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?,r.get::<_,bool>(2)?)),
                    )
                    .optional()?)
            })
            .await?;
        let Some((epoch, hash, target_owner)) = snapshot else {
            return Ok(false);
        };
        crypto::password_for(&bytes, target_owner)?;
        // Admission occurs before scheduling any expensive verification.
        crypto::admit(&hash)?;
        let (matched, permit) = crypto::verify(bytes, hash, permit).await?;
        self.store
            .run(move |db| {
                let _permit = permit;
                let current = db
                    .query_row(
                        "SELECT epoch FROM accounts WHERE id=?1 AND state='enabled'",
                        [id],
                        |r| r.get::<_, i64>(0),
                    )
                    .optional()?;
                Ok(matched && current == Some(epoch) && is_owner(db, id)? == target_owner)
            })
            .await
    }

    /// Show a secret-free projection under local operator authority.
    /// # Errors
    /// Requires enabled durable Owner and an existing target.
    pub async fn show(&self, id: i64) -> Result<Summary, AppError> {
        self.store
            .run(move |db| {
                let tx = db.transaction()?;
                owner(&tx)?;
                summary(&tx, id)
            })
            .await
    }

    /// List at most 100 secret-free summaries after an exclusive ID cursor.
    /// # Errors
    /// Requires enabled durable Owner and nonnegative cursor.
    pub async fn list(&self, after_id: i64) -> Result<Vec<Summary>, AppError> {
        if after_id < 0 {
            return Err(AppError::Invalid("Account cursor must be nonnegative."));
        }
        self.store.run(move |db| {
            let tx = db.transaction()?;
            owner(&tx)?;
            let mut query = tx.prepare("SELECT a.id,a.login,a.display_name,a.state,a.epoch,EXISTS(SELECT 1 FROM account_owner o WHERE o.account_id=a.id) FROM accounts a WHERE a.id>?1 ORDER BY a.id LIMIT 100")?;
            Ok(query.query_map([after_id], read_summary)?.collect::<Result<Vec<_>,_>>()?)
        }).await
    }
}

fn identity(login: &str, display: &str) -> Result<(), AppError> {
    if !valid_login(login) {
        return Err(AppError::Invalid(
            "Login must be 3–64 lowercase ASCII letters/digits/hyphen/underscore, starting alphanumeric.",
        ));
    }
    if !(1..=100).contains(&display.chars().count()) || display.chars().any(char::is_control) {
        return Err(AppError::Invalid(
            "Display name must contain 1–100 non-control scalars.",
        ));
    }
    Ok(())
}

fn reserve(
    db: &Connection,
    login: &str,
    display: &str,
    expected: Option<i64>,
) -> Result<(i64, i64, &'static str), AppError> {
    let existing = db
        .query_row("SELECT id FROM accounts WHERE login=?1", [login], |r| {
            r.get::<_, i64>(0)
        })
        .optional()?;
    if let Some(id) = existing {
        if expected == Some(0) {
            return Err(AppError::Conflict);
        }
        let account = summary(db, id)?;
        if expected.is_some_and(|epoch| epoch != account.epoch) {
            return Err(AppError::Conflict);
        }
        if account.state != "pending" || account.display_name != display {
            return Err(AppError::Invalid(
                "Only the same reserved pending identity can be reissued.",
            ));
        }
        let epoch = next_epoch(account.epoch)?;
        change_epoch(db, id, account.epoch, epoch)?;
        revoke(db, id)?;
        Ok((id, epoch, "reissue"))
    } else {
        if expected.is_some_and(|epoch| epoch != 0) {
            return Err(AppError::Conflict);
        }
        db.execute(
            "INSERT INTO accounts(login,display_name,state,epoch) VALUES(?1,?2,'pending',1)",
            params![login, display],
        )?;
        Ok((db.last_insert_rowid(), 1, "invite"))
    }
}
fn reset_target(db: &Connection, id: i64, expected: i64) -> Result<i64, AppError> {
    let account = enabled(db, id, expected)?;
    let epoch = next_epoch(account.epoch)?;
    change_epoch(db, id, account.epoch, epoch)?;
    revoke(db, id)?;
    Ok(epoch)
}
fn disable_target(db: &Connection, id: i64, expected: i64) -> Result<i64, AppError> {
    enabled(db, id, expected)?;
    let epoch = next_epoch(expected)?;
    if db.execute("UPDATE accounts SET state='disabled',epoch=?1 WHERE id=?2 AND epoch=?3 AND state='enabled'",params![epoch,id,expected])?!=1 {return Err(AppError::Conflict);}
    revoke(db, id)?;
    Ok(epoch)
}

fn valid_login(login: &str) -> bool {
    (3..=64).contains(&login.len())
        && login
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        && login.as_bytes()[0].is_ascii_alphanumeric()
}

fn current(db: &Connection, id: i64, epoch: i64) -> Result<Option<Summary>, AppError> {
    Ok(db.query_row("SELECT a.id,a.login,a.display_name,a.state,a.epoch,EXISTS(SELECT 1 FROM account_owner o WHERE o.account_id=a.id) FROM accounts a WHERE a.id=?1 AND a.epoch=?2 AND a.state='enabled'",params![id,epoch],read_summary).optional()?)
}

fn read_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<Summary> {
    Ok(Summary {
        id: row.get(0)?,
        login: row.get(1)?,
        display_name: row.get(2)?,
        state: row.get(3)?,
        epoch: row.get(4)?,
        owner: row.get(5)?,
    })
}
fn summary(db: &Connection, id: i64) -> Result<Summary, AppError> {
    db.query_row("SELECT a.id,a.login,a.display_name,a.state,a.epoch,EXISTS(SELECT 1 FROM account_owner o WHERE o.account_id=a.id) FROM accounts a WHERE a.id=?1", [id], read_summary).optional()?.ok_or(AppError::Invalid("Account not found."))
}
fn owner(db: &Connection) -> Result<(), AppError> {
    if !db.query_row("SELECT EXISTS(SELECT 1 FROM account_owner o JOIN accounts a ON a.id=o.account_id WHERE o.singleton=1 AND a.state='enabled')", [], |r| r.get::<_,bool>(0))? { return Err(AppError::Invalid("An enabled durable Owner binding is required.")); }
    Ok(())
}

fn is_owner(db: &Connection, id: i64) -> Result<bool, AppError> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM account_owner WHERE account_id=?1)",
        [id],
        |row| row.get(0),
    )?)
}
fn absent_owner(db: &Connection) -> Result<(), AppError> {
    if db.query_row("SELECT EXISTS(SELECT 1 FROM account_owner)", [], |r| {
        r.get::<_, bool>(0)
    })? {
        return Err(AppError::Invalid("Owner bootstrap has already occurred."));
    }
    Ok(())
}
fn enabled(db: &Connection, id: i64, epoch: i64) -> Result<Summary, AppError> {
    let account = summary(db, id)?;
    if account.state != "enabled" || account.epoch != epoch {
        return Err(AppError::Conflict);
    }
    Ok(account)
}
fn next_epoch(epoch: i64) -> Result<i64, AppError> {
    epoch
        .checked_add(1)
        .filter(|e| *e > 0)
        .ok_or(AppError::Invalid("Account epoch exhausted."))
}
fn change_epoch(db: &Connection, id: i64, old: i64, next: i64) -> Result<(), AppError> {
    if db.execute(
        "UPDATE accounts SET epoch=?1 WHERE id=?2 AND epoch=?3",
        params![next, id, old],
    )? != 1
    {
        return Err(AppError::Conflict);
    }
    Ok(())
}
fn revoke(db: &Connection, id: i64) -> Result<(), AppError> {
    db.execute(
        "UPDATE account_credentials SET state='revoked' WHERE account_id=?1 AND state='active'",
        [id],
    )?;
    Ok(())
}
fn event(db: &Connection, id: i64, epoch: i64, now: i64, action: &str) -> Result<(), AppError> {
    let actor = if matches!(action, "enroll" | "reset_complete") {
        "credential_redemption"
    } else {
        "local_operator"
    };
    db.execute("INSERT INTO account_events(account_id,epoch,occurred_at,actor,action) VALUES(?1,?2,?3,?4,?5)", params![id,epoch,now,actor,action])?;
    Ok(())
}
fn issue(
    db: &Connection,
    id: i64,
    epoch: i64,
    purpose: &str,
    now: i64,
    output: &Path,
    fault: files::SyncFault,
) -> Result<(), AppError> {
    issue_to(db, id, epoch, purpose, now, |encoded| {
        files::write(output, encoded, fault)
    })?;
    Ok(())
}
fn issue_to(
    db: &Connection,
    id: i64,
    epoch: i64,
    purpose: &str,
    now: i64,
    deliver: impl FnOnce(&[u8]) -> Result<(), AppError>,
) -> Result<Zeroizing<Vec<u8>>, AppError> {
    let lifetime = if purpose == "enroll" { 86_400 } else { 3600 };
    let expires = now
        .checked_add(lifetime)
        .filter(|t| *t <= MAX_TIME)
        .ok_or(AppError::Invalid("Credential expiry overflow."))?;
    let mut raw = Zeroizing::new([0u8; 32]);
    getrandom::fill(raw.as_mut())
        .map_err(|_| AppError::Invalid("Credential entropy unavailable."))?;
    let digest = Sha256::digest(raw.as_slice());
    let mut encoded = Zeroizing::new([0u8; 64]);
    hex::encode_to_slice(raw.as_slice(), encoded.as_mut())
        .map_err(|_| AppError::Invalid("Credential encoding failed."))?;
    deliver(encoded.as_slice())?;
    db.execute("INSERT INTO account_credentials(digest,account_id,purpose,epoch,issued_at,expires_at,state) VALUES(?1,?2,?3,?4,?5,?6,'active')", params![digest.as_slice(),id,purpose,epoch,now,expires])?;
    Ok(Zeroizing::new(encoded.to_vec()))
}
fn token_digest(path: &Path) -> Result<[u8; 32], AppError> {
    let encoded = files::read(path, 64)?;
    digest_bytes(&encoded)
}
pub(crate) fn digest_bytes(encoded: &[u8]) -> Result<[u8; 32], AppError> {
    if encoded.len() != 64
        || !encoded
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
    {
        return Err(AppError::Invalid(
            "Credential file must contain exactly 64 lowercase hexadecimal bytes.",
        ));
    }
    let mut raw = Zeroizing::new([0u8; 32]);
    hex::decode_to_slice(encoded, raw.as_mut())
        .map_err(|_| AppError::Invalid("Credential encoding is invalid."))?;
    Ok(Sha256::digest(raw.as_slice()).into())
}
fn credential(
    db: &Connection,
    digest: &[u8; 32],
    purpose: &str,
    now: i64,
) -> Result<(i64, i64), AppError> {
    db.query_row("SELECT a.id,a.epoch FROM account_credentials c JOIN accounts a ON a.id=c.account_id AND a.epoch=c.epoch WHERE c.digest=?1 AND c.purpose=?2 AND c.state='active' AND c.issued_at<=?3 AND ?3<c.expires_at AND a.state=?4", params![digest.as_slice(),purpose,now,if purpose=="enroll" {"pending"} else {"enabled"}], |r| Ok((r.get(0)?,r.get(1)?))).optional()?.ok_or(AppError::Invalid("Credential is unavailable, expired or superseded."))
}

#[cfg(test)]
mod tests;
