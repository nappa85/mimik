use super::*;
use crate::assistant::runtime::models::OlderPrompt;
use std::sync::Mutex;

/// Records in memory for the manager's tests: what was stored, and optionally a failure.
#[derive(Default)]
pub(crate) struct MemRecords {
    pub log: Mutex<Vec<String>>,
    pub fail: Mutex<Option<String>>,
    pub stored: Mutex<Stored>,
    /// While set, `set_length` waits for a permit (a slow store).
    pub set_gate: Mutex<Option<std::sync::Arc<tokio::sync::Semaphore>>>,
    /// The reply length stored per file hash.
    pub lengths: Mutex<std::collections::HashMap<String, u32>>,
    /// The reminder setting (`None` until set).
    pub reminders: Mutex<Option<bool>>,
}

#[async_trait]
impl Records for MemRecords {
    async fn activate(&self, new: &Record, old: Option<&Record>, by: i64) -> Result<(), String> {
        if let Some(why) = self.fail.lock().unwrap().clone() {
            return Err(why);
        }
        self.log.lock().unwrap().push(format!(
            "activate {} (previous {}) by {by}",
            new.model.spec,
            old.map_or("kept", |old| old.model.spec.as_str())
        ));
        let mut stored = self.stored.lock().unwrap();
        if let Some(old) = old {
            stored.previous = Some(old.clone());
        }
        stored.active = Some(new.clone());
        Ok(())
    }

    async fn length(&self, sha256: &str) -> Result<Option<u32>, String> {
        if let Some(why) = self.fail.lock().unwrap().clone() {
            return Err(why);
        }
        Ok(self.lengths.lock().unwrap().get(sha256).copied())
    }

    async fn set_length(
        &self,
        sha256: &str,
        max_tokens: Option<u32>,
        by: i64,
    ) -> Result<(), String> {
        let gate = self.set_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.acquire().await.unwrap().forget();
        }
        if let Some(why) = self.fail.lock().unwrap().clone() {
            return Err(why);
        }
        self.log.lock().unwrap().push(format!(
            "set_length {} by {by}",
            max_tokens.map_or_else(|| "none".to_owned(), |n| n.to_string())
        ));
        let mut lengths = self.lengths.lock().unwrap();
        match max_tokens {
            Some(tokens) => lengths.insert(sha256.to_owned(), tokens),
            None => lengths.remove(sha256),
        };
        Ok(())
    }

    async fn reminders(&self) -> Result<Option<bool>, String> {
        if let Some(why) = self.fail.lock().unwrap().clone() {
            return Err(why);
        }
        Ok(*self.reminders.lock().unwrap())
    }

    async fn set_reminders(&self, enabled: bool, by: i64) -> Result<(), String> {
        if let Some(why) = self.fail.lock().unwrap().clone() {
            return Err(why);
        }
        self.log
            .lock()
            .unwrap()
            .push(format!("set_reminders {enabled} by {by}"));
        *self.reminders.lock().unwrap() = Some(enabled);
        Ok(())
    }

    async fn swap(&self, by: i64) -> Result<(), String> {
        if let Some(why) = self.fail.lock().unwrap().clone() {
            return Err(why);
        }
        self.log.lock().unwrap().push(format!("swap by {by}"));
        let mut stored = self.stored.lock().unwrap();
        let stored = &mut *stored;
        std::mem::swap(&mut stored.active, &mut stored.previous);
        Ok(())
    }
}

fn model(name: &str, sha: char, max_tokens: u32) -> Model {
    Model {
        spec: format!("local:{name}"),
        name: name.to_owned(),
        sha256: sha.to_string().repeat(64),
        alias: name.trim_end_matches(".gguf").to_ascii_lowercase(),
        max_tokens,
        size: Some(1_000_000),
    }
}

fn record(name: &str, sha: char, max_tokens: u32, trial: Option<&str>) -> Record {
    Record {
        model: model(name, sha, max_tokens),
        trial: trial.map(str::to_owned),
    }
}

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("mimik.sqlite3")).unwrap();
    (dir, store)
}

async fn count(store: &Store, table: &'static str) -> i64 {
    store
        .run(
            move |db| Ok(db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn nothing_is_stored_until_the_owner_switches() {
    let (_dir, store) = store();
    assert_eq!(SqlRecords(store).load().await, Ok(Stored::default()));
}

#[tokio::test]
async fn a_switch_stores_the_new_model_with_its_trial_and_the_old_one_as_previous() {
    let (_dir, store) = store();
    let records = SqlRecords(store.clone());
    let default = record("granite.gguf", 'e', 1024, None);
    let qwen = record("qwen.gguf", 'a', 1024, Some(r#"{"trial":1}"#));
    records.activate(&qwen, Some(&default), 7).await.unwrap();
    let stored = records.load().await.unwrap();
    assert_eq!(stored.active, Some(qwen.clone()));
    assert_eq!(stored.previous, Some(default.clone()));
    let (by, at): (i64, i64) = store
        .run(|db| {
            Ok(db.query_row(
                "SELECT changed_by,changed_at FROM assistant_model_choice",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!((by, at > 0), (7, true));
    // The choice survives a reopened database.
    drop(records);
    let reopened = SqlRecords(store);
    assert_eq!(reopened.load().await.unwrap().active, Some(qwen));
}

#[tokio::test]
async fn switch_back_swaps_active_and_previous_and_refuses_without_a_previous() {
    let (_dir, store) = store();
    let records = SqlRecords(store);
    assert_eq!(
        records.swap(1).await,
        Err("there is no previous model".to_owned())
    );
    let default = record("granite.gguf", 'e', 1024, None);
    let qwen = record("qwen.gguf", 'a', 1024, Some(r#"{"trial":1}"#));
    records.activate(&qwen, Some(&default), 1).await.unwrap();
    records.swap(1).await.unwrap();
    let stored = records.load().await.unwrap();
    assert_eq!(stored.active, Some(default.clone()));
    assert_eq!(stored.previous, Some(qwen.clone()));
    records.swap(1).await.unwrap();
    assert_eq!(records.load().await.unwrap().active, Some(qwen));
}

#[tokio::test]
async fn a_model_switched_away_from_keeps_one_row_and_a_retry_of_the_active_file_keeps_the_previous()
 {
    let (_dir, store) = store();
    let records = SqlRecords(store.clone());
    let default = record("granite.gguf", 'e', 1024, None);
    let qwen = record("qwen.gguf", 'a', 1024, Some(r#"{"trial":1}"#));
    let qwen_again = record("qwen.gguf", 'a', 1024, Some(r#"{"trial":2}"#));
    records.activate(&qwen, Some(&default), 1).await.unwrap();
    // Back to the default through a new trial of it: its old row is found, not duplicated.
    let default_tried = record("granite.gguf", 'e', 1024, Some(r#"{"trial":3}"#));
    records
        .activate(&default_tried, Some(&qwen), 1)
        .await
        .unwrap();
    assert_eq!(
        count(&store, "assistant_models").await,
        3,
        "default, qwen, default tried"
    );
    // Re-trying the active file (no old given) keeps the previous model.
    records.activate(&qwen_again, None, 1).await.unwrap();
    let stored = records.load().await.unwrap();
    assert_eq!(stored.active, Some(qwen_again));
    assert_eq!(stored.previous, Some(qwen));
    assert_eq!(count(&store, "assistant_model_choice").await, 1);
}

#[tokio::test]
async fn the_tables_refuse_rows_the_runtime_would_never_write() {
    let (_dir, store) = store();
    let good = "('local:a.gguf','a.gguf','{}',NULL,'a',NULL,1)".replace("{}", &"a".repeat(64));
    let bad = |sha: &str, name: &str, spec: &str, alias: &str, trial: &str| {
        format!(
            "INSERT INTO assistant_models(spec,file_name,sha256,size,alias,trial,recorded_at) VALUES('{spec}','{name}','{sha}',NULL,'{alias}',{trial},1)"
        )
    };
    let sha = "a".repeat(64);
    let refused = vec![
        bad(&"A".repeat(64), "a.gguf", "local:a.gguf", "a", "NULL"),
        bad(&"a".repeat(63), "a.gguf", "local:a.gguf", "a", "NULL"),
        bad(&sha, "../a.gguf", "local:a.gguf", "a", "NULL"),
        bad(&sha, ".a.gguf", "local:a.gguf", "a", "NULL"),
        bad(&sha, "a.bin", "local:a.gguf", "a", "NULL"),
        bad(&sha, "a.gguf", "hf:a/b/c.gguf@x", "a", "NULL"),
        bad(&sha, "a.gguf", "local:a.gguf", "a b", "NULL"),
        bad(&sha, "a.gguf", "local:a.gguf", "", "NULL"),
        bad(&sha, "a.gguf", "local:a.gguf", "a", "'not json'"),
        bad(&sha, "a.gguf", "local:a.gguf", "a", "'[1]'"),
    ];
    store
        .run(move |db| {
            db.execute(
                &format!("INSERT INTO assistant_models(spec,file_name,sha256,size,alias,trial,recorded_at) VALUES{good}"),
                [],
            )?;
            for statement in &refused {
                assert!(db.execute(statement, []).is_err(), "{statement}");
            }
            // A model row has no reply length: that is the file's.
            assert!(
                db.prepare("SELECT max_tokens FROM assistant_models").is_err()
            );
            // The single choice row, and no model as its own previous.
            assert!(db
                .execute(
                    "INSERT INTO assistant_model_choice VALUES(2,1,NULL,1,1)",
                    []
                )
                .is_err());
            assert!(db
                .execute(
                    "INSERT INTO assistant_model_choice VALUES(1,1,1,1,1)",
                    []
                )
                .is_err());
            assert!(db
                .execute(
                    "INSERT INTO assistant_model_choice VALUES(1,99,NULL,1,1)",
                    []
                )
                .is_err());
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn the_reply_length_table_accepts_every_step_and_follows_the_constants() {
    use crate::assistant::model::reply_length::{MAX, MIN, STEP};
    let (_dir, store) = store();
    store
        .run(move |db| {
            for (n, tokens) in (MIN..=MAX).step_by(STEP as usize).enumerate() {
                db.execute(
                    "INSERT INTO assistant_reply_lengths(sha256,max_tokens,changed_by,changed_at) VALUES(?1,?2,1,1)",
                    params![format!("{n:064x}"), tokens],
                )?;
            }
            let insert = |sha: String, tokens: i64, by: i64, at: i64| {
                db.execute(
                    "INSERT INTO assistant_reply_lengths(sha256,max_tokens,changed_by,changed_at) VALUES(?1,?2,?3,?4)",
                    params![sha, tokens, by, at],
                )
            };
            let ok = "b".repeat(64);
            // One step either side of the bounds, and every off-step value in between.
            for tokens in [
                i64::from(MIN - STEP),
                i64::from(MAX + STEP),
                i64::from(MIN + 1),
                i64::from(MAX - 1),
                1000,
                1500,
                2559,
                0,
            ] {
                assert!(insert(ok.clone(), tokens, 1, 1).is_err(), "{tokens}");
            }
            // The hash is 64 lowercase hex digits; the account and the time are positive.
            assert!(insert("B".repeat(64), 1024, 1, 1).is_err());
            assert!(insert("b".repeat(63), 1024, 1, 1).is_err());
            assert!(insert(format!("{}g", "b".repeat(63)), 1024, 1, 1).is_err());
            assert!(insert(ok.clone(), 1024, 0, 1).is_err());
            assert!(insert(ok.clone(), 1024, 1, 0).is_err());
            assert!(insert(ok.clone(), 1024, 1, 1).is_ok());
            // One row per file.
            assert!(insert(ok, 1280, 1, 1).is_err());
            Ok(())
        })
        .await
        .unwrap();
    // The migration's CHECK text names the same bounds and step as the constants.
    let migration = include_str!("../../migrations/032_assistant_reply_length.sql");
    assert!(
        migration.contains(&format!(
            "max_tokens BETWEEN {MIN} AND {MAX} AND max_tokens % {STEP} = 0"
        )),
        "{migration}"
    );
}

/// The row counts of the models, the choice and the reply lengths.
async fn tables(store: &Store) -> (i64, i64, i64) {
    (
        count(store, "assistant_models").await,
        count(store, "assistant_model_choice").await,
        count(store, "assistant_reply_lengths").await,
    )
}

#[tokio::test]
async fn the_reply_length_is_stored_per_file_apart_from_the_models_and_the_choice() {
    let (_dir, store) = store();
    let records = SqlRecords(store.clone());
    let qwen = record("qwen.gguf", 'a', 1024, Some(r#"{"trial":1}"#));
    let default = record("granite.gguf", 'e', 1024, None);
    records.activate(&qwen, Some(&default), 1).await.unwrap();
    let before = records.load().await.unwrap();
    assert_eq!(tables(&store).await, (2, 1, 0));
    assert_eq!(records.length(&qwen.model.sha256).await, Ok(None));
    // Setting it touches one row of its own table: the model rows and the choice stay.
    records
        .set_length(&qwen.model.sha256, Some(1536), 4)
        .await
        .unwrap();
    assert_eq!(tables(&store).await, (2, 1, 1));
    assert_eq!(records.length(&qwen.model.sha256).await, Ok(Some(1536)));
    let after = records.load().await.unwrap();
    assert_eq!(after.active.as_ref().unwrap().model.max_tokens, 1536);
    assert_eq!(after.active.as_ref().unwrap().trial, qwen.trial);
    // The other file still has the default; the choice's own columns are unchanged.
    assert_eq!(after.previous, before.previous);
    let (by, active): (i64, i64) = store
        .run(|db| {
            Ok(db.query_row(
                "SELECT changed_by,active_model FROM assistant_model_choice",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!((by, active), (1, 2));
    // A second setting replaces the file's row (one row per file), with the new account.
    records
        .set_length(&qwen.model.sha256, Some(2560), 9)
        .await
        .unwrap();
    assert_eq!(tables(&store).await, (2, 1, 1));
    let changed_by: i64 = store
        .run(|db| {
            Ok(
                db.query_row("SELECT changed_by FROM assistant_reply_lengths", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .await
        .unwrap();
    assert_eq!(changed_by, 9);
    // Another file keeps its own length; a swap keeps both; a re-trial keeps the file's.
    records
        .set_length(&default.model.sha256, Some(256), 9)
        .await
        .unwrap();
    records.swap(1).await.unwrap();
    let swapped = records.load().await.unwrap();
    assert_eq!(swapped.active.unwrap().model.max_tokens, 256);
    assert_eq!(swapped.previous.unwrap().model.max_tokens, 2560);
    let retried = record("qwen.gguf", 'a', 1024, Some(r#"{"trial":2}"#));
    records.activate(&retried, None, 1).await.unwrap();
    assert_eq!(
        records
            .load()
            .await
            .unwrap()
            .active
            .unwrap()
            .model
            .max_tokens,
        2560
    );
    // Deleting restores the default; deleting nothing is not an error.
    records
        .set_length(&qwen.model.sha256, None, 1)
        .await
        .unwrap();
    records
        .set_length(&qwen.model.sha256, None, 1)
        .await
        .unwrap();
    assert_eq!(records.length(&qwen.model.sha256).await, Ok(None));
    assert_eq!(
        records
            .load()
            .await
            .unwrap()
            .active
            .unwrap()
            .model
            .max_tokens,
        1024
    );
    assert_eq!(records.length(&default.model.sha256).await, Ok(Some(256)));
}

/// The migrated rows as read back: version, models, choice, foreign-key violations, leftovers.
type Upgraded = (i64, Vec<(i64, i64, String)>, (i64, i64, i64, i64), i64, i64);

/// A database at schema 31 with an active and a previous model (the old 352 reply length and
/// a trial each), as the version before migration 032 wrote it.
async fn schema_31_database(path: &std::path::Path) {
    let store = Store::open(path).unwrap();
    store
        .run(|db| {
            db.execute_batch(
                "DROP TABLE assistant_reply_lengths; DROP TABLE assistant_model_choice; DROP TABLE assistant_models;",
            )?;
            db.execute_batch(
                &include_str!("../../migrations/022_assistant_models.sql")
                    .replace("PRAGMA user_version=22;", ""),
            )?;
            for (id, name, sha) in [(4, "granite.gguf", 'e'), (9, "qwen.gguf", 'a')] {
                db.execute(
                    "INSERT INTO assistant_models(id,spec,file_name,sha256,size,alias,max_tokens,trial,recorded_at) VALUES(?1,?2,?3,?4,1000,'x',352,?5,11)",
                    params![
                        id,
                        format!("local:{name}"),
                        name,
                        sha.to_string().repeat(64),
                        format!(r#"{{"trial":{id}}}"#)
                    ],
                )?;
            }
            db.execute(
                "INSERT INTO assistant_model_choice(id,active_model,previous_model,changed_by,changed_at) VALUES(1,9,4,7,5)",
                [],
            )?;
            db.execute_batch("PRAGMA user_version=31;")?;
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn schema_31_with_stored_models_upgrades_keeping_rows_and_choice_and_adding_the_lengths() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mimik.sqlite3");
    schema_31_database(&path).await;
    let store = Store::open(&path).unwrap();
    let (version, models, choice, check, leftovers): Upgraded = store
        .run(|db| {
            let version = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
            let models = db
                .prepare("SELECT id,size,trial FROM assistant_models ORDER BY id")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<Result<_, _>>()?;
            let choice = db.query_row(
                "SELECT active_model,previous_model,changed_by,changed_at FROM assistant_model_choice",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?;
            let check = db
                .prepare("PRAGMA foreign_key_check")?
                .query_map([], |_| Ok(()))?
                .count();
            let leftovers = db.query_row(
                "SELECT (SELECT count(*) FROM sqlite_master WHERE name LIKE 'assistant_%_old') + (SELECT count(*) FROM sqlite_temp_master)",
                [],
                |r| r.get(0),
            )?;
            Ok((version, models, choice, i64::try_from(check).unwrap(), leftovers))
        })
        .await
        .unwrap();
    assert_eq!(version, crate::store::SCHEMA_VERSION);
    // Ids, sizes and trials kept; the choice (and who made it, when) still points at them.
    assert_eq!(
        models,
        vec![
            (4, 1000, r#"{"trial":4}"#.to_owned()),
            (9, 1000, r#"{"trial":9}"#.to_owned())
        ]
    );
    assert_eq!(choice, (9, 4, 7, 5));
    assert_eq!((check, leftovers), (0, 0));
    // Every migrated file has the default length and the file table starts empty.
    let records = SqlRecords(store.clone());
    let stored = records.load().await.unwrap();
    assert_eq!(stored.active.as_ref().unwrap().model.max_tokens, 1024);
    assert_eq!(stored.previous.as_ref().unwrap().model.name, "granite.gguf");
    assert_eq!(tables(&store).await, (2, 1, 0));
    records
        .set_length(&"a".repeat(64), Some(1536), 3)
        .await
        .unwrap();
    let stored = records.load().await.unwrap();
    assert_eq!(stored.active.as_ref().unwrap().model.max_tokens, 1536);
    assert_eq!(stored.previous.as_ref().unwrap().model.max_tokens, 1024);
    assert_eq!(tables(&store).await, (2, 1, 1));
    store
        .run(|db| {
            assert!(
                db.prepare("SELECT max_tokens FROM assistant_models")
                    .is_err()
            );
            assert!(
                db.execute("UPDATE assistant_model_choice SET active_model=99", [])
                    .is_err()
            );
            // A model with a choice pointing at it cannot be deleted.
            assert!(
                db.execute("DELETE FROM assistant_models WHERE id=4", [])
                    .is_err()
            );
            Ok(())
        })
        .await
        .unwrap();
}

/// A trial as the manager stores it under the prompt in force, and one from before ASSIST-56.
fn current_trial() -> String {
    format!(
        r#"{{"prefix_sha256":"{}","trial":9}}"#,
        crate::assistant::contract::prefix_sha256()
    )
}

const OLD_TRIAL: &str = r#"{"trial":1}"#;

#[tokio::test]
async fn the_older_prompt_flag_covers_the_active_and_the_previous_model_separately() {
    let (_dir, store) = store();
    let records = SqlRecords(store);
    let old = record("old.gguf", 'a', 1024, Some(OLD_TRIAL));
    let current = record("new.gguf", 'b', 1024, Some(&current_trial()));
    // The previous model is the one measured with the older prompt...
    records.activate(&current, Some(&old), 1).await.unwrap();
    assert_eq!(
        records.load().await.unwrap().older_prompt(),
        OlderPrompt {
            active: false,
            previous: true
        }
    );
    // ...and after the models change places, the active one is.
    records.swap(1).await.unwrap();
    assert_eq!(
        records.load().await.unwrap().older_prompt(),
        OlderPrompt {
            active: true,
            previous: false
        }
    );
    // A stored choice with neither is flagged for neither.
    assert_eq!(Stored::default().older_prompt(), OlderPrompt::default());
}

#[tokio::test]
async fn the_older_prompt_flag_survives_switches_a_swap_and_a_retrial_in_the_database() {
    let (_dir, store) = store();
    let records = SqlRecords(store);
    let flags = |stored: Stored| (stored.older_prompt(), stored);
    // Load an old trial: the active model was measured before ASSIST-56.
    let a_old = record("a.gguf", 'a', 1024, Some(OLD_TRIAL));
    records.activate(&a_old, None, 1).await.unwrap();
    let (older, _) = flags(records.load().await.unwrap());
    assert_eq!((older.active, older.previous), (true, false));
    // Switch to B (tried under the current prompt). The manager stores the model it leaves
    // with no trial; its row is found, so A keeps the trial it had.
    let b = record("b.gguf", 'b', 1024, Some(&current_trial()));
    let a_left = record("a.gguf", 'a', 1024, None);
    records.activate(&b, Some(&a_left), 1).await.unwrap();
    let (older, stored) = flags(records.load().await.unwrap());
    assert_eq!((older.active, older.previous), (false, true));
    assert_eq!(stored.previous.unwrap().model.name, "a.gguf");
    // Switch back to A: it is the flagged one again, after a reload too.
    records.swap(1).await.unwrap();
    let (older, _) = flags(records.load().await.unwrap());
    assert_eq!((older.active, older.previous), (true, false));
    // Trying A again under the current prompt and switching to it (the active file, so no
    // previous model is given): A is no longer flagged and B stays the previous model.
    let a_again = record("a.gguf", 'a', 1024, Some(&current_trial()));
    records.activate(&a_again, None, 1).await.unwrap();
    let (older, stored) = flags(records.load().await.unwrap());
    assert_eq!((older.active, older.previous), (false, false));
    assert_eq!(stored.active.unwrap().model.name, "a.gguf");
    assert_eq!(stored.previous.unwrap().model.name, "b.gguf");
}

#[tokio::test]
async fn the_reminder_setting_is_unset_until_chosen_and_one_row_holds_the_choice() {
    let (_dir, store) = store();
    let records = SqlRecords(store.clone());
    assert_eq!(records.reminders().await, Ok(None));
    records.set_reminders(true, 3).await.unwrap();
    assert_eq!(records.reminders().await, Ok(Some(true)));
    records.set_reminders(false, 4).await.unwrap();
    assert_eq!(records.reminders().await, Ok(Some(false)));
    let (rows, by) = store
        .run(|db| {
            Ok(db.query_row(
                "SELECT count(*), max(changed_by) FROM assistant_reminders",
                [],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!((rows, by), (1, 4));
    // The table admits no other row and no other value.
    store
        .run(|db| {
            assert!(
                db.execute(
                    "INSERT INTO assistant_reminders(id,enabled,changed_by,changed_at) VALUES(2,1,1,1)",
                    []
                )
                .is_err()
            );
            assert!(
                db.execute("UPDATE assistant_reminders SET enabled=2", [])
                    .is_err()
            );
            Ok(())
        })
        .await
        .unwrap();
}
