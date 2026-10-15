use crate::{AppError, Store};
use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fmt};
use time::OffsetDateTime;
use tower_sessions::{
    SessionStore,
    session::{Id, Record},
    session_store,
};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Principal {
    pub id: i64,
    pub epoch: i64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    csrf: String,
    #[serde(
        default,
        deserialize_with = "present_principal",
        skip_serializing_if = "Option::is_none"
    )]
    principal: Option<Principal>,
}

// Missing is anonymous; a present key must deserialize as the closed non-null
// Principal object. Option's default decoder would normalize JSON null away.
fn present_principal<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Principal>, D::Error> {
    Principal::deserialize(deserializer).map(Some)
}

fn invalid() -> AppError {
    AppError::Invalid("Invalid session record.")
}

fn encode(record: &Record) -> Result<(String, Option<Principal>), AppError> {
    if record.expiry_date.nanosecond() != 0 || record.data.len() > 2 {
        return Err(invalid());
    }
    let csrf = record
        .data
        .get("csrf")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(invalid)?;
    if csrf.len() != 64
        || !csrf
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid());
    }
    // Bound and admit scalars before any clone/serialization expansion.
    let principal = if let Some(value) = record.data.get("principal") {
        let object = value.as_object().ok_or_else(invalid)?;
        if object.len() != 2 {
            return Err(invalid());
        }
        let id = object
            .get("id")
            .and_then(serde_json::Value::as_i64)
            .filter(|v| *v > 0)
            .ok_or_else(invalid)?;
        let epoch = object
            .get("epoch")
            .and_then(serde_json::Value::as_i64)
            .filter(|v| *v > 0)
            .ok_or_else(invalid)?;
        Some(Principal { id, epoch })
    } else {
        None
    };
    if record.data.len() != 1 + usize::from(principal.is_some()) {
        return Err(invalid());
    }
    Ok((
        serde_json::to_string(&Payload {
            csrf: csrf.into(),
            principal,
        })?,
        principal,
    ))
}

pub(crate) fn now(db: &Connection) -> Result<i64, AppError> {
    Ok(db.query_row("SELECT mimik_auth_now()", [], |row| row.get(0))?)
}

fn cleanup(db: &Connection) -> Result<usize, AppError> {
    now(db)?;
    Ok(db.execute("DELETE FROM auth_sessions WHERE id IN (SELECT id FROM auth_sessions WHERE expires_at<=mimik_auth_now() ORDER BY expires_at,id LIMIT 100)", [])?)
}

/// `SQLite` persistence for library sessions, with immutable bindings and tombstones.
#[derive(Clone)]
pub struct SqliteSessions {
    pub(crate) store: Store,
    #[cfg(test)]
    pub(super) candidate: Option<std::sync::Arc<dyn Fn() -> Id + Send + Sync>>,
    #[cfg(any(test, feature = "auth-test-support"))]
    read_hook: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
}

impl fmt::Debug for SqliteSessions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SqliteSessions")
    }
}

impl SqliteSessions {
    #[must_use]
    pub fn new(store: Store) -> Self {
        Self {
            store,
            #[cfg(test)]
            candidate: None,
            #[cfg(any(test, feature = "auth-test-support"))]
            read_hook: None,
        }
    }

    /// Rendezvous immediately before the row query, for deterministic synthetic tests.
    #[cfg(any(test, feature = "auth-test-support"))]
    #[must_use]
    pub fn with_read_test_hook(mut self, hook: std::sync::Arc<dyn Fn() + Send + Sync>) -> Self {
        self.read_hook = Some(hook);
        self
    }

    /// Remove at most 100 expired rows. No task is started by construction.
    /// # Errors
    /// Returns bounded-worker, clock or database failures.
    pub async fn cleanup(&self) -> Result<usize, AppError> {
        self.store
            .run_bounded_auth(|db| {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let removed = cleanup(&tx)?;
                tx.commit()?;
                Ok(removed)
            })
            .await
    }
}

fn backend(error: &AppError) -> session_store::Error {
    if matches!(error, AppError::AuthStorageBusy) {
        super::mark_storage_busy();
        session_store::Error::Backend("Auth storage worker is busy.".into())
    } else {
        session_store::Error::Backend("Session storage failed.".into())
    }
}

#[async_trait]
impl SessionStore for SqliteSessions {
    async fn create(&self, record: &mut Record) -> session_store::Result<()> {
        let (payload, principal) = encode(record).map_err(|error| backend(&error))?;
        let deadline = record.expiry_date.unix_timestamp();
        let first = record.id;
        #[cfg(test)]
        let candidate = self.candidate.clone();
        let id = self.store.run_bounded_auth(move |db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            cleanup(&tx)?;
            let issued = now(&tx)?;
            let mut id = first;
            for attempt in 0..8 {
                let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM auth_sessions WHERE id=?1)", [id.to_string()], |r| r.get(0))?;
                if !exists {
                    tx.execute("INSERT INTO auth_sessions(id,issued_at,expires_at,state,payload,account_id,epoch) VALUES(?1,?2,?3,'active',?4,?5,?6)", params![id.to_string(),issued,deadline,payload,principal.map(|p| p.id),principal.map(|p| p.epoch)])?;
                    tx.commit()?;
                    return Ok(id);
                }
                if attempt<7 {
                    id = Id::default();
                    #[cfg(test)]
                    if let Some(candidate)=&candidate {id=candidate();}
                }
            }
            Err(AppError::Invalid("Session collision limit reached."))
        }).await.map_err(|error| backend(&error))?;
        record.id = id;
        Ok(())
    }

    async fn save(&self, record: &Record) -> session_store::Result<()> {
        let (payload, principal) = encode(record).map_err(|error| backend(&error))?;
        let id = record.id.to_string();
        let deadline = record.expiry_date.unix_timestamp();
        self.store.run_bounded_auth(move |db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if tx.execute("UPDATE auth_sessions SET payload=payload WHERE id=?1 AND state='active' AND expires_at=?2 AND expires_at>mimik_auth_now() AND payload=?3 AND account_id IS ?4 AND epoch IS ?5", params![id,deadline,payload,principal.map(|p| p.id),principal.map(|p| p.epoch)])? != 1 { return Err(invalid()); }
            tx.commit()?;
            Ok(())
        }).await.map_err(|error| backend(&error))
    }

    async fn load(&self, session_id: &Id) -> session_store::Result<Option<Record>> {
        let id = *session_id;
        #[cfg(any(test, feature = "auth-test-support"))]
        let read_hook = self.read_hook.clone();
        let loaded = self.store.run_bounded_auth(move |db| {
            #[cfg(any(test, feature = "auth-test-support"))]
            if let Some(hook) = read_hook { hook(); }
            let row = db.query_row("SELECT id,issued_at,expires_at,state,CAST(substr(CAST(payload AS BLOB),1,4097) AS TEXT),account_id,epoch FROM auth_sessions WHERE id=?1", [id.to_string()], |r| Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?,r.get::<_,Option<String>>(4)?,r.get::<_,Option<i64>>(5)?,r.get::<_,Option<i64>>(6)?))).optional()?;
            // Row queries may wait on SQLite locks: time sampled before fetching
            // cannot admit the fetched session. Equality is expired.
            let clock = now(db)?;
            let Some((stored_id,issued,deadline,state,payload,account,epoch)) = row else { return Ok(None); };
            if stored_id.parse::<Id>().map_err(|_| invalid())? != id || issued<1 || deadline>253_402_300_799 || deadline<=issued || deadline-issued>if account.is_some(){28800}else{600} || account.is_some()!=epoch.is_some() || account.is_some_and(|v|v<=0) || epoch.is_some_and(|v|v<=0) { return Err(invalid()); }
            if state=="revoked" { return if payload.is_none(){Ok(None)}else{Err(invalid())}; }
            if state!="active" { return Err(invalid()); }
            let payload = payload.ok_or_else(invalid)?;
            if payload.len()>4096 { return Err(invalid()); }
            let decoded: Payload = serde_json::from_str(&payload)?;
            if decoded.principal != account.zip(epoch).map(|(id,epoch)|Principal{id,epoch}) { return Err(invalid()); }
            let mut data = HashMap::new();
            data.insert("csrf".into(), serde_json::Value::String(decoded.csrf));
            if let Some(principal)=decoded.principal { data.insert("principal".into(),serde_json::to_value(principal)?); }
            let record = Record { id, data, expiry_date: OffsetDateTime::from_unix_timestamp(deadline).map_err(|_|invalid())? };
            let _ = encode(&record)?;
            if deadline<=clock { return Ok(None); }
            Ok(Some(record))
        }).await.map_err(|error| backend(&error))?;
        if let Some(record) = &loaded {
            super::remember_session_read(record);
        }
        Ok(loaded)
    }

    async fn delete(&self, session_id: &Id) -> session_store::Result<()> {
        let id = session_id.to_string();
        self.store.run_bounded_auth(move |db| {
            // Clock failure also prevents revocation; never claim a successful logout.
            now(db)?;
            db.execute("UPDATE auth_sessions SET state='revoked',payload=NULL WHERE id=?1 AND state='active'",[id])?;
            Ok(())
        }).await.map_err(|error| backend(&error))
    }
}
