//! Single-transaction publication: a failure before any stage leaves nothing behind, and the
//! Rust projections agree with the seal trigger. Synthetic data only.
use super::{PublishRequest, Stage, publish_in};
use crate::{
    Store,
    catalogues::{CatalogueError, store::load_draft},
};
use rusqlite::{Connection, TransactionBehavior, params};

const GAME: &str = "synthetic-game";

/// A registered pair with one finalized synthetic source of one page and two units.
async fn database() -> (tempfile::TempDir, std::path::PathBuf, Store) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("publish.sqlite3");
    let store = Store::open(&path).unwrap();
    store
        .register_ruleset(crate::registry::RegisterRuleset {
            game_id: GAME.into(),
            game_label: "Synthetic game".into(),
            edition_id: "first".into(),
            edition_label: "First edition".into(),
        })
        .await
        .unwrap();
    store
        .run(|db| {
            let zeros = "0".repeat(64);
            let prepared = serde_json::json!({"contract_version": 1, "extraction_sha256": zeros}).to_string();
            db.execute("INSERT INTO sources(id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,raw_json,prepared_json,total_pages) VALUES(7,?1,'fixture','Fixture','synthetic-game','First','en',?1,x'',?2,1)", params![zeros, prepared.as_bytes()])?;
            db.execute("INSERT INTO source_rulesets VALUES(7,?1,'first')", [GAME])?;
            db.execute("INSERT INTO source_pages(id,source_id,pdf_page,png,printed_label) VALUES(70,7,1,x'','1')", [])?;
            for unit in [700, 701] {
                db.execute("INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(?1,70,?2,'text','[[0,0],[1,0],[1,1],[0,1]]','','Synthetic unit text')", params![unit, format!("b{unit}")])?;
            }
            db.execute("INSERT INTO source_page_hashes VALUES(70,7,?1)", [&zeros])?;
            db.execute("INSERT INTO source_finalizations VALUES(7,?1,?1)", [&zeros])?;
            Ok(())
        })
        .await
        .unwrap();
    (dir, path, store)
}

fn catalogue() -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "contract_version": 1, "game_id": GAME, "edition_id": "first", "catalogue_id": "spells",
        "title": "Synthetic spells", "sources": [{"source_id": 7, "pdf_sha256": "0".repeat(64)}],
        "entry_types": [{"id": "spell", "label": "Spell", "properties": [
            {"id": "level", "label": "Level", "control": "integer"}]}],
        "entries": [
            {"key": "flare", "type": "spell", "name": "Flare", "properties": {"level": 1},
             "description": {"units": [{"source_id": 7, "page_id": 70, "unit_id": 701},
                                       {"source_id": 7, "page_id": 70, "unit_id": 700}]}},
            {"key": "charm", "type": "spell", "name": "Charm", "properties": {"level": 0},
             "retired": true, "description": {"owner_text": "Synthetic house text."}}]
    }))
    .unwrap()
}

fn rows(db: &Connection) -> [i64; 7] {
    [
        "catalogues",
        "catalogue_versions",
        "catalogue_entry_types",
        "catalogue_entries",
        "catalogue_entry_units",
        "catalogue_seals",
        "catalogue_names_fts",
    ]
    .map(|table| {
        db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failure_before_any_stage_rolls_back_the_whole_publication() {
    let _guard = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, path, store) = database().await;
    let id = store.save_catalogue_draft(catalogue(), 0).await.unwrap().id;
    let ticket = store.compare_catalogue(id).await.unwrap().comparison_hash;
    let (draft, permit) = load_draft(&store, id).await.unwrap();
    drop(permit);
    let request = PublishRequest {
        comparison_hash: ticket,
        actor: "fixture".into(),
        rationale: "Synthetic".into(),
    };
    let mut db = Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    for stage in [Stage::Projections, Stage::NameIndex, Stage::Seal] {
        let reached = std::cell::Cell::new(Vec::new());
        {
            let tx = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            let result = publish_in(&tx, &draft, &request, &|at| {
                let mut seen = reached.take();
                seen.push(at);
                reached.set(seen);
                if at == stage {
                    Err(crate::catalogues::invalid("/test", "injected-failure"))
                } else {
                    Ok(())
                }
            });
            assert!(
                matches!(result, Err(CatalogueError::Invalid(_))),
                "{stage:?}"
            );
            // Some rows exist inside the transaction before it is abandoned.
            assert_eq!(rows(&tx)[1], 1, "{stage:?}");
        }
        assert_eq!(rows(&db), [0; 7], "{stage:?}");
        assert_eq!(reached.take().last(), Some(&stage));
    }
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let published = publish_in(&tx, &draft, &request, &|_| Ok(())).unwrap();
    tx.commit().unwrap();
    assert!(published.published);
    assert_eq!(rows(&db), [1, 1, 1, 2, 2, 1, 2]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rust_projections_equal_the_stored_rows_in_author_unit_order() {
    let _guard = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, path, store) = database().await;
    let id = store.save_catalogue_draft(catalogue(), 0).await.unwrap().id;
    let ticket = store.compare_catalogue(id).await.unwrap().comparison_hash;
    let published = store
        .publish_catalogue(
            id,
            PublishRequest {
                comparison_hash: ticket,
                actor: "fixture".into(),
                rationale: "Synthetic".into(),
            },
        )
        .await
        .unwrap();
    let db = Connection::open(&path).unwrap();
    let units: Vec<(String, i64, i64)> = db
        .prepare("SELECT key,ordinal,unit_id FROM catalogue_entry_units WHERE version_id=?1 ORDER BY ordinal")
        .unwrap()
        .query_map([published.version.version_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    // Unit order is the author's order, never sorted.
    assert_eq!(units, [("flare".into(), 0, 701), ("flare".into(), 1, 700)]);
    let (draft, _permit) = load_draft(&store, id).await.unwrap();
    assert_eq!(
        super::projections(&draft.data.catalogue).unwrap(),
        super::read_projections(&db, published.version.version_id).unwrap()
    );
}

/// `SQLite` VM steps spent inserting the projections of a fresh catalogue of `entries`
/// owner-written entries (the replacement and sealed guards run per row).
async fn projection_steps(entries: usize) -> u64 {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    let (_dir, path, store) = database().await;
    let listed: Vec<serde_json::Value> = (0..entries)
        .map(|index| {
            serde_json::json!({"key": format!("entry-{index:05}"), "type": "spell",
                "name": format!("Entry {index}"), "properties": {"level": 1},
                "description": {"owner_text": "Synthetic."}})
        })
        .collect();
    let mut value: serde_json::Value = serde_json::from_slice(&catalogue()).unwrap();
    value["entries"] = listed.into();
    let id = store
        .save_catalogue_draft(serde_json::to_vec(&value).unwrap(), 0)
        .await
        .unwrap()
        .id;
    let ticket = store.compare_catalogue(id).await.unwrap().comparison_hash;
    let (draft, permit) = load_draft(&store, id).await.unwrap();
    drop(permit);
    let mut db = Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    // The handler needs exclusive access to the connection, so it is set before the transaction
    // starts and counts only between the Projections and NameIndex stages.
    let steps = Arc::new(AtomicU64::new(0));
    let counting = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let (steps, counting) = (steps.clone(), counting.clone());
        db.progress_handler(
            100,
            Some(move || {
                if counting.load(Ordering::Relaxed) {
                    steps.fetch_add(100, Ordering::Relaxed);
                }
                false
            }),
        )
        .unwrap();
    }
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let request = PublishRequest {
        comparison_hash: ticket,
        actor: "fixture".into(),
        rationale: "Synthetic".into(),
    };
    publish_in(&tx, &draft, &request, &|stage| {
        match stage {
            Stage::Projections => counting.store(true, Ordering::Relaxed),
            Stage::NameIndex => counting.store(false, Ordering::Relaxed),
            Stage::Seal => {}
        }
        Ok(())
    })
    .unwrap();
    steps.load(Ordering::Relaxed)
}

/// Regression: replacement guards written as one `EXISTS(... key=… OR ordinal=…)` scanned the
/// whole version per inserted row, so publication was quadratic (10.5 s at 8,192 entries in
/// the release profile). Doubling the entries must about double the insert work.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn projection_inserts_scale_linearly() {
    let _guard = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let small = projection_steps(1024).await;
    let large = projection_steps(2048).await;
    assert!(small > 0);
    // Linear work doubles; the quadratic guards made it grow about 3.9 times.
    assert!(
        large * 10 < small * 26,
        "insert work grew more than 2.6x for 2x entries ({small} -> {large})"
    );
}

#[test]
fn integrity_check_storage_errors_propagate_and_failures_are_reported() {
    use rusqlite::{Error, ffi};
    let failure = |code| Err(Error::SqliteFailure(ffi::Error::new(code), None));
    for code in [
        ffi::SQLITE_BUSY,
        ffi::SQLITE_LOCKED,
        ffi::SQLITE_IOERR,
        ffi::SQLITE_NOMEM,
        ffi::SQLITE_INTERRUPT,
        ffi::SQLITE_FULL,
    ] {
        assert!(
            matches!(
                super::integrity_outcome(failure(code)),
                Err(CatalogueError::Storage(_))
            ),
            "{code}"
        );
    }
    for code in [
        ffi::SQLITE_CORRUPT_VTAB,
        ffi::SQLITE_CORRUPT,
        ffi::SQLITE_ERROR,
    ] {
        assert!(!super::integrity_outcome(failure(code)).unwrap(), "{code}");
    }
    assert!(super::integrity_outcome(Ok(1)).unwrap());
}

#[test]
fn drop_schema_matches_the_integration_fixture() {
    // `DROP_SCHEMA` (unit tests) and `DROP_CATALOGUE_SCHEMA` (integration tests, which cannot
    // see crate-private items) must name the same objects in the same order.
    let fixture = include_str!("../../tests/support/catalogue_fixture.rs");
    assert!(
        fixture.contains(&format!(
            "DROP_CATALOGUE_SCHEMA: &str = \"{}\";",
            crate::catalogues::DROP_SCHEMA
        )),
        "tests/support/catalogue_fixture.rs DROP_CATALOGUE_SCHEMA drifted"
    );
}
