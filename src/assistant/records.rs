//! The owner's model setting in the database (migration 022; `docs/model-runtime.md`, "Owner model setting"):
//! the models the owner switched to with their trial measurements, and which one is active
//! and which previous. The runtime manager is the only writer: it stores a switch before it
//! applies it.
use super::runtime::models::Model;
use crate::store::Store;
use async_trait::async_trait;
use rusqlite::{OptionalExtension, params};

/// A model as stored: the model and the trial that measured it (JSON, absent for a model
/// the start flags named).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Record {
    pub model: Model,
    pub trial: Option<String>,
}

impl Record {
    /// Whether the record's trial measured the model with a prompt other than the current one
    /// (ASSIST-56): `max_tokens` and the speeds of such a model predate the prompt in force.
    /// A record without a trial (a model the start flags named) is not flagged.
    pub(crate) fn older_prompt(&self) -> bool {
        self.trial.as_deref().is_some_and(|trial| {
            serde_json::from_str::<serde_json::Value>(trial)
                .ok()
                .and_then(|trial| trial["prefix_sha256"].as_str().map(str::to_owned))
                .as_deref()
                != Some(super::contract::prefix_sha256())
        })
    }
}

/// The stored choice.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Stored {
    pub active: Option<Record>,
    pub previous: Option<Record>,
}

impl Stored {
    /// Whether the active and the previous model were measured with an older prompt.
    pub(crate) fn older_prompt(&self) -> (bool, bool) {
        let older = |record: &Option<Record>| record.as_ref().is_some_and(Record::older_prompt);
        (older(&self.active), older(&self.previous))
    }
}

#[async_trait]
pub(crate) trait Records: Send + Sync {
    /// Makes `new` the active model. `old` becomes the previous model (`None` leaves the
    /// previous model as it is), all in one transaction; `by` is the owner's account.
    async fn activate(&self, new: &Record, old: Option<&Record>, by: i64) -> Result<(), String>;
    /// Swaps the active and the previous model.
    async fn swap(&self, by: i64) -> Result<(), String>;
}

/// Drops migration 022's tables (tests that rewind the schema).
#[cfg(test)]
pub(crate) const DROP_SCHEMA: &str =
    "DROP TABLE assistant_model_choice; DROP TABLE assistant_models;";

/// The records in the application's database.
#[derive(Clone)]
pub(crate) struct SqlRecords(pub Store);

const SELECT: &str = "SELECT m.spec,m.file_name,m.sha256,m.size,m.alias,m.max_tokens,m.trial FROM assistant_models m";

fn record(row: &rusqlite::Row<'_>) -> rusqlite::Result<Record> {
    let size: Option<i64> = row.get(3)?;
    let max_tokens: i64 = row.get(5)?;
    Ok(Record {
        model: Model {
            spec: row.get(0)?,
            name: row.get(1)?,
            sha256: row.get(2)?,
            size: size.and_then(|n| u64::try_from(n).ok()),
            alias: row.get(4)?,
            max_tokens: u32::try_from(max_tokens).unwrap_or(0),
        },
        trial: row.get(6)?,
    })
}

fn insert(db: &rusqlite::Connection, record: &Record) -> rusqlite::Result<i64> {
    let size = record.model.size.and_then(|n| i64::try_from(n).ok());
    db.execute(
        "INSERT INTO assistant_models(spec,file_name,sha256,size,alias,max_tokens,trial,recorded_at) VALUES(?1,?2,?3,?4,?5,?6,?7,mimik_auth_now())",
        params![
            record.model.spec,
            record.model.name,
            record.model.sha256,
            size,
            record.model.alias,
            record.model.max_tokens,
            record.trial,
        ],
    )?;
    Ok(db.last_insert_rowid())
}

/// The row of a model with the same file, hash and `max_tokens` (a model the owner switched
/// away from keeps its row), or a new one.
fn find_or_insert(db: &rusqlite::Connection, record: &Record) -> rusqlite::Result<i64> {
    let found: Option<i64> = db
        .query_row(
            "SELECT id FROM assistant_models WHERE file_name=?1 AND sha256=?2 AND max_tokens=?3 AND alias=?4 ORDER BY id DESC LIMIT 1",
            params![
                record.model.name,
                record.model.sha256,
                record.model.max_tokens,
                record.model.alias
            ],
            |row| row.get(0),
        )
        .optional()?;
    match found {
        Some(id) => Ok(id),
        None => insert(db, record),
    }
}

impl SqlRecords {
    /// The stored choice, if any.
    ///
    /// # Errors
    ///
    /// A storage failure, by message.
    pub(crate) async fn load(&self) -> Result<Stored, String> {
        self.0
            .run(|db| {
                let by_id = |id: Option<i64>| -> rusqlite::Result<Option<Record>> {
                    id.map_or(Ok(None), |id| {
                        db.query_row(&format!("{SELECT} WHERE m.id=?1"), [id], record)
                            .optional()
                    })
                };
                let choice: Option<(i64, Option<i64>)> = db
                    .query_row(
                        "SELECT active_model,previous_model FROM assistant_model_choice WHERE id=1",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?;
                Ok(match choice {
                    None => Stored::default(),
                    Some((active, previous)) => Stored {
                        active: by_id(Some(active))?,
                        previous: by_id(previous)?,
                    },
                })
            })
            .await
            .map_err(|_| "the model setting cannot be read".to_owned())
    }
}

#[async_trait]
impl Records for SqlRecords {
    async fn activate(&self, new: &Record, old: Option<&Record>, by: i64) -> Result<(), String> {
        let (new, old) = (new.clone(), old.cloned());
        self.0
            .run(move |db| {
                let tx = db.transaction()?;
                let old_id = old.as_ref().map(|old| find_or_insert(&tx, old)).transpose()?;
                let new_id = insert(&tx, &new)?;
                // `COALESCE` keeps the previous model when none is given.
                tx.execute(
                    "INSERT INTO assistant_model_choice(id,active_model,previous_model,changed_by,changed_at) VALUES(1,?1,?2,?3,mimik_auth_now())
                     ON CONFLICT(id) DO UPDATE SET active_model=excluded.active_model,previous_model=COALESCE(?2,previous_model),changed_by=excluded.changed_by,changed_at=excluded.changed_at",
                    params![new_id, old_id, by],
                )?;
                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(|_| "the model setting cannot be stored".to_owned())
    }

    async fn swap(&self, by: i64) -> Result<(), String> {
        let changed = self
            .0
            .run(move |db| {
                Ok(db.execute(
                    "UPDATE assistant_model_choice SET active_model=previous_model,previous_model=active_model,changed_by=?1,changed_at=mimik_auth_now() WHERE id=1 AND previous_model IS NOT NULL",
                    [by],
                )?)
            })
            .await
            .map_err(|_| "the model setting cannot be stored".to_owned())?;
        if changed == 1 {
            Ok(())
        } else {
            Err("there is no previous model".into())
        }
    }
}

#[cfg(test)]
#[path = "records_tests.rs"]
pub(crate) mod tests;
