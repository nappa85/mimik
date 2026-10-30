use super::*;

const SHA: &str = "03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8";
const SPEC: &str =
    "hf:unsloth/Qwen3.5-9B-GGUF/Qwen3.5-9B-Q4_K_M.gguf@3885219b6810b007914f3a7950a8d1b469d598a5";

fn new(spec: &str, file_name: &str) -> New {
    New {
        spec: spec.to_owned(),
        file_name: file_name.to_owned(),
        sha256: SHA.to_owned(),
        size: 5_680_522_464,
        licence_id: Some("apache-2.0".into()),
        licence_path: Some("LICENSE".into()),
        licence_sha256: Some("a".repeat(64)),
        confirmed_by: 1,
    }
}

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("mimik.sqlite3")).unwrap();
    (dir, store)
}

#[tokio::test]
async fn rows_are_recorded_listed_completed_and_deleted() {
    let (_dir, store) = store();
    let rows = SqlRows(store);
    assert_eq!(rows.list().await.unwrap().len(), 0);
    let first = rows
        .insert(new(SPEC, "Qwen3.5-9B-Q4_K_M.gguf"))
        .await
        .unwrap();
    assert_eq!(first.state, State::Confirmed);
    assert_eq!(first.size, 5_680_522_464);
    assert_eq!(first.completed_at, None);
    assert!(first.confirmed_at > 0);
    // The same spec cannot be confirmed twice.
    assert!(rows.insert(new(SPEC, "other.gguf")).await.is_err());
    rows.complete(first.id).await.unwrap();
    let listed = rows.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].state, State::Complete);
    assert!(listed[0].completed_at.is_some());
    rows.delete(first.id).await.unwrap();
    assert_eq!(rows.list().await.unwrap().len(), 0);
}

#[tokio::test]
async fn the_checks_refuse_rows_the_grammar_would_not() {
    let (_dir, store) = store();
    let rows = SqlRows(store);
    let bad: Vec<New> = vec![
        new("local:x.gguf", "x.gguf"),
        new(&SPEC.replace("@3885219b", "@3885219B"), "x.gguf"),
        new(&format!("{SPEC} "), "x.gguf"),
        new(SPEC, "../x.gguf"),
        new(SPEC, ".hidden.gguf"),
        new(SPEC, "x.bin"),
        New {
            sha256: SHA.to_uppercase(),
            ..new(SPEC, "x.gguf")
        },
        New {
            size: 0,
            ..new(SPEC, "x.gguf")
        },
        New {
            size: 8_589_934_593,
            ..new(SPEC, "x.gguf")
        },
        New {
            licence_id: Some("a b".into()),
            ..new(SPEC, "x.gguf")
        },
        New {
            licence_path: Some("../LICENSE".into()),
            ..new(SPEC, "x.gguf")
        },
        New {
            licence_sha256: Some("zz".into()),
            ..new(SPEC, "x.gguf")
        },
        New {
            confirmed_by: 0,
            ..new(SPEC, "x.gguf")
        },
    ];
    for row in bad {
        assert!(rows.insert(row.clone()).await.is_err(), "{row:?}");
    }
    // A state without its completion time is refused by the table itself.
    let first = rows.insert(new(SPEC, "x.gguf")).await.unwrap();
    let broken = rows
        .0
        .run(move |db| {
            Ok(db
                .execute(
                    "UPDATE assistant_downloads SET state='complete' WHERE id=?1",
                    [first.id],
                )
                .is_err())
        })
        .await
        .unwrap();
    assert!(broken);
}

#[tokio::test]
async fn the_model_choice_decides_whether_a_file_is_in_use() {
    let (_dir, store) = store();
    let rows = SqlRows(store.clone());
    assert!(!rows.in_use("a.gguf").await.unwrap());
    store
        .run(|db| {
            for (name, id) in [("a.gguf", 1), ("b.gguf", 2), ("c.gguf", 3)] {
                db.execute(
                    "INSERT INTO assistant_models(id,spec,file_name,sha256,size,alias,max_tokens,trial,recorded_at) VALUES(?1,?2,?3,?4,10,'alias',256,NULL,1)",
                    rusqlite::params![id, format!("local:{name}"), name, SHA],
                )?;
            }
            db.execute(
                "INSERT INTO assistant_model_choice(id,active_model,previous_model,changed_by,changed_at) VALUES(1,1,2,1,1)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(rows.in_use("a.gguf").await.unwrap());
    assert!(rows.in_use("b.gguf").await.unwrap());
    // Tried once but neither active nor previous: not in use.
    assert!(!rows.in_use("c.gguf").await.unwrap());
}

#[tokio::test]
async fn migration_023_keeps_populated_022_tables_and_passes_the_foreign_key_check() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mimik.sqlite3");
    {
        let store = Store::open(&path).unwrap();
        store
            .run(|db| {
                for (name, id) in [("a.gguf", 1), ("b.gguf", 2)] {
                    db.execute(
                        "INSERT INTO assistant_models(id,spec,file_name,sha256,size,alias,max_tokens,trial,recorded_at) VALUES(?1,?2,?3,?4,10,'alias',300,'{\"a\":1}',5)",
                        rusqlite::params![id, format!("local:{name}"), name, SHA],
                    )?;
                }
                db.execute(
                    "INSERT INTO assistant_model_choice(id,active_model,previous_model,changed_by,changed_at) VALUES(1,2,1,7,9)",
                    [],
                )?;
                // Rewind to a populated schema 22.
                crate::sources::documents::rewind_schema(db)?;
                db.execute_batch("DROP TABLE assistant_downloads; PRAGMA user_version=22;")?;
                Ok(())
            })
            .await
            .unwrap();
    }
    // The production runner applies migration 023 on opening.
    let store = Store::open(&path).unwrap();
    let (version, models, choice, violations, downloads) = store
        .run(|db| {
            let count = |sql: &str| db.query_row(sql, [], |r| r.get::<_, i64>(0));
            let violations = db
                .prepare("PRAGMA foreign_key_check")?
                .query_map([], |_| Ok(()))?
                .count();
            Ok((
                count("PRAGMA user_version")?,
                count("SELECT count(*) FROM assistant_models WHERE max_tokens=300 AND trial='{\"a\":1}'")?,
                count("SELECT count(*) FROM assistant_model_choice WHERE active_model=2 AND previous_model=1 AND changed_by=7 AND changed_at=9")?,
                violations,
                count("SELECT count(*) FROM assistant_downloads")?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(
        (version, models, choice, violations, downloads),
        (crate::store::SCHEMA_VERSION, 2, 1, 0, 0)
    );
}

#[tokio::test]
async fn every_licence_path_the_hub_admits_is_one_the_table_accepts() {
    use crate::assistant::download::hub::valid_repo_path;
    let (_dir, store) = store();
    let rows = SqlRows(store);
    let corpus = [
        "LICENSE",
        "LICENSE.md",
        "LICENSE..txt",
        "a/b/LICENSE",
        "a..b/LICENSE",
        "docs/LICENSE.v2..final",
        "x_y-z.1/NOTICE",
        "..LICENSE",
        "LICENSE..",
        "a/..b",
    ];
    for (index, path) in corpus.iter().enumerate() {
        let admitted = valid_repo_path(path);
        let spec = SPEC.replace("Qwen3.5-9B-Q4_K_M", &format!("m{index}"));
        let result = rows
            .insert(New {
                licence_path: Some((*path).to_owned()),
                ..new(&spec, "x.gguf")
            })
            .await;
        assert_eq!(
            admitted,
            result.is_ok(),
            "{path}: the hub grammar and the table must agree"
        );
    }
}

#[tokio::test]
async fn a_download_id_is_never_reused_after_its_row_is_deleted() {
    let (_dir, store) = store();
    let rows = SqlRows(store);
    let first = rows.insert(new(SPEC, "x.gguf")).await.unwrap();
    rows.delete(first.id).await.unwrap();
    // The deleted row held the largest id: a plain INTEGER PRIMARY KEY would hand it out again.
    let second = rows.insert(new(SPEC, "x.gguf")).await.unwrap();
    assert!(second.id > first.id, "{} then {}", first.id, second.id);
    rows.delete(second.id).await.unwrap();
    let third = rows.insert(new(SPEC, "x.gguf")).await.unwrap();
    assert!(third.id > second.id);
}
