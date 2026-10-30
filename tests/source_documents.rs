//! Store-level source import schema regressions; synthetic evidence only.
use mimik::sources::{OwnerUnit, SourceSuggestion, SuggestionTarget, SuggestionUnit, UnitDecision};
use mimik::{
    Store,
    sources::{PageReview, UnitReview},
};
use rusqlite::{Connection, params};

#[path = "support/source_fixture.rs"]
pub mod fixture;
#[path = "support/schema_fixture.rs"]
mod schema_fixture;

fn owner_unit(revision: i64) -> OwnerUnit {
    OwnerUnit {
        revision,
        polygon: [[1.0, 1.0], [20.0, 1.0], [20.0, 20.0], [1.0, 20.0]],
        text: "Missing <text> & transcription".into(),
        actor: "owner".into(),
        rationale: "Read original image".into(),
    }
}

async fn lookup(store: &Store, query: &str) -> mimik::sources::EvidenceSearch {
    store
        .search_evidence("mork-borg".into(), "fixture-edition".into(), query.into())
        .await
        .unwrap()
}

fn review(revision: i64) -> UnitReview {
    UnitReview {
        revision,
        state: "accepted".into(),
        correction: String::new(),
        actor: "owner".into(),
        rationale: "Checked synthetic page".into(),
    }
}

async fn complete(store: &Store, page: i64) {
    let revision = store.source_page(page).await.unwrap().revision;
    store
        .complete_source_page(
            page,
            PageReview {
                revision,
                printed_label: "1".into(),
                actor: "owner".into(),
                rationale: "Checked omissions".into(),
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn unreviewed_owner_insert_resets_complete_page_and_blocks_completion() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("owner.sqlite3");
    let store = Store::open(&path).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    store.review_source_unit(1, 1, review(1)).await.unwrap();
    complete(&store, 1).await;
    let before = store.source_page(1).await.unwrap().revision;
    let db = Connection::open(&path).unwrap();
    db.execute("INSERT INTO source_units(page_id,block_id,kind,polygon,raw_html,raw_text,origin) VALUES(1,'owner:1','Text','[[1,1],[20,1],[20,20],[1,20]]','<p>missing</p>','missing','owner')", []).unwrap();
    let page = store.source_page(1).await.unwrap();
    assert!(!page.complete);
    assert_eq!(page.revision, before + 1);
    assert!(
        db.execute(
            "UPDATE source_pages SET complete=1,revision=revision+1 WHERE id=1",
            []
        )
        .is_err()
    );
    assert!(
        db.execute(
            "UPDATE source_units SET origin='extracted' WHERE block_id='owner:1'",
            []
        )
        .is_err()
    );
    assert!(db.execute("INSERT INTO source_units(page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(1,'new','Text','[]','','')", []).is_err());
    assert_eq!(store.import_source(&bundle, &pdf).await.unwrap(), 1);
}

#[tokio::test]
async fn import_registers_immutable_document_membership_and_primary_event() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("document.sqlite3");
    let store = Store::open(&path).unwrap();
    let source = store.import_source(&bundle, &pdf).await.unwrap();
    let db = Connection::open(&path).unwrap();
    let document: i64 = db
        .query_row(
            "SELECT document_id FROM source_document_members WHERE source_id=?1",
            [source],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        db.query_row(
            "SELECT source_id FROM source_primary_pages WHERE document_id=?1 AND pdf_page=1",
            [document],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        source
    );
    assert_eq!(
        db.query_row("SELECT actor FROM source_primary_events", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "import"
    );
    for sql in [
        "UPDATE source_document_members SET document_id=document_id",
        "DELETE FROM source_document_members",
        "INSERT OR REPLACE INTO source_document_members SELECT * FROM source_document_members",
        "DELETE FROM source_primary_events",
        "UPDATE source_primary_events SET actor='forged'",
    ] {
        assert!(db.execute(sql, []).is_err(), "{sql}");
    }
    assert_eq!(store.import_source(&bundle, &pdf).await.unwrap(), source);
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM source_primary_events WHERE document_id=?1",
            params![document],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn owner_correction_is_searchable_idempotent_and_stale_insert_is_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("owner-api.sqlite3");
    let store = Store::open(&path).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    store.review_source_unit(1, 1, review(1)).await.unwrap();
    complete(&store, 1).await;
    let revision = store.source_page(1).await.unwrap().revision;
    assert!(matches!(
        store.add_owner_unit(1, owner_unit(revision - 1)).await,
        Err(mimik::AppError::Conflict)
    ));
    let event = store.add_owner_unit(1, owner_unit(revision)).await.unwrap();
    let page = store.source_page(1).await.unwrap();
    assert!(!page.complete);
    assert_eq!(page.revision, revision + 2); // insertion and correction each invalidate the page
    assert_eq!(page.units.len(), 2);
    assert_eq!(page.units[1].state, "corrected");
    assert!(page.units[1].sanitized_html.contains("&lt;text&gt;"));
    assert_eq!(page.units[1].revision, 2);
    let db = Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT state FROM source_review_events WHERE id=?1",
            [event],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "corrected"
    );
    complete(&store, 1).await;
    let hits = lookup(&store, "transcription").await;
    assert_eq!(hits.matches.len(), 1);
    assert_eq!(hits.matches[0].text, owner_unit(1).text);
    assert_eq!(store.import_source(&bundle, &pdf).await.unwrap(), 1);
    assert_eq!(store.source_page(1).await.unwrap().units.len(), 2);
}

#[tokio::test]
async fn owner_insert_guards_reject_bad_geometry_identity_and_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("geometry.sqlite3");
    let store = Store::open(&path).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    let db = Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA recursive_triggers=OFF; PRAGMA foreign_keys=OFF;")
        .unwrap();
    for polygon in [
        "[]",
        "null",
        "[[0,0],[300,0],[300,20],[0,20]]",
        "[[0,0],[0,0],[0,0],[0,0]]",
        "[[0,0],[20,20],[20,0],[0,20]]",
        "[[null,1],[20,1],[20,20],[1,20]]",
        "[[1],[20,1],[20,20],[1,20]]",
    ] {
        assert!(db.execute("INSERT INTO source_units(page_id,block_id,kind,polygon,raw_html,raw_text,origin) VALUES(1,'owner:1','Text',?1,'<p>x</p>','x','owner')",[polygon]).is_err(),"{polygon}");
    }
    for block in [
        "owner:0",
        "owner:-1",
        "owner:1x",
        "owner:01",
        "/page/0/Text/1",
    ] {
        assert!(db.execute("INSERT INTO source_units(page_id,block_id,kind,polygon,raw_html,raw_text,origin) VALUES(1,?1,'Text','[[1,1],[20,1],[20,20],[1,20]]','<p>x</p>','x','owner')",[block]).is_err(),"{block}");
    }
    store.add_owner_unit(1, owner_unit(1)).await.unwrap();
    assert!(
        db.execute(
            "INSERT OR REPLACE INTO source_units SELECT * FROM source_units WHERE origin='owner'",
            []
        )
        .is_err()
    );
    assert_eq!(store.source_page(1).await.unwrap().units.len(), 2);
}

fn second_bundle(bundle: &std::path::Path) {
    let path = bundle.join("bundle.json");
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    json["title"] = "Second reading".into();
    std::fs::write(path, serde_json::to_vec(&json).unwrap()).unwrap();
}

#[tokio::test]
async fn primary_switch_excludes_old_reading_and_refuses_stale_and_aba_state() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("primary.sqlite3");
    let store = Store::open(&path).unwrap();
    let first = store.import_source(&bundle, &pdf).await.unwrap();
    store.review_source_unit(1, 1, review(1)).await.unwrap();
    complete(&store, 1).await;
    second_bundle(&bundle);
    let second = store.import_source(&bundle, &pdf).await.unwrap();
    store.review_source_unit(2, 2, review(1)).await.unwrap();
    complete(&store, 2).await;
    let old = store.source_primary_state(1).await.unwrap();
    assert_eq!(
        lookup(&store, "Strength")
            .await
            .matches
            .iter()
            .map(|hit| hit.source_id)
            .collect::<Vec<_>>(),
        vec![first]
    );
    store
        .set_primary_source(
            1,
            old.clone(),
            second,
            "owner".into(),
            "Choose reading".into(),
        )
        .await
        .unwrap();
    assert_eq!(
        lookup(&store, "Strength")
            .await
            .matches
            .iter()
            .map(|hit| hit.source_id)
            .collect::<Vec<_>>(),
        vec![second]
    );
    assert!(matches!(
        store
            .set_primary_source(1, old.clone(), first, "owner".into(), "Stale choice".into())
            .await,
        Err(mimik::AppError::Conflict)
    ));
    store
        .set_primary_source(
            1,
            store.source_primary_state(1).await.unwrap(),
            first,
            "owner".into(),
            "Choose original".into(),
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .set_primary_source(1, old, second, "owner".into(), "ABA choice".into())
            .await,
        Err(mimik::AppError::Conflict)
    ));
}

#[tokio::test]
async fn migration_23_preserves_reviewed_ids_hashes_and_does_not_choose_legacy_overlap() {
    for overlap in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
        let path = dir.path().join("legacy.sqlite3");
        let store = Store::open(&path).unwrap();
        store.import_source(&bundle, &pdf).await.unwrap();
        store.review_source_unit(1, 1, review(1)).await.unwrap();
        complete(&store, 1).await;
        if overlap {
            second_bundle(&bundle);
            store.import_source(&bundle, &pdf).await.unwrap();
        }
        let db = Connection::open(&path).unwrap();
        let before = retained(&db);
        schema_fixture::rewind_documents(&db);
        db.pragma_update(None, "user_version", 23).unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(retained(&db), before);
        let states = store.source_primary_state(1).await.unwrap();
        let found = lookup(&store, "Strength").await;
        if overlap {
            assert_eq!(states[0].source_id, None);
            assert_eq!(found.needs_primary_choice, 1);
            assert!(found.matches.is_empty());
            // Re-import is read-only and cannot silently resolve a legacy overlap.
            store.import_source(&bundle, &pdf).await.unwrap();
            assert_eq!(
                store.source_primary_state(1).await.unwrap()[0].source_id,
                None
            );
        } else {
            assert_eq!(states[0].source_id, Some(1));
            assert_eq!(found.matches.len(), 1);
        }
    }
}

fn retained(db: &Connection) -> Vec<String> {
    ["SELECT json_array(id,bundle_hash,pdf_hash,hex(pdf),hex(raw_json),hex(prepared_json)) FROM sources ORDER BY id", "SELECT json_array(id,page_id,block_id,state,correction,revision) FROM source_units ORDER BY id", "SELECT json_array(id,page_id,unit_id,actor,rationale,state,correction,previous_revision,created_at) FROM source_review_events ORDER BY id", "SELECT json_array(source_id,prepared_hash,extraction_hash) FROM source_finalizations ORDER BY source_id", "SELECT json_array(page_id,source_id,image_hash) FROM source_page_hashes ORDER BY page_id"]
        .into_iter().flat_map(|sql|db.prepare(sql).unwrap().query_map([],|r|r.get::<_,String>(0)).unwrap().collect::<Result<Vec<_>,_>>().unwrap()).collect()
}

#[tokio::test]
async fn atomic_batch_prechecks_all_revisions_and_rolls_back_on_hook_failure() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("batch.sqlite3");
    let store = Store::open(&path).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    store.add_owner_unit(1, owner_unit(1)).await.unwrap();
    let db = Connection::open(&path).unwrap();
    let before = retained(&db);
    let decisions = |revision| {
        vec![
            UnitDecision {
                page: 1,
                unit: 1,
                review: review(1),
            },
            UnitDecision {
                page: 1,
                unit: 2,
                review: review(revision),
            },
        ]
    };
    assert!(matches!(
        store
            .review_units_atomic(
                decisions(1),
                "stale".into(),
                "Checked".into(),
                |_, _| panic!("hook must not run before every revision is checked")
            )
            .await,
        Err(mimik::AppError::Conflict)
    ));
    assert_eq!(retained(&db), before);
    let mut called = 0;
    assert!(
        store
            .review_units_atomic(
                decisions(2),
                "failure".into(),
                "Checked".into(),
                move |tx, event| {
                    called += 1;
                    assert_eq!(
                        tx.query_row("SELECT max(id) FROM source_review_events", [], |r| r
                            .get::<_, i64>(0))
                            .unwrap(),
                        event
                    );
                    if called == 2 {
                        Err(mimik::AppError::Invalid("Synthetic hook failure"))
                    } else {
                        Ok(())
                    }
                }
            )
            .await
            .is_err()
    );
    assert_eq!(retained(&db), before);
    let events = store
        .review_units_atomic(
            decisions(2),
            "success".into(),
            "Shared rationale".into(),
            |_, _| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(db.query_row("SELECT count(*) FROM source_review_events WHERE operation_id='success' AND rationale='Shared rationale'",[],|r|r.get::<_,i64>(0)).unwrap(),2);
}

#[tokio::test]
async fn suggestions_are_bundle_bound_validate_targets_and_dismiss_without_deciding_units() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("suggestions.sqlite3");
    let store = Store::open(&path).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    let db = Connection::open(&path).unwrap();
    let before = retained(&db);
    let hash: String = db
        .query_row("SELECT bundle_hash FROM sources WHERE id=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    let suggestion = SourceSuggestion {
        source: 1,
        bundle_hash: hash,
        origin: "reviewer".into(),
        kind: "correction".into(),
        target: SuggestionTarget::Unit {
            target: SuggestionUnit {
                unit: 1,
                revision: 1,
            },
        },
        suggested_text: Some("Advice".into()),
        rationale: "Possible reading".into(),
    };
    let mut wrong = suggestion.clone();
    wrong.bundle_hash = "0".repeat(64);
    assert!(store.insert_source_suggestion(wrong).await.is_err());
    let mut wrong = suggestion.clone();
    wrong.target = SuggestionTarget::Unit {
        target: SuggestionUnit {
            unit: 1,
            revision: 2,
        },
    };
    assert!(store.insert_source_suggestion(wrong).await.is_err());
    let id = store
        .insert_source_suggestion(suggestion.clone())
        .await
        .unwrap();
    let mut region = suggestion;
    region.target = SuggestionTarget::Region {
        page: 1,
        polygon: owner_unit(1).polygon,
    };
    store.insert_source_suggestion(region).await.unwrap();
    store
        .dismiss_source_suggestion(id, "owner".into())
        .await
        .unwrap();
    assert!(matches!(
        store.dismiss_source_suggestion(id, "owner".into()).await,
        Err(mimik::AppError::Conflict)
    ));
    assert_eq!(retained(&db), before);
    assert_eq!(
        db.query_row(
            "SELECT dismissed_by FROM source_suggestions WHERE id=?1",
            [id],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "owner"
    );
}
