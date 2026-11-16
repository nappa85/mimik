use super::*;
use crate::assistant::runtime::models::OlderPrompt;
use std::sync::Mutex;

/// Records in memory for the manager's tests: what was stored, and optionally a failure.
#[derive(Default)]
pub(crate) struct MemRecords {
    pub log: Mutex<Vec<String>>,
    pub fail: Mutex<Option<String>>,
    pub stored: Mutex<Stored>,
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

    async fn set_length(&self, active: &Model, max_tokens: u32, by: i64) -> Result<(), String> {
        if let Some(why) = self.fail.lock().unwrap().clone() {
            return Err(why);
        }
        self.log
            .lock()
            .unwrap()
            .push(format!("set_length {max_tokens} by {by}"));
        let mut stored = self.stored.lock().unwrap();
        let trial = stored.active.as_ref().and_then(|r| r.trial.clone());
        let old = Record {
            model: active.clone(),
            trial: trial.clone(),
        };
        stored.previous = Some(old);
        stored.active = Some(Record {
            model: Model {
                max_tokens,
                ..active.clone()
            },
            trial,
        });
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
    let qwen = record("qwen.gguf", 'a', 384, Some(r#"{"trial":1}"#));
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
    let qwen = record("qwen.gguf", 'a', 384, Some(r#"{"trial":1}"#));
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
    let qwen = record("qwen.gguf", 'a', 384, Some(r#"{"trial":1}"#));
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
    let good = "('local:a.gguf','a.gguf','{}',NULL,'a',1024,NULL,1)".replace("{}", &"a".repeat(64));
    let bad = |sha: &str, name: &str, spec: &str, alias: &str, tokens: i64, trial: &str| {
        format!(
            "INSERT INTO assistant_models(spec,file_name,sha256,size,alias,max_tokens,trial,recorded_at) VALUES('{spec}','{name}','{sha}',NULL,'{alias}',{tokens},{trial},1)"
        )
    };
    let sha = "a".repeat(64);
    let refused = vec![
        bad(&"A".repeat(64), "a.gguf", "local:a.gguf", "a", 1024, "NULL"),
        bad(&"a".repeat(63), "a.gguf", "local:a.gguf", "a", 1024, "NULL"),
        bad(&sha, "../a.gguf", "local:a.gguf", "a", 1024, "NULL"),
        bad(&sha, ".a.gguf", "local:a.gguf", "a", 1024, "NULL"),
        bad(&sha, "a.bin", "local:a.gguf", "a", 1024, "NULL"),
        bad(&sha, "a.gguf", "hf:a/b/c.gguf@x", "a", 1024, "NULL"),
        bad(&sha, "a.gguf", "local:a.gguf", "a b", 1024, "NULL"),
        bad(&sha, "a.gguf", "local:a.gguf", "", 1024, "NULL"),
        bad(&sha, "a.gguf", "local:a.gguf", "a", 255, "NULL"),
        bad(&sha, "a.gguf", "local:a.gguf", "a", 385, "NULL"),
        // Within the bounds but off the 128-token step, and beyond the new top.
        bad(&sha, "a.gguf", "local:a.gguf", "a", 352, "NULL"),
        bad(&sha, "a.gguf", "local:a.gguf", "a", 1025, "NULL"),
        bad(&sha, "a.gguf", "local:a.gguf", "a", 2688, "NULL"),
        bad(&sha, "a.gguf", "local:a.gguf", "a", 0, "NULL"),
        bad(&sha, "a.gguf", "local:a.gguf", "a", 1024, "'not json'"),
        bad(&sha, "a.gguf", "local:a.gguf", "a", 1024, "'[1]'"),
    ];
    store
        .run(move |db| {
            db.execute(
                &format!("INSERT INTO assistant_models(spec,file_name,sha256,size,alias,max_tokens,trial,recorded_at) VALUES{good}"),
                [],
            )?;
            for statement in &refused {
                assert!(db.execute(statement, []).is_err(), "{statement}");
            }
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
async fn the_check_accepts_every_step_of_the_reply_length_and_follows_the_constants() {
    use crate::assistant::model::reply_length::{MAX, MIN, STEP};
    let (_dir, store) = store();
    let sha = "a".repeat(64);
    store
        .run(move |db| {
            for tokens in (MIN..=MAX).step_by(STEP as usize) {
                db.execute(
                    "INSERT INTO assistant_models(spec,file_name,sha256,size,alias,max_tokens,trial,recorded_at) VALUES('local:a.gguf','a.gguf',?1,NULL,'a',?2,NULL,1)",
                    params![sha, tokens],
                )?;
            }
            // One step either side of the bounds, and every off-step value in between.
            for tokens in [MIN - STEP, MAX + STEP, MIN + 1, MAX - 1, 1000, 1500, 2559] {
                assert!(
                    db.execute(
                        "INSERT INTO assistant_models(spec,file_name,sha256,size,alias,max_tokens,trial,recorded_at) VALUES('local:a.gguf','a.gguf',?1,NULL,'a',?2,NULL,1)",
                        params![sha, tokens],
                    )
                    .is_err(),
                    "{tokens}"
                );
            }
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

#[tokio::test]
async fn setting_the_reply_length_copies_the_trial_and_makes_the_replaced_row_previous() {
    let (_dir, store) = store();
    let records = SqlRecords(store.clone());
    let qwen = record("qwen.gguf", 'a', 1024, Some(r#"{"trial":1}"#));
    let default = record("granite.gguf", 'e', 1024, None);
    records.activate(&qwen, Some(&default), 1).await.unwrap();
    records.set_length(&qwen.model, 1536, 4).await.unwrap();
    let stored = records.load().await.unwrap();
    let longer = Record {
        model: Model {
            max_tokens: 1536,
            ..qwen.model.clone()
        },
        trial: qwen.trial.clone(),
    };
    assert_eq!(stored.active, Some(longer.clone()));
    assert_eq!(stored.previous, Some(qwen.clone()));
    assert_eq!(count(&store, "assistant_models").await, 3);
    let (by, rows): (i64, i64) = store
        .run(|db| {
            Ok(db.query_row(
                "SELECT changed_by,(SELECT count(*) FROM assistant_model_choice) FROM assistant_model_choice",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!((by, rows), (4, 1));
    // Setting it back finds the row of the first length (the record match): no new row.
    records.set_length(&longer.model, 1024, 4).await.unwrap();
    assert_eq!(count(&store, "assistant_models").await, 3);
    assert_eq!(records.load().await.unwrap().active, Some(qwen.clone()));
    // The same length again is refused and changes nothing.
    assert!(records.set_length(&qwen.model, 1024, 4).await.is_err());
    assert_eq!(count(&store, "assistant_models").await, 3);
    // A model the start flags named (no stored row) is stored first as the previous row.
    let (_flags_dir, flags) = self::store();
    let records = SqlRecords(flags.clone());
    let named = model("flag.gguf", 'c', 1024);
    records.set_length(&named, 2560, 1).await.unwrap();
    let stored = records.load().await.unwrap();
    assert_eq!(stored.active.unwrap().model.max_tokens, 2560);
    assert_eq!(stored.previous.unwrap().model, named);
    assert_eq!(count(&flags, "assistant_models").await, 2);
}

/// The migrated rows as read back: version, models, choice, foreign-key violations, leftovers.
type Upgraded = (i64, Vec<(i64, i64, String)>, (i64, i64, i64, i64), i64, i64);

#[tokio::test]
async fn schema_31_with_stored_models_upgrades_keeping_ids_and_setting_1024() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mimik.sqlite3");
    {
        let store = Store::open(&path).unwrap();
        store
            .run(|db| {
                // Rewind to schema 31: migration 022's tables, an active and a previous model
                // at the old 352 and a trial each.
                db.execute_batch("DROP TABLE assistant_model_choice; DROP TABLE assistant_models;")?;
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
    let store = Store::open(&path).unwrap();
    let (version, models, choice, check, leftovers): Upgraded = store
        .run(|db| {
            let version = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
            let models = db
                .prepare("SELECT id,max_tokens,trial FROM assistant_models ORDER BY id")?
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
    // Ids kept, max_tokens 1024, trials kept; the choice still points at them.
    assert_eq!(
        models,
        vec![
            (4, 1024, r#"{"trial":4}"#.to_owned()),
            (9, 1024, r#"{"trial":9}"#.to_owned())
        ]
    );
    assert_eq!(choice, (9, 4, 7, 5));
    assert_eq!((check, leftovers), (0, 0));
    // The runtime reads the migrated rows, and the new CHECK and the foreign key hold.
    let stored = SqlRecords(store.clone()).load().await.unwrap();
    assert_eq!(stored.active.unwrap().model.max_tokens, 1024);
    assert_eq!(stored.previous.unwrap().model.name, "granite.gguf");
    store
        .run(|db| {
            assert!(
                db.execute("UPDATE assistant_models SET max_tokens=352 WHERE id=9", [])
                    .is_err()
            );
            assert!(
                db.execute("UPDATE assistant_models SET max_tokens=1536 WHERE id=9", [])
                    .is_ok()
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
    let old = record("old.gguf", 'a', 384, Some(OLD_TRIAL));
    let current = record("new.gguf", 'b', 384, Some(&current_trial()));
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
    let a_old = record("a.gguf", 'a', 384, Some(OLD_TRIAL));
    records.activate(&a_old, None, 1).await.unwrap();
    let (older, _) = flags(records.load().await.unwrap());
    assert_eq!((older.active, older.previous), (true, false));
    // Switch to B (tried under the current prompt). The manager stores the model it leaves
    // with no trial; its row is found, so A keeps the trial it had.
    let b = record("b.gguf", 'b', 384, Some(&current_trial()));
    let a_left = record("a.gguf", 'a', 384, None);
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
    let a_again = record("a.gguf", 'a', 384, Some(&current_trial()));
    records.activate(&a_again, None, 1).await.unwrap();
    let (older, stored) = flags(records.load().await.unwrap());
    assert_eq!((older.active, older.previous), (false, false));
    assert_eq!(stored.active.unwrap().model.name, "a.gguf");
    assert_eq!(stored.previous.unwrap().model.name, "b.gguf");
}
