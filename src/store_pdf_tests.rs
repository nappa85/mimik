//! Migration 037 (SRC-15 slice 7): the `sources.pdf` column goes; every other column and row
//! stays. No release shipped and local data is disposable (owner decision 2026-11-11), so the
//! legacy blobs are not copied into the store.
use super::{MIGRATIONS, SCHEMA_VERSION, Store, register_auth_clock};
use rusqlite::Connection;
use std::sync::Arc;

fn columns(db: &Connection) -> Vec<String> {
    db.prepare("SELECT name FROM pragma_table_info('sources') ORDER BY cid")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn migration_037_drops_the_pdf_column_and_keeps_rows_and_other_columns() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .unwrap();
    crate::auth::register_domain_guard(&connection).unwrap();
    register_auth_clock(&connection, Arc::new(|| Ok(100))).unwrap();
    for migration in &MIGRATIONS[..36] {
        connection.execute_batch(migration).unwrap();
    }
    let before = columns(&connection);
    assert!(before.contains(&"pdf".to_owned()));
    for (id, blob) in [(1, vec![1_u8; 5000]), (2, vec![2_u8; 10])] {
        connection
            .execute(
                "INSERT INTO sources(id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages)
                 VALUES(?1,?2,'s','Title','mork-borg','Edition','en',?3,?4,x'7b7d',x'7b7d',3)",
                rusqlite::params![id, format!("bundle-{id}"), "a".repeat(64), blob],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO source_pages(source_id,pdf_page,png) VALUES(?1,1,x'00')",
                [id],
            )
            .unwrap();
    }
    let store = Store::migrate(connection).unwrap();
    store
        .upload_storage(|db| {
            assert_eq!(
                db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
                SCHEMA_VERSION
            );
            assert_eq!(SCHEMA_VERSION, 38);
            let after = columns(db);
            let expected: Vec<String> = before.iter().filter(|c| *c != "pdf").cloned().collect();
            assert_eq!(after, expected);
            let rows: Vec<(i64, String, String, i64)> = db
                .prepare("SELECT id,bundle_hash,pdf_hash,total_pages FROM sources ORDER BY id")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<Result<_, _>>()?;
            assert_eq!(
                rows,
                vec![
                    (1, "bundle-1".into(), "a".repeat(64), 3),
                    (2, "bundle-2".into(), "a".repeat(64), 3)
                ]
            );
            let pages: i64 = db.query_row("SELECT count(*) FROM source_pages", [], |r| r.get(0))?;
            assert_eq!(pages, 2);
            let violations = db
                .prepare("PRAGMA foreign_key_check")?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            assert!(violations.is_empty(), "{violations:?}");
            Ok(())
        })
        .unwrap();
    // The old rows have no file in the store: both are counted as missing.
    let missing = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(store.missing_pdf_count())
        .unwrap();
    assert_eq!(missing, 2);
}

/// The rewind guard in `migrate` skips the whole migration, so its text is pinned: a later
/// statement added to 037 would not run on that path and must be a conscious change here.
#[test]
fn migration_037_is_exactly_the_drop_and_the_store_identity() {
    let statements: Vec<String> = MIGRATIONS[36]
        .lines()
        .filter(|line| !line.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n")
        .split(';')
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty())
        .collect();
    assert_eq!(
        statements,
        [
            "ALTER TABLE sources DROP COLUMN pdf",
            "CREATE TABLE pdf_store_identity ( singleton INTEGER PRIMARY KEY CHECK(singleton=1), store_id TEXT NOT NULL CHECK(length(store_id)=32 AND store_id NOT GLOB '*[^0-9a-f]*') )",
            "INSERT INTO pdf_store_identity VALUES(1,lower(hex(randomblob(16))))",
            "PRAGMA user_version=37",
        ]
    );
}

#[test]
fn a_database_rewound_below_037_reopens_with_its_store_id_intact() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    drop(Store::open(&path).unwrap());
    let id = |db: &Connection| -> String {
        db.query_row("SELECT store_id FROM pdf_store_identity", [], |r| r.get(0))
            .unwrap()
    };
    let db = Connection::open(&path).unwrap();
    let before = id(&db);
    db.pragma_update(None, "user_version", 36).unwrap();
    drop(db);
    drop(Store::open(&path).unwrap());
    let db = Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
    assert_eq!(id(&db), before);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("sources/.store-id")).unwrap(),
        before
    );
}
