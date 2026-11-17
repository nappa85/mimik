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
            .review_units_atomic(decisions(1), "Checked".into(), |_, _| panic!(
                "hook must not run before every revision is checked"
            ))
            .await,
        Err(mimik::AppError::Conflict)
    ));
    assert_eq!(retained(&db), before);
    let mut called = 0;
    assert!(
        store
            .review_units_atomic(decisions(2), "Checked".into(), move |tx, event| {
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
            })
            .await
            .is_err()
    );
    assert_eq!(retained(&db), before);
    let events = store
        .review_units_atomic(decisions(2), "Shared rationale".into(), |_, _| Ok(()))
        .await
        .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(db.query_row("SELECT count(*) FROM source_review_events WHERE operation_id LIKE 'batch-%' AND rationale='Shared rationale'",[],|r|r.get::<_,i64>(0)).unwrap(),2);
    // The id is generated per batch and exposed on the page history.
    let ids: std::collections::BTreeSet<_> = store
        .source_page(1)
        .await
        .unwrap()
        .events
        .iter()
        .filter_map(|e| e.operation_id.clone())
        .collect();
    assert_eq!(ids.len(), 1);
    let again = store
        .review_units_atomic(
            vec![UnitDecision {
                page: 1,
                unit: 1,
                review: UnitReview {
                    revision: 2,
                    state: "rejected".into(),
                    ..review(2)
                },
            }],
            "Second batch".into(),
            |_, _| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(again.len(), 1);
    assert_eq!(
        db.query_row(
            "SELECT count(DISTINCT operation_id) FROM source_review_events",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
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
        report_id: None,
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
    region.kind = "missing_text".into();
    region.target = SuggestionTarget::Region {
        page: 1,
        polygon: owner_unit(1).polygon,
    };
    store.insert_source_suggestion(region).await.unwrap();
    store
        .dismiss_source_suggestion(id, "owner".into(), None)
        .await
        .unwrap();
    assert!(matches!(
        store
            .dismiss_source_suggestion(id, "owner".into(), None)
            .await,
        Err(mimik::AppError::Conflict)
    ));
    assert_eq!(retained(&db), before);
    assert_eq!(
        db.query_row(
            "SELECT actor FROM source_suggestion_dismissals WHERE suggestion_id=?1",
            [id],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "owner"
    );
}

fn titled_bundle(bundle: &std::path::Path, title: &str) {
    let path = bundle.join("bundle.json");
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    json["title"] = title.into();
    std::fs::write(path, serde_json::to_vec(&json).unwrap()).unwrap();
}

const RAW_SOURCE: &str = "INSERT INTO sources (id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages) VALUES (1,'b','s','t','mork-borg','e','it','aa',x'00','{}','{}',1)";

#[tokio::test]
async fn registered_source_hash_and_identity_cannot_change_and_unfinalized_cannot_be_selected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("registered.sqlite3");
    drop(Store::open(&path).unwrap());
    let db = Connection::open(&path).unwrap();
    db.execute_batch(RAW_SOURCE).unwrap();
    db.execute_batch("INSERT INTO source_pages (id,source_id,pdf_page,png) VALUES (1,1,1,x'00'); INSERT INTO source_documents(pdf_sha256) VALUES ('aa'); INSERT INTO source_document_members VALUES (1,1);").unwrap();
    db.execute_batch("INSERT INTO sources (id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages) VALUES (2,'c','s','t','mork-borg','e','it','cc',x'00','{}','{}',1)").unwrap();
    for sql in [
        "UPDATE sources SET pdf_hash='bb' WHERE id=1",
        "UPDATE sources SET id=9 WHERE id=1",
        "DELETE FROM sources WHERE id=1",
        "INSERT OR REPLACE INTO sources (id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages) VALUES (1,'b2','s','t','mork-borg','e','it','bb',x'00','{}','{}',1)",
        // Replacement through the other unique key, bundle_hash (recursive triggers are off).
        "INSERT OR REPLACE INTO sources (id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages) VALUES (3,'b','s','t','mork-borg','e','it','cc',x'00','{}','{}',1)",
        "UPDATE OR REPLACE sources SET bundle_hash='b' WHERE id=2",
        "UPDATE sources SET bundle_hash='b3' WHERE id=1",
        // Legacy unfinalized sources cannot be newly selected (only finalized readings).
        "INSERT INTO source_primary_events(document_id,pdf_page,source_id,actor,rationale) VALUES (1,1,1,'owner','Choose')",
    ] {
        assert!(db.execute(sql, []).is_err(), "{sql}");
    }
    assert_eq!(
        db.query_row("SELECT pdf_hash FROM sources WHERE id=1", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "aa"
    );
}

fn suggestion_sql(
    db: &Connection,
    kind: &str,
    targets: &str,
    text: Option<&str>,
) -> rusqlite::Result<usize> {
    let hash: String = db
        .query_row("SELECT bundle_hash FROM sources WHERE id=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    db.execute(
        "INSERT INTO source_suggestions(source_id,bundle_hash,origin,kind,targets,suggested_text,rationale) VALUES (1,?1,'tool',?2,?3,?4,'why')",
        params![hash, kind, targets, text],
    )
}

fn unit_target(u: i64, r: i64) -> String {
    format!(r#"{{"type":"unit","target":{{"unit":{u},"revision":{r}}}}}"#)
}
fn units_target(a: &str) -> String {
    format!(r#"{{"type":"units","targets":[{a}]}}"#)
}
fn region_target(page: &str, polygon: &str) -> String {
    format!(r#"{{"type":"region","page":{page},"polygon":{polygon}}}"#)
}

type Bad = Vec<(&'static str, String, Option<&'static str>)>;

fn rejected_suggestions() -> Bad {
    let (unit, units, region) = (unit_target, units_target, region_target);
    let good = "[[1,1],[20,1],[20,20],[1,20]]";
    vec![
        ("bogus", unit(1, 1), Some("t")),
        ("correction", unit(1, 1), None),
        ("correction", unit(1, 1), Some("")),
        ("correction", unit(3, 1), Some("t")), // unit of another source
        ("correction", unit(1, 2), Some("t")), // newer than current
        ("correction", unit(1, 0), Some("t")),
        ("correction", unit(999, 1), Some("t")),
        (
            "correction",
            units(&format!(
                "{},{}",
                r#"{"unit":1,"revision":1}"#, r#"{"unit":2,"revision":1}"#
            )),
            Some("t"),
        ),
        ("correction", region("1", good), Some("t")),
        ("merge", unit(1, 1), Some("t")),
        ("merge", units(r#"{"unit":1,"revision":1}"#), Some("t")),
        (
            "merge",
            units(&format!(
                "{},{}",
                r#"{"unit":1,"revision":1}"#, r#"{"unit":1,"revision":1}"#
            )),
            Some("t"),
        ),
        (
            "merge",
            units(&format!(
                "{},{}",
                r#"{"unit":1,"revision":1}"#, r#"{"unit":2,"revision":1}"#
            )),
            None,
        ),
        (
            "merge",
            units(&format!(
                "{},{}",
                r#"{"unit":1,"revision":1}"#, r#"{"unit":3,"revision":1}"#
            )),
            Some("t"),
        ),
        ("missing_text", region("1", good), None),
        ("missing_text", region("2", good), Some("t")), // page of another source
        (
            "missing_text",
            region("1", "[[0,0],[300,0],[300,20],[0,20]]"),
            Some("t"),
        ),
        (
            "missing_text",
            region("1", "[[0,0],[20,20],[20,0],[0,20]]"),
            Some("t"),
        ),
        (
            "missing_text",
            region("1", "[[1,1],[2,2],[3,3],[4,4]]"),
            Some("t"),
        ),
        ("missing_text", region("\"1\"", good), Some("t")),
        ("missing_text", unit(1, 1), Some("t")),
        ("reject", unit(1, 1), Some("t")),
        ("reject", region("1", good), None),
        ("reject", "[]".into(), None),
        // Missing or null fields must fail closed, not evaluate to NULL and pass.
        ("correction", "{}".into(), Some("t")),
        ("correction", r#"{"type":null}"#.into(), Some("t")),
        ("correction", r#"{"type":"unit"}"#.into(), Some("t")),
        ("merge", r#"{"type":"units"}"#.into(), Some("t")),
        (
            "missing_text",
            r#"{"type":"region","page":1}"#.into(),
            Some("t"),
        ),
        (
            "missing_text",
            format!(r#"{{"type":"region","polygon":{good}}}"#),
            Some("t"),
        ),
        (
            "missing_text",
            r#"{"type":"region","page":1,"polygon":{}}"#.into(),
            Some("t"),
        ),
        (
            "missing_text",
            r#"{"type":"region","page":1,"polygon":null}"#.into(),
            Some("t"),
        ),
    ]
}

#[tokio::test]
async fn suggestion_targets_and_kinds_are_enforced_in_sql() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("envelope.sqlite3");
    let store = Store::open(&path).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    store.add_owner_unit(1, owner_unit(1)).await.unwrap(); // unit 2, revision 2
    second_bundle(&bundle);
    store.import_source(&bundle, &pdf).await.unwrap(); // source 2, unit 3
    let db = Connection::open(&path).unwrap();
    let bad = rejected_suggestions();
    for (kind, targets, text) in &bad {
        assert!(
            suggestion_sql(&db, kind, targets, *text).is_err(),
            "{kind} {targets} {text:?}"
        );
    }
    let (unit, units, region) = (unit_target, units_target, region_target);
    let good = "[[1,1],[20,1],[20,20],[1,20]]";
    for (kind, targets, text) in [
        ("correction", unit(1, 1), Some("t")),
        (
            "merge",
            units(&format!(
                "{},{}",
                r#"{"unit":1,"revision":1}"#, r#"{"unit":2,"revision":2}"#
            )),
            Some("t"),
        ),
        ("missing_text", region("1", good), Some("t")),
        ("reject", unit(2, 2), None),
    ] {
        suggestion_sql(&db, kind, &targets, text).unwrap_or_else(|e| panic!("{kind}: {e}"));
    }
}

#[tokio::test]
async fn merge_suggestions_must_stay_on_one_page() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("merge.sqlite3");
    drop(Store::open(&path).unwrap());
    let db = Connection::open(&path).unwrap();
    db.execute_batch(RAW_SOURCE).unwrap();
    db.execute_batch("INSERT INTO source_pages (id,source_id,pdf_page,png) VALUES (1,1,1,x'00'),(2,1,2,x'00'); INSERT INTO source_units (id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES (1,1,'a','Text','[]','','x'),(2,1,'b','Text','[]','','x'),(3,2,'c','Text','[]','','x');").unwrap();
    let two = |a: i64, b: i64| {
        format!(
            r#"{{"type":"units","targets":[{{"unit":{a},"revision":1}},{{"unit":{b},"revision":1}}]}}"#
        )
    };
    assert!(suggestion_sql(&db, "merge", &two(1, 3), Some("t")).is_err());
    suggestion_sql(&db, "merge", &two(1, 2), Some("t")).unwrap();
}

#[tokio::test]
async fn sql_region_checks_require_numeric_page_dimensions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dimensions.sqlite3");
    drop(Store::open(&path).unwrap());
    let db = Connection::open(&path).unwrap();
    db.execute_batch(r#"INSERT INTO sources (id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages) VALUES (1,'b','s','t','mork-borg','e','it','aa',x'00','{}','{"pages":[{"pdf_page":1,"width_points":true,"height_points":300},{"pdf_page":2,"width_points":200,"height_points":300}]}',2);
        INSERT INTO source_pages (id,source_id,pdf_page,png) VALUES (1,1,1,x'00'),(2,1,2,x'00');"#).unwrap();
    // JSON true extracts as 1, so a bare range check would accept this tiny polygon.
    let tiny = "[[0,0],[1,0],[1,1],[0,1]]";
    assert!(suggestion_sql(&db, "missing_text", &region_target("1", tiny), Some("t")).is_err());
    suggestion_sql(&db, "missing_text", &region_target("2", tiny), Some("t")).unwrap();
}

#[tokio::test]
async fn dismissal_timestamp_must_be_a_valid_datetime() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("dismiss.sqlite3");
    let store = Store::open(&path).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    let db = Connection::open(&path).unwrap();
    suggestion_sql(
        &db,
        "reject",
        r#"{"type":"unit","target":{"unit":1,"revision":1}}"#,
        None,
    )
    .unwrap();
    assert!(
        db.execute(
            "INSERT INTO source_suggestion_dismissals(suggestion_id,actor,created_at) VALUES(1,'o','not a date')",
            []
        )
        .is_err()
    );
    db.execute(
        "INSERT INTO source_suggestion_dismissals(suggestion_id,actor) VALUES(1,'o')",
        [],
    )
    .unwrap();
}

#[tokio::test]
async fn owner_block_ids_reject_nul_suffix_and_overflowing_ordinals() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("block.sqlite3");
    let store = Store::open(&path).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    let db = Connection::open(&path).unwrap();
    for block in [
        "'owner:1'||char(0)||'x'",
        "'owner:1'||char(0)",
        "'owner:99999999999999999999'",
        "'owner:9223372036854775808'",
    ] {
        assert!(db.execute(&format!("INSERT INTO source_units(page_id,block_id,kind,polygon,raw_html,raw_text,origin) VALUES(1,{block},'Text','[[1,1],[20,1],[20,20],[1,20]]','<p>x</p>','x','owner')"), []).is_err(), "{block}");
    }
    // The largest ordinal is storable but exhausts the allocator instead of wrapping.
    db.execute("INSERT INTO source_units(page_id,block_id,kind,polygon,raw_html,raw_text,origin) VALUES(1,'owner:9223372036854775807','Text','[[1,1],[20,1],[20,20],[1,20]]','<p>x</p>','x','owner')", []).unwrap();
    let revision = store.source_page(1).await.unwrap().revision;
    assert!(matches!(
        store.add_owner_unit(1, owner_unit(revision)).await,
        Err(mimik::AppError::Invalid(_))
    ));
}

#[tokio::test]
async fn operation_ids_are_reusable_only_by_the_latest_matching_event() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("operation.sqlite3");
    let store = Store::open(&path).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    store.add_owner_unit(1, owner_unit(1)).await.unwrap(); // unit 2 rev 2
    let db = Connection::open(&path).unwrap();
    let event = |db: &Connection,
                 unit: i64,
                 rev: i64,
                 state: &str,
                 actor: &str,
                 rationale: &str,
                 op: Option<&str>| {
        db.execute(
            "UPDATE source_units SET state=?1,correction='',revision=?2 WHERE id=?3",
            params![state, rev + 1, unit],
        )?;
        db.execute("INSERT INTO source_review_events(page_id,unit_id,actor,rationale,state,correction,previous_revision,operation_id) VALUES(1,?1,?2,?3,?4,'',?5,?6)", params![unit, actor, rationale, state, rev, op])
    };
    event(&db, 1, 1, "accepted", "a", "r", Some("op-a")).unwrap(); // unused id
    event(&db, 2, 2, "accepted", "a", "r", Some("op-a")).unwrap(); // latest, same actor/rationale
    assert!(event(&db, 1, 2, "rejected", "other", "r", Some("op-a")).is_err()); // actor differs
    assert!(event(&db, 1, 2, "rejected", "a", "different", Some("op-a")).is_err()); // rationale differs
    event(&db, 1, 2, "rejected", "a", "r", None).unwrap();
    assert!(event(&db, 2, 3, "rejected", "a", "r", Some("op-a")).is_err()); // not the latest event
    event(&db, 2, 3, "rejected", "a", "r", Some("op-b")).unwrap();
}

#[tokio::test]
async fn third_import_onto_a_legacy_overlap_leaves_the_page_unresolved() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("third.sqlite3");
    let store = Store::open(&path).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    second_bundle(&bundle);
    store.import_source(&bundle, &pdf).await.unwrap();
    let db = Connection::open(&path).unwrap();
    schema_fixture::rewind_documents(&db);
    db.pragma_update(None, "user_version", 23).unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store.source_primary_state(1).await.unwrap()[0].source_id,
        None
    );
    titled_bundle(&bundle, "Third reading");
    assert_eq!(store.import_source(&bundle, &pdf).await.unwrap(), 3);
    assert_eq!(
        store.source_primary_state(1).await.unwrap()[0].source_id,
        None
    );
}

#[test]
fn page_lists_parse_into_distinct_ascending_pages() {
    assert_eq!(
        mimik::sources::parse_page_list("3,5-7").unwrap(),
        vec![3, 5, 6, 7]
    );
    assert_eq!(
        mimik::sources::parse_page_list(" 1 - 2 ").unwrap(),
        vec![1, 2]
    );
    for bad in [
        "", "0", "-1", "a", "3,3", "5,4", "2-1", "1-2,2", "1-5000", "1,,2", "1-",
    ] {
        assert!(mimik::sources::parse_page_list(bad).is_err(), "{bad}");
    }
}

async fn legacy_overlap(dir: &std::path::Path) -> (Store, std::path::PathBuf, std::path::PathBuf) {
    let (bundle, pdf) = fixture::fixture(dir).unwrap();
    let path = dir.join("overlap.sqlite3");
    let store = Store::open(&path).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    store.review_source_unit(1, 1, review(1)).await.unwrap();
    complete(&store, 1).await;
    second_bundle(&bundle);
    store.import_source(&bundle, &pdf).await.unwrap();
    let db = Connection::open(&path).unwrap();
    schema_fixture::rewind_documents(&db);
    db.pragma_update(None, "user_version", 23).unwrap();
    drop(store);
    (Store::open(&path).unwrap(), bundle, pdf)
}

#[tokio::test]
async fn operator_command_says_an_unfinalized_source_must_be_finalized() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("unfinalized.sqlite3");
    drop(Store::open(&path).unwrap());
    let hash = "a".repeat(64);
    let db = Connection::open(&path).unwrap();
    db.execute_batch(&RAW_SOURCE.replace("'aa'", &format!("'{hash}'")))
        .unwrap();
    db.execute_batch(&format!("INSERT INTO source_pages (id,source_id,pdf_page,png) VALUES (1,1,1,x'00'); INSERT INTO source_documents(pdf_sha256) VALUES ('{hash}'); INSERT INTO source_document_members VALUES (1,1);")).unwrap();
    drop(db);
    let store = Store::open(&path).unwrap();
    let result = store
        .operator_set_primary_source(hash.clone(), vec![1], 1, "Choose".into())
        .await;
    assert!(
        matches!(&result, Err(mimik::AppError::Invalid(message)) if message.contains("finalized")),
        "{result:?}"
    );
    // A source outside the document is still simply not found.
    let result = store
        .operator_set_primary_source(hash, vec![1], 9, "Choose".into())
        .await;
    assert!(
        matches!(result, Err(mimik::AppError::SourceNotFound)),
        "{result:?}"
    );
}

#[tokio::test]
async fn operator_command_resolves_unresolved_pages_with_its_own_actor() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _bundle, _pdf) = legacy_overlap(dir.path()).await;
    let hash: String = Connection::open(dir.path().join("overlap.sqlite3"))
        .unwrap()
        .query_row("SELECT pdf_sha256 FROM source_documents", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        store.source_primary_state(1).await.unwrap()[0].source_id,
        None
    );
    for (digest, pages, source) in [
        ("0".repeat(64), vec![1], 2),
        (hash.to_uppercase(), vec![1], 2),
        (hash.clone(), vec![2], 2),  // page not in the document
        (hash.clone(), vec![1], 99), // source not in the document
        (hash.clone(), vec![1, 1], 2),
    ] {
        assert!(
            store
                .operator_set_primary_source(digest, pages, source, "Choose".into())
                .await
                .is_err()
        );
    }
    assert!(
        store
            .operator_set_primary_source(hash.clone(), vec![1], 2, " ".into())
            .await
            .is_err()
    );
    let events = store
        .operator_set_primary_source(hash, vec![1], 2, "Second reading is cleaner".into())
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        store.source_primary_state(1).await.unwrap()[0].source_id,
        Some(2)
    );
    let actor: String = Connection::open(dir.path().join("overlap.sqlite3"))
        .unwrap()
        .query_row(
            "SELECT actor FROM source_primary_events WHERE id=?1",
            [events[0]],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(actor, "operator:cli");
}

#[tokio::test]
async fn search_page_shows_unresolved_choice_apart_from_unsupported_source() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let dir = tempfile::tempdir().unwrap();
    let (store, _bundle, _pdf) = legacy_overlap(dir.path()).await;
    let found = lookup(&store, "Strength").await;
    assert_eq!((found.needs_primary_choice, found.matches.len()), (1, 0));
    assert!(
        found.source_count > 0,
        "member sources count even when no page is chosen"
    );
    let get = |uri: String| {
        let app = mimik::app(store.clone());
        async move {
            let response = app
                .oneshot(
                    Request::builder()
                        .uri(uri)
                        .header("host", "localhost:3000")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            String::from_utf8(
                to_bytes(response.into_body(), 200_000)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap()
        }
    };
    let html =
        get("/sources/search?system=mork-borg&edition=fixture-edition&query=Strength".into()).await;
    assert!(html.contains("Primary reading not chosen"), "{html}");
    assert!(html.contains("href=\"/admin/materials/sources/1\""));
    assert!(html.contains("href=\"/admin/materials/sources/2\""));
    assert!(!html.contains("Unsupported source"), "{html}");
    let html =
        get("/sources/search?system=mork-borg&edition=nonexistent&query=Strength".into()).await;
    assert!(html.contains("Unsupported source"), "{html}");
}

fn seed_source(
    db: &Connection,
    id: i64,
    edition: &str,
    ruleset: Option<&str>,
    pdf: &str,
    total: i64,
    pages: &[(i64, &str, bool)],
) {
    db.execute("INSERT INTO sources (id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages) VALUES (?1,?2,'synthetic',?3,'mork-borg',?4,'it',?5,x'00','{}','{}',?6)", params![id, format!("bundle-{id}"), format!("Synthetic {id}"), edition, pdf, total]).unwrap();
    if let Some(edition_id) = ruleset {
        db.execute(
            "INSERT INTO source_rulesets VALUES (?1,'mork-borg',?2)",
            params![id, edition_id],
        )
        .unwrap();
    }
    for (number, text, complete) in pages {
        let page = id * 100 + number;
        db.execute("INSERT INTO source_pages (id,source_id,pdf_page,png,complete,printed_label) VALUES (?1,?2,?3,x'00',?4,'p')", params![page, id, number, complete]).unwrap();
        db.execute("INSERT INTO source_units (id,page_id,block_id,kind,polygon,raw_html,raw_text,state) VALUES (?1,?1,'b','Text','[[0,0],[1,0],[1,1],[0,1]]',?2,?3,'accepted')", params![page, format!("<p>{text}</p>"), text]).unwrap();
    }
}

fn seeded_coverage_database(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("coverage.sqlite3");
    drop(Store::open(&path).unwrap());
    let db = Connection::open(&path).unwrap();
    schema_fixture::drop_search_schema(&db);
    db.execute_batch(&format!("DROP TABLE catalogue_name_fold; DROP TABLE catalogue_entry_names; DROP TABLE catalogue_names_fts; DROP TABLE catalogue_seals; DROP TABLE catalogue_entry_units; DROP TABLE catalogue_entries; DROP TABLE catalogue_entry_types; DROP TABLE catalogue_versions; DROP TABLE catalogues; DROP TABLE catalogue_drafts; DROP TABLE private_sheet_notes; DROP TABLE assistant_downloads; DROP TABLE assistant_model_choice; DROP TABLE assistant_models; PRAGMA user_version={}; PRAGMA foreign_keys=ON;", mimik::sources::SEARCH_SCHEMA - 1)).unwrap();
    // One PDF of three pages read in two sources: page 1 only in 1, page 3 only in 2, page 2 in both.
    seed_source(
        &db,
        1,
        "e",
        Some("it-2025"),
        "pdf-x",
        3,
        &[(1, "alpha", true), (2, "gamma", true)],
    );
    seed_source(
        &db,
        2,
        "e",
        Some("it-2025"),
        "pdf-x",
        3,
        &[(2, "gamma", true), (3, "beta", true)],
    );
    // Legacy labels: an overlapping pair and a unique reading in another edition.
    seed_source(
        &db,
        3,
        "legacy-overlap",
        None,
        "pdf-y",
        1,
        &[(1, "delta", true)],
    );
    seed_source(
        &db,
        4,
        "legacy-overlap",
        None,
        "pdf-y",
        1,
        &[(1, "delta", true)],
    );
    seed_source(
        &db,
        5,
        "legacy-unique",
        None,
        "pdf-z",
        1,
        &[(1, "epsilon", true)],
    );
    path
}

#[tokio::test]
async fn coverage_counters_follow_primary_pages_across_split_readings_and_overlaps() {
    let dir = tempfile::tempdir().unwrap();
    let path = seeded_coverage_database(dir.path());
    let store = Store::open(&path).unwrap();
    let registered = |query: &'static str, set: Option<Vec<i64>>| {
        let store = store.clone();
        async move {
            store
                .search_ruleset_sources("mork-borg".into(), "it-2025".into(), set, query.into())
                .await
                .unwrap()
        }
    };
    let counters = |r: &mimik::sources::EvidenceSearch| {
        (
            r.source_count,
            r.incomplete_pages,
            r.partial_runs,
            r.needs_primary_choice,
        )
    };
    let alpha = registered("alpha", None).await;
    assert_eq!(alpha.matches.len(), 1);
    assert_eq!(counters(&alpha), (2, 0, 1, 1));
    assert_eq!(registered("beta", None).await.matches.len(), 1);
    assert!(
        registered("gamma", None).await.matches.is_empty(),
        "unresolved page is excluded"
    );
    // A restricted set only filters matches: source 1 is primary for alpha but outside {2}.
    let restricted = registered("alpha", Some(vec![2])).await;
    assert!(restricted.matches.is_empty());
    assert_eq!(counters(&restricted), (2, 0, 1, 1));
    assert_eq!(registered("beta", Some(vec![2])).await.matches.len(), 1);
    // Legacy labels use the same rules.
    let delta = lookup_label(&store, "legacy-overlap", "delta").await;
    assert!(delta.matches.is_empty());
    assert_eq!(counters(&delta), (2, 0, 0, 1));
    let epsilon = lookup_label(&store, "legacy-unique", "epsilon").await;
    assert_eq!(epsilon.matches.len(), 1);
    assert_eq!(counters(&epsilon), (1, 0, 0, 0));
    // Unfinalized legacy readings keep their migrated selections but cannot be newly selected.
    let document: i64 = Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT id FROM source_documents WHERE pdf_sha256='pdf-x'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let state: Vec<_> = store
        .source_primary_state(document)
        .await
        .unwrap()
        .into_iter()
        .filter(|p| p.pdf_page == 2)
        .collect();
    assert!(matches!(
        store
            .set_primary_source(document, state, 2, "owner".into(), "Second reading".into())
            .await,
        Err(mimik::AppError::Invalid(message)) if message.contains("finalized")
    ));
    assert_eq!(counters(&registered("gamma", None).await), (2, 0, 1, 1));
}

#[tokio::test]
async fn sql_and_rust_polygon_validation_agree_on_edge_cases() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("agree.sqlite3");
    let store = Store::open(&path).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    let db = Connection::open(&path).unwrap();
    // The fixture page is 200 x 300 points.
    let cases: [([[f64; 2]; 4], bool); 11] = [
        (
            [[0.0, 0.0], [200.0, 0.0], [200.0, 300.0], [0.0, 300.0]],
            true,
        ), // exactly the page boundary
        (
            [[0.0, 0.0], [200.0001, 0.0], [200.0, 300.0], [0.0, 300.0]],
            false,
        ),
        (
            [[-0.0001, 0.0], [20.0, 0.0], [20.0, 20.0], [0.0, 20.0]],
            false,
        ),
        ([[1.0, 1.0], [20.0, 1.0], [20.0, 20.0], [1.0, 20.0]], true), // counter-clockwise
        ([[1.0, 1.0], [1.0, 20.0], [20.0, 20.0], [20.0, 1.0]], true), // clockwise
        (
            [[10.0, 10.0], [20.0, 10.0], [30.0, 10.0], [10.0, 20.0]],
            false,
        ), // collinear points
        ([[0.0, 0.0], [10.0, 0.0], [20.0, 0.0], [20.0, 20.0]], false),
        ([[0.0, 0.0], [20.0, 20.0], [20.0, 0.0], [0.0, 20.0]], false), // crossed
        ([[0.0, 0.0], [20.0, 0.0], [5.0, 5.0], [0.0, 20.0]], false),   // concave
        ([[1.0, 1.0], [1.0, 1.0], [20.0, 20.0], [1.0, 20.0]], false),  // repeated point
        ([[5.0, 5.0], [5.0, 5.0], [5.0, 5.0], [5.0, 5.0]], false),
    ];
    for (polygon, accepted) in cases {
        let json = serde_json::to_string(&polygon).unwrap();
        db.execute_batch("BEGIN").unwrap();
        let sql = db.execute("INSERT INTO source_units(page_id,block_id,kind,polygon,raw_html,raw_text,origin) VALUES(1,'owner:900','Text',?1,'<p>x</p>','x','owner')", [&json]);
        db.execute_batch("ROLLBACK").unwrap();
        let revision = store.source_page(1).await.unwrap().revision;
        let rust = store
            .add_owner_unit(
                1,
                OwnerUnit {
                    polygon,
                    ..owner_unit(revision)
                },
            )
            .await;
        assert_eq!(sql.is_ok(), accepted, "SQL {json}");
        assert_eq!(rust.is_ok(), accepted, "Rust {json}: {rust:?}");
    }
}

async fn lookup_label(store: &Store, edition: &str, query: &str) -> mimik::sources::EvidenceSearch {
    store
        .search_evidence("mork-borg".into(), edition.into(), query.into())
        .await
        .unwrap()
}
