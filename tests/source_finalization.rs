use mimik::Store;
use std::fs;

#[path = "support/source_fixture.rs"]
mod fixture;
#[path = "support/schema_fixture.rs"]
mod schema_fixture;

#[tokio::test]
async fn revision_correct_completion_rejects_unreviewed_units() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("completion.sqlite3");
    let store = Store::open(&path).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    assert!(
        db.execute(
            "UPDATE source_pages SET complete=1,revision=revision+1 WHERE id=1",
            []
        )
        .is_err()
    );
    let page = store.source_page(1).await.unwrap();
    assert!(!page.complete);
    assert_eq!(page.revision, 1);
    assert!(page.events.is_empty());
    db.execute(
        "UPDATE source_units SET state='accepted',revision=revision+1 WHERE id=1",
        [],
    )
    .unwrap();
    db.execute(
        "UPDATE source_pages SET complete=1,revision=revision+1 WHERE id=1",
        [],
    )
    .unwrap();
    db.execute("INSERT INTO source_review_events(page_id,actor,rationale,state,correction,previous_revision) VALUES (1,'fixture','Synthetic','page-complete','',2)", []).unwrap();
    assert!(store.source_page(1).await.unwrap().complete);
}

#[tokio::test]
async fn matching_completion_event_rejects_legacy_unreviewed_units() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("completion-event.sqlite3");
    let store = Store::open(&path).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    schema_fixture::pre_finalization(&db, false);
    db.execute(
        "UPDATE source_pages SET complete=1,revision=2 WHERE id=1",
        [],
    )
    .unwrap();
    db.execute_batch(include_str!("../migrations/007_source_finalization.sql"))
        .unwrap();
    assert!(db.execute("INSERT INTO source_review_events(page_id,actor,rationale,state,correction,previous_revision) VALUES (1,'fixture','Synthetic','page-complete','',1)", []).is_err());
    // The schema is rewound here, so read the history directly rather than through the current store.
    assert_eq!(
        db.query_row("SELECT count(*) FROM source_review_events", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn import_block_id_bound_matches_publication_bytes() {
    for (suffix, accepted) in [
        ("a".repeat(499), true),
        ("a".repeat(500), false),
        ("é".repeat(249), true),
        ("é".repeat(250), false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
        let block_id = format!("/page/0/Text/{suffix}");
        let mut raw: serde_json::Value =
            serde_json::from_slice(&fs::read(bundle.join("source.json")).unwrap()).unwrap();
        raw["children"][0]["children"][0]["id"] = serde_json::json!(block_id);
        let bytes = serde_json::to_vec(&raw).unwrap();
        fs::write(bundle.join("source.json"), &bytes).unwrap();
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(bundle.join("bundle.json")).unwrap()).unwrap();
        manifest["pages"][0]["units"][0]["id"] = serde_json::json!(block_id);
        manifest["extraction_sha256"] = serde_json::json!(fixture::hash(&bytes));
        fs::write(
            bundle.join("bundle.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let store = Store::open(&dir.path().join("bound.sqlite3")).unwrap();
        let result = store.import_source(&bundle, &pdf).await;
        assert_eq!(
            result.is_ok(),
            accepted,
            "block_id bytes: {}",
            block_id.len()
        );
        assert_eq!(store.sources().await.unwrap().len(), usize::from(accepted));
    }
}

#[tokio::test]
async fn finalized_import_rejects_artifact_mutation_and_replacement() {
    let dir = tempfile::tempdir().unwrap();
    fixture::fixture_pair(dir.path()).unwrap();
    let path = dir.path().join("sealed.sqlite3");
    let store = Store::open(&path).unwrap();
    let id = store
        .import_source(&dir.path().join("bundle"), &dir.path().join("original.pdf"))
        .await
        .unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    assert!(
        db.execute("UPDATE sources SET pdf_hash='wrong' WHERE id=?1", [id])
            .is_err()
    );
    assert!(
        db.execute("UPDATE source_pages SET png=X'00' WHERE source_id=?1", [id])
            .is_err()
    );
    assert!(
        db.execute("UPDATE source_units SET raw_text='Changed'", [])
            .is_err()
    );
    db.execute_batch("PRAGMA recursive_triggers=OFF; PRAGMA foreign_keys=OFF;")
        .unwrap();
    for sql in [
        "INSERT OR REPLACE INTO sources SELECT * FROM sources",
        "INSERT OR REPLACE INTO source_pages SELECT * FROM source_pages",
        "INSERT OR REPLACE INTO source_units SELECT * FROM source_units",
        "INSERT OR REPLACE INTO source_finalizations SELECT * FROM source_finalizations",
        "INSERT OR REPLACE INTO source_page_hashes SELECT page_id,source_id,printf('%064d',0) FROM source_page_hashes",
        "INSERT INTO source_units(page_id,block_id,kind,polygon,raw_html,raw_text) VALUES (1,'new','Text','[]','','')",
        "DELETE FROM source_pages",
        "DELETE FROM source_units",
        "DELETE FROM sources",
        "UPDATE source_units SET revision=0",
        "UPDATE source_pages SET revision=0",
        "UPDATE source_units SET state='accepted'",
        "UPDATE source_pages SET complete=1",
    ] {
        assert!(db.execute(sql, []).is_err(), "Guard accepted {sql}");
    }
    assert_eq!(
        store
            .import_source(&dir.path().join("bundle"), &dir.path().join("original.pdf"))
            .await
            .unwrap(),
        id
    );
}

#[tokio::test]
async fn old_sources_are_unverified_until_exact_reimport_and_corruption_never_gets_sealed() {
    for sql in [
        "UPDATE sources SET title='Wrong'",
        "UPDATE sources SET source_id='wrong'",
        "UPDATE sources SET language='wrong'",
        "UPDATE sources SET edition='wrong'",
        "UPDATE sources SET system_id='dnd'",
        "UPDATE sources SET pdf_hash='0000000000000000000000000000000000000000000000000000000000000000'",
        "UPDATE sources SET raw_json=X'00'",
        "UPDATE source_pages SET png=X'00'",
        "UPDATE source_units SET raw_text='Wrong'",
        "UPDATE source_units SET polygon='[]'",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
        let path = dir.path().join("old.sqlite3");
        let store = Store::open(&path).unwrap();
        store.import_source(&bundle, &pdf).await.unwrap();
        let db = rusqlite::Connection::open(&path).unwrap();
        schema_fixture::pre_finalization(&db, true);
        db.execute("UPDATE sources SET total_pages=0", []).unwrap();
        if sql.contains("pdf_hash") {
            // The rewind keeps the document-registration guard, which refuses a hash change.
            db.execute_batch("DROP TRIGGER sources_registered_update")
                .unwrap();
        }
        db.execute(sql, []).unwrap();
        assert!(!store.sources().await.unwrap()[0].finalized);
        assert!(
            store.import_source(&bundle, &pdf).await.is_err(),
            "Corruption accepted: {sql}"
        );
        assert_eq!(store.sources().await.unwrap()[0].total_pages, 0);
        assert_eq!(
            db.query_row("SELECT count(*) FROM source_finalizations", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            0
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM source_page_hashes", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[tokio::test]
async fn reimport_seals_legacy_artifacts_without_changing_reviews_and_first_alias_is_exact() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("old.sqlite3");
    let store = Store::open(&path).unwrap();
    let id = store.import_source(&bundle, &pdf).await.unwrap();
    let source = store.sources().await.unwrap().remove(0);
    let page = store.source_page(source.pages[0].id).await.unwrap();
    store
        .review_source_unit(
            page.id,
            page.units[0].id,
            mimik::sources::UnitReview {
                revision: 1,
                state: "accepted".into(),
                correction: String::new(),
                actor: "fixture".into(),
                rationale: "Synthetic".into(),
            },
        )
        .await
        .unwrap();
    let before = store.source_page(page.id).await.unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    schema_fixture::pre_finalization(&db, true);
    db.execute("UPDATE sources SET total_pages=0", []).unwrap();
    assert_eq!(store.import_source(&bundle, &pdf).await.unwrap(), id);
    assert!(store.sources().await.unwrap()[0].finalized);
    assert_eq!(
        store.source_page(page.id).await.unwrap().revision,
        before.revision
    );
    assert_eq!(
        store.source_page(page.id).await.unwrap().events.len(),
        before.events.len()
    );
    assert!(
        db.execute(
            "INSERT INTO source_rulesets VALUES (?1,'mork-borg','it-2025')",
            [id]
        )
        .is_err()
    );
    store
        .register_ruleset(mimik::registry::RegisterRuleset {
            game_id: "mork-borg".into(),
            game_label: "MÖRK BORG".into(),
            edition_id: "fixture".into(),
            edition_label: "Fixture".into(),
        })
        .await
        .unwrap();
    store
        .register_ruleset_alias(mimik::registry::RegisterAlias {
            game_id: "mork-borg".into(),
            edition_id: "fixture".into(),
            legacy_edition: "fixture-edition".into(),
        })
        .await
        .unwrap();
    for sql in [
        "DELETE FROM source_rulesets",
        "UPDATE source_rulesets SET edition_id='it-2025'",
        "INSERT OR REPLACE INTO source_rulesets SELECT * FROM source_rulesets",
    ] {
        assert!(db.execute(sql, []).is_err());
    }
}

#[tokio::test]
async fn page_hash_ownership_completion_and_review_overflow_fail_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("meta.sqlite3");
    let store = Store::open(&path).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    let source = store.sources().await.unwrap().remove(0);
    let page = store.source_page(source.pages[0].id).await.unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    schema_fixture::pre_finalization(&db, false);
    db.execute("UPDATE source_pages SET revision=?1", [i64::MAX])
        .unwrap();
    db.execute_batch(include_str!("../migrations/007_source_finalization.sql"))
        .unwrap();
    assert!(
        db.execute(
            "INSERT INTO source_page_hashes VALUES (?1,999,?2)",
            rusqlite::params![page.id, "0".repeat(64)]
        )
        .is_err()
    );
    let prepared: String = db
        .query_row("SELECT bundle_hash FROM sources", [], |row| row.get(0))
        .unwrap();
    let extraction: String = db
        .query_row(
            "SELECT json_extract(prepared_json,'$.extraction_sha256') FROM sources",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        db.execute(
            "INSERT INTO source_finalizations VALUES (?1,?2,?3)",
            rusqlite::params![source.id, prepared, extraction]
        )
        .is_err()
    );
    let review = mimik::sources::UnitReview {
        revision: 1,
        state: "accepted".into(),
        correction: String::new(),
        actor: "fixture".into(),
        rationale: "Synthetic".into(),
    };
    assert!(
        store
            .review_source_unit(page.id, page.units[0].id, review)
            .await
            .is_err()
    );
    // The schema is rewound here, so read the rows directly rather than through the current store.
    let (revision, state): (i64, String) = db
        .query_row(
            "SELECT revision,state FROM source_units WHERE id=?1",
            [page.units[0].id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((revision, state.as_str()), (1, "unreviewed"));
    assert_eq!(
        db.query_row("SELECT count(*) FROM source_review_events", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn direct_valid_unit_review_advances_parent_page_revision_once() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("cascade.sqlite3");
    let store = Store::open(&path).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute(
        "UPDATE source_units SET state='accepted',revision=revision+1",
        [],
    )
    .unwrap();
    assert_eq!(store.source_page(1).await.unwrap().revision, 2);
}

#[tokio::test]
async fn source_review_history_is_append_only_and_relationally_consistent() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("history.sqlite3");
    let store = Store::open(&path).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    store
        .review_source_unit(
            1,
            1,
            mimik::sources::UnitReview {
                revision: 1,
                state: "accepted".into(),
                correction: String::new(),
                actor: "fixture".into(),
                rationale: "Synthetic".into(),
            },
        )
        .await
        .unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA recursive_triggers=OFF").unwrap();
    assert!(
        db.execute("UPDATE source_review_events SET actor='Changed'", [])
            .is_err()
    );
    assert!(db.execute("DELETE FROM source_review_events", []).is_err());
    assert!(
        db.execute(
            "INSERT OR REPLACE INTO source_review_events SELECT * FROM source_review_events",
            []
        )
        .is_err()
    );
    assert!(db.execute("INSERT INTO source_review_events(page_id,unit_id,actor,rationale,state,correction,previous_revision) VALUES (999,1,'fixture','Synthetic','accepted','',1)",[]).is_err());
}

#[tokio::test]
async fn finalized_duplicate_import_is_read_only_even_with_another_reader() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("reader.sqlite3");
    let store = Store::open(&path).unwrap();
    let id = store.import_source(&bundle, &pdf).await.unwrap();
    let reader = rusqlite::Connection::open(&path).unwrap();
    let other = Store::open(&path).unwrap();
    reader.execute_batch("BEGIN;").unwrap();
    reader
        .query_row("SELECT count(*) FROM sources", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    assert_eq!(store.import_source(&bundle, &pdf).await.unwrap(), id);
    let (first, second) = tokio::join!(
        store.import_source(&bundle, &pdf),
        other.import_source(&bundle, &pdf)
    );
    assert_eq!(first.unwrap(), id);
    assert_eq!(second.unwrap(), id);
    reader.execute_batch("ROLLBACK").unwrap();
}

/// Restore the interrupted-session schema7 completion triggers, preserving data.
fn legacy_completion_guards(db: &rusqlite::Connection) {
    let sql = include_str!("../migrations/007_source_finalization.sql");
    let start = sql.find("CREATE TRIGGER page_review_revision").unwrap();
    let end = sql
        .find("CREATE TRIGGER source_review_event_update")
        .unwrap();
    let completion = "    SELECT CASE WHEN NEW.complete=1 AND EXISTS(SELECT 1 FROM source_units WHERE page_id=OLD.id AND state='unreviewed')\n      THEN RAISE(ABORT,'Page completion requires every unit to be reviewed') END;\n";
    let event = "\n          AND NOT EXISTS(SELECT 1 FROM source_units u WHERE u.page_id=p.id AND u.state='unreviewed'))))";
    assert!(sql[start..end].contains(completion));
    assert!(sql[start..end].contains(event));
    db.execute_batch("DROP TRIGGER IF EXISTS page_completion_integrity; DROP TRIGGER IF EXISTS page_completion_event_integrity; DROP TRIGGER page_review_revision; DROP TRIGGER unit_review_revision; DROP TRIGGER unit_review_page_cascade; DROP TRIGGER source_review_event_insert;").unwrap();
    db.execute_batch(
        &sql[start..end]
            .replace(completion, "")
            .replace(event, ")))"),
    )
    .unwrap();
}

#[tokio::test]
async fn schema9_retrofit_schema7_rejects_completion() {
    retrofit_completion(7).await;
}

#[tokio::test]
async fn schema9_retrofit_schema8_rejects_completion() {
    retrofit_completion(8).await;
}

async fn retrofit_completion(version: i64) {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("retrofit.sqlite3");
    let store = Store::open(&path).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    schema_fixture::remove_empty_account_tables(&db);
    legacy_completion_guards(&db);
    if version == 7 {
        db.execute_batch("DROP TRIGGER IF EXISTS definition_approvals_provenance; DROP TABLE definition_withdrawals; DROP TABLE definition_approvals; DROP TABLE definition_versions;").unwrap();
    }
    db.pragma_update(None, "user_version", version).unwrap();
    drop(store);
    // Historical guards accepted revision-correct completion despite unreviewed units.
    db.execute_batch("SAVEPOINT historical").unwrap();
    db.execute(
        "UPDATE source_pages SET complete=1,revision=revision+1 WHERE id=1",
        [],
    )
    .unwrap();
    db.execute_batch("ROLLBACK TO historical; RELEASE historical")
        .unwrap();
    for _ in 0..2 {
        let reopened = Store::open(&path).unwrap();
        assert!(
            db.execute(
                "UPDATE source_pages SET complete=1,revision=revision+1 WHERE id=1",
                []
            )
            .is_err(),
            "schema{version} startup accepted invalid completion"
        );
        let page = reopened.source_page(1).await.unwrap();
        assert!(!page.complete);
        assert_eq!(page.revision, 1);
        assert!(page.events.is_empty());
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            mimik::SCHEMA_VERSION
        );
    }
}

#[tokio::test]
async fn schema9_retrofit_schema7_rejects_event_on_legacy_inconsistent_page() {
    retrofit_event(7).await;
}

#[tokio::test]
async fn schema9_retrofit_schema8_rejects_event_on_legacy_inconsistent_page() {
    retrofit_event(8).await;
}

async fn retrofit_event(version: i64) {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("legacy-event.sqlite3");
    let store = Store::open(&path).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    schema_fixture::remove_empty_account_tables(&db);
    legacy_completion_guards(&db);
    db.execute(
        "UPDATE source_pages SET complete=1,revision=revision+1 WHERE id=1",
        [],
    )
    .unwrap();
    if version == 7 {
        db.execute_batch("DROP TRIGGER IF EXISTS definition_approvals_provenance; DROP TABLE definition_withdrawals; DROP TABLE definition_approvals; DROP TABLE definition_versions;").unwrap();
    }
    db.pragma_update(None, "user_version", version).unwrap();
    drop(store);
    let event = "INSERT INTO source_review_events(page_id,actor,rationale,state,correction,previous_revision) VALUES (1,'fixture','Synthetic legacy completion','page-complete','',1)";
    db.execute_batch("SAVEPOINT historical").unwrap();
    db.execute(event, []).unwrap();
    db.execute_batch("ROLLBACK TO historical; RELEASE historical")
        .unwrap();
    for _ in 0..2 {
        let reopened = Store::open(&path).unwrap();
        assert!(
            db.execute(event, []).is_err(),
            "schema{version} startup accepted invalid event"
        );
        let page = reopened.source_page(1).await.unwrap();
        assert!(page.complete);
        assert_eq!(page.revision, 2);
        assert_eq!(page.units[0].state, "unreviewed");
        assert!(page.events.is_empty());
    }
}
