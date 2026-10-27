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
    let default = record("granite.gguf", 'e', 352, None);
    let qwen = record("qwen.gguf", 'a', 320, Some(r#"{"trial":1}"#));
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
    let default = record("granite.gguf", 'e', 352, None);
    let qwen = record("qwen.gguf", 'a', 320, Some(r#"{"trial":1}"#));
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
    let default = record("granite.gguf", 'e', 352, None);
    let qwen = record("qwen.gguf", 'a', 320, Some(r#"{"trial":1}"#));
    let qwen_again = record("qwen.gguf", 'a', 352, Some(r#"{"trial":2}"#));
    records.activate(&qwen, Some(&default), 1).await.unwrap();
    // Back to the default through a new trial of it: its old row is found, not duplicated.
    let default_tried = record("granite.gguf", 'e', 352, Some(r#"{"trial":3}"#));
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
    let good = "('local:a.gguf','a.gguf','{}',NULL,'a',352,NULL,1)".replace("{}", &"a".repeat(64));
    let bad = |sha: &str, name: &str, spec: &str, alias: &str, tokens: i64, trial: &str| {
        format!(
            "INSERT INTO assistant_models(spec,file_name,sha256,size,alias,max_tokens,trial,recorded_at) VALUES('{spec}','{name}','{sha}',NULL,'{alias}',{tokens},{trial},1)"
        )
    };
    let sha = "a".repeat(64);
    let refused = vec![
        bad(&"A".repeat(64), "a.gguf", "local:a.gguf", "a", 352, "NULL"),
        bad(&"a".repeat(63), "a.gguf", "local:a.gguf", "a", 352, "NULL"),
        bad(&sha, "../a.gguf", "local:a.gguf", "a", 352, "NULL"),
        bad(&sha, ".a.gguf", "local:a.gguf", "a", 352, "NULL"),
        bad(&sha, "a.bin", "local:a.gguf", "a", 352, "NULL"),
        bad(&sha, "a.gguf", "hf:a/b/c.gguf@x", "a", 352, "NULL"),
        bad(&sha, "a.gguf", "local:a.gguf", "a b", 352, "NULL"),
        bad(&sha, "a.gguf", "local:a.gguf", "", 352, "NULL"),
        bad(&sha, "a.gguf", "local:a.gguf", "a", 255, "NULL"),
        bad(&sha, "a.gguf", "local:a.gguf", "a", 385, "NULL"),
        bad(&sha, "a.gguf", "local:a.gguf", "a", 352, "'not json'"),
        bad(&sha, "a.gguf", "local:a.gguf", "a", 352, "'[1]'"),
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
    let old = record("old.gguf", 'a', 320, Some(OLD_TRIAL));
    let current = record("new.gguf", 'b', 320, Some(&current_trial()));
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
    let a_old = record("a.gguf", 'a', 320, Some(OLD_TRIAL));
    records.activate(&a_old, None, 1).await.unwrap();
    let (older, _) = flags(records.load().await.unwrap());
    assert_eq!((older.active, older.previous), (true, false));
    // Switch to B (tried under the current prompt). The manager stores the model it leaves
    // with no trial; its row is found, so A keeps the trial it had.
    let b = record("b.gguf", 'b', 320, Some(&current_trial()));
    let a_left = record("a.gguf", 'a', 320, None);
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
    let a_again = record("a.gguf", 'a', 320, Some(&current_trial()));
    records.activate(&a_again, None, 1).await.unwrap();
    let (older, stored) = flags(records.load().await.unwrap());
    assert_eq!((older.active, older.previous), (false, false));
    assert_eq!(stored.active.unwrap().model.name, "a.gguf");
    assert_eq!(stored.previous.unwrap().model.name, "b.gguf");
}
