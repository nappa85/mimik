use mimik::{
    Store,
    definitions::{ApprovalRequest, DefinitionError, WiringStatus},
    registry::RegisterRuleset,
};
use serde_json::json;
#[path = "support/source_fixture.rs"]
mod fixture;
#[path = "support/schema_fixture.rs"]
mod schema_fixture;
static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn invalid_approval_envelopes() -> [(&'static str, &'static str); 18] {
    [
        ("missing draft", "'draft',999999,1,snapshot"),
        ("wrong draft revision", "'draft',target_id,2,snapshot"),
        ("wrong version id", "'version',999999,1,snapshot"),
        ("wrong version number", "'version',version_id,2,snapshot"),
        (
            "empty envelope",
            "target_kind,target_id,target_revision,'{}'",
        ),
        (
            "wrong encoding",
            "target_kind,target_id,target_revision,json_set(snapshot,'$.encoding',2)",
        ),
        (
            "wrong game",
            "target_kind,target_id,target_revision,json_set(snapshot,'$.ruleset.game_id','other')",
        ),
        (
            "wrong edition",
            "target_kind,target_id,target_revision,json_set(snapshot,'$.ruleset.edition_id','other')",
        ),
        (
            "wrong definition",
            "target_kind,target_id,target_revision,json_set(snapshot,'$.definition_id','other')",
        ),
        (
            "wrong hash",
            "target_kind,target_id,target_revision,json_set(snapshot,'$.content_hash','other')",
        ),
        (
            "missing sources",
            "target_kind,target_id,target_revision,json_remove(snapshot,'$.sources')",
        ),
        (
            "object pages",
            "target_kind,target_id,target_revision,json_set(snapshot,'$.pages',json('{}'))",
        ),
        (
            "empty units",
            "target_kind,target_id,target_revision,json_set(snapshot,'$.units',json('[]'))",
        ),
        (
            "null unresolved",
            "target_kind,target_id,target_revision,json_set(snapshot,'$.unresolved',NULL)",
        ),
        (
            "extra envelope key",
            "target_kind,target_id,target_revision,json_set(snapshot,'$.extra',1)",
        ),
        (
            "extra ruleset key",
            "target_kind,target_id,target_revision,json_set(snapshot,'$.ruleset.extra',1)",
        ),
        (
            "string encoding",
            "target_kind,target_id,target_revision,json_set(snapshot,'$.encoding','1')",
        ),
        (
            "real encoding",
            "target_kind,target_id,target_revision,json_set(snapshot,'$.encoding',1.0)",
        ),
    ]
}

fn provenance_rejected(result: rusqlite::Result<usize>) -> bool {
    match result {
        Ok(_) => false,
        Err(rusqlite::Error::SqliteFailure(_, Some(message))) => {
            assert_eq!(message, "Invalid approval provenance or snapshot envelope");
            true
        }
        Err(error) => panic!("Unexpected SQL failure: {error}"),
    }
}

#[tokio::test]
async fn ordinary_sql_approval_insert_rejects_invalid_targets_and_snapshot_envelopes() {
    let _guard = LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (store, draft, _, _) = setup(dir.path()).await;
    let check = store.compare_definition(draft, false).await.unwrap();
    let version = store
        .approve_definition(draft, false, request(check.comparison_hash))
        .await
        .unwrap();
    let db = rusqlite::Connection::open(dir.path().join("publication.sqlite3")).unwrap();
    let mut accepted = Vec::new();
    for (name, replacement) in invalid_approval_envelopes() {
        let sql = format!(
            "INSERT INTO definition_approvals(version_id,encoding,snapshot_hash,actor,rationale,originals_compared,comparison_encoding,comparison_hash,target_kind,target_id,target_revision,snapshot) SELECT version_id,encoding,snapshot_hash,actor,rationale,originals_compared,comparison_encoding,comparison_hash,{replacement} FROM definition_approvals WHERE id=?1"
        );
        db.execute_batch("SAVEPOINT rejection").unwrap();
        if !provenance_rejected(db.execute(&sql, [version.approval_id])) {
            accepted.push(name);
        }
        db.execute_batch("ROLLBACK TO rejection; RELEASE rejection")
            .unwrap();
    }
    let snapshot: String = db
        .query_row(
            "SELECT snapshot FROM definition_approvals WHERE id=?1",
            [version.approval_id],
            |row| row.get(0),
        )
        .unwrap();
    for (field, limit) in [
        ("sources", 32),
        ("pages", 1024),
        ("units", 1024),
        ("unresolved", 256),
    ] {
        let mut oversized: serde_json::Value = serde_json::from_str(&snapshot).unwrap();
        oversized[field] = json!(vec![json!({}); limit + 1]);
        db.execute_batch("SAVEPOINT rejection").unwrap();
        if !provenance_rejected(db.execute("INSERT INTO definition_approvals(version_id,encoding,snapshot,snapshot_hash,actor,rationale,originals_compared,target_kind,target_id,target_revision,comparison_encoding,comparison_hash) SELECT version_id,encoding,?1,snapshot_hash,actor,rationale,originals_compared,target_kind,target_id,target_revision,comparison_encoding,comparison_hash FROM definition_approvals WHERE id=?2", rusqlite::params![serde_json::to_string(&oversized).unwrap(),version.approval_id])) { accepted.push(field); }
        db.execute_batch("ROLLBACK TO rejection; RELEASE rejection")
            .unwrap();
    }
    db.execute("INSERT INTO definition_versions(game_id,edition_id,definition_id,version,content,content_hash) SELECT game_id,edition_id,definition_id,2,content,content_hash FROM definition_versions WHERE id=?1", [version.id]).unwrap();
    let other_version = db.last_insert_rowid();
    if !provenance_rejected(db.execute("INSERT INTO definition_approvals(version_id,encoding,snapshot,snapshot_hash,actor,rationale,originals_compared,target_kind,target_id,target_revision,comparison_encoding,comparison_hash) SELECT version_id,encoding,snapshot,snapshot_hash,actor,rationale,originals_compared,'version',?1,2,comparison_encoding,comparison_hash FROM definition_approvals WHERE id=?2", rusqlite::params![other_version,version.approval_id])) { accepted.push("different existing version"); }
    // A different valid draft cannot supply provenance for this version.
    let mut body =
        serde_json::to_value(&store.definition_draft(draft).await.unwrap().definition).unwrap();
    body["definition_id"] = "unrelated".into();
    let other = store
        .save_definition_draft(serde_json::to_vec(&body).unwrap(), 0)
        .await
        .unwrap();
    if !provenance_rejected(db.execute("INSERT INTO definition_approvals(version_id,encoding,snapshot,snapshot_hash,actor,rationale,originals_compared,target_kind,target_id,target_revision,comparison_encoding,comparison_hash) SELECT version_id,encoding,snapshot,snapshot_hash,actor,rationale,originals_compared,'draft',?1,1,comparison_encoding,comparison_hash FROM definition_approvals WHERE id=?2", rusqlite::params![other.id,version.approval_id])) { accepted.push("unrelated draft"); }
    body["definition_id"] = store
        .definition_draft(draft)
        .await
        .unwrap()
        .definition
        .definition_id
        .into();
    body["title"] = "Same identity, different content".into();
    store
        .save_definition_draft(serde_json::to_vec(&body).unwrap(), 1)
        .await
        .unwrap();
    if !provenance_rejected(db.execute("INSERT INTO definition_approvals(version_id,encoding,snapshot,snapshot_hash,actor,rationale,originals_compared,target_kind,target_id,target_revision,comparison_encoding,comparison_hash) SELECT version_id,encoding,snapshot,snapshot_hash,actor,rationale,originals_compared,'draft',target_id,2,comparison_encoding,comparison_hash FROM definition_approvals WHERE id=?1", [version.approval_id])) { accepted.push("same identity with different content"); }
    assert!(
        accepted.is_empty(),
        "Invalid ordinary inserts accepted: {accepted:?}"
    );
}

#[tokio::test]
async fn retained_draft_same_revision_content_mismatch_is_an_integrity_error() {
    let _guard = LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (store, draft, _, _) = setup(dir.path()).await;
    let ticket = store.compare_definition(draft, false).await.unwrap();
    let version = store
        .approve_definition(draft, false, request(ticket.comparison_hash))
        .await
        .unwrap();
    let db = rusqlite::Connection::open(dir.path().join("publication.sqlite3")).unwrap();
    db.execute("UPDATE definition_drafts SET content=json_set(content,'$.title','Changed without revision') WHERE id=?1", [draft]).unwrap();
    assert!(
        matches!(store.definition_version(version.id).await, Err(DefinitionError::Invalid(report)) if report.diagnostics[0].code == "stored-approval-target-integrity")
    );
}

#[tokio::test]
async fn retained_approval_requires_its_draft_target_even_after_withdrawal() {
    let _guard = LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (store, draft, _, _) = setup(dir.path()).await;
    let ticket = store.compare_definition(draft, false).await.unwrap();
    let version = store
        .approve_definition(draft, false, request(ticket.comparison_hash))
        .await
        .unwrap();
    let db = rusqlite::Connection::open(dir.path().join("publication.sqlite3")).unwrap();
    db.execute("DELETE FROM definition_drafts WHERE id=?1", [draft])
        .unwrap();
    for withdrawn in [false, true] {
        if withdrawn {
            store
                .withdraw_definition(
                    version.id,
                    version.approval_id,
                    "fixture".into(),
                    "Synthetic withdrawal".into(),
                )
                .await
                .unwrap();
        }
        assert!(
            matches!(store.definition_version(version.id).await, Err(DefinitionError::Invalid(report)) if report.diagnostics[0].code == "stored-approval-target-integrity")
        );
    }
}

#[tokio::test]
async fn version_counter_overflow_has_no_partial_publication() {
    let _guard = LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (store, draft, _, _) = setup(dir.path()).await;
    let ticket = store.compare_definition(draft, false).await.unwrap();
    let version = store
        .approve_definition(draft, false, request(ticket.comparison_hash))
        .await
        .unwrap();
    let db = rusqlite::Connection::open(dir.path().join("publication.sqlite3")).unwrap();
    db.execute("INSERT INTO definition_versions(game_id,edition_id,definition_id,version,content,content_hash) SELECT game_id,edition_id,definition_id,?1,content,content_hash FROM definition_versions WHERE id=?2", rusqlite::params![i64::MAX,version.id]).unwrap();
    let mut body =
        serde_json::to_value(&store.definition_draft(draft).await.unwrap().definition).unwrap();
    body["title"] = "Different content requires next version".into();
    store
        .save_definition_draft(serde_json::to_vec(&body).unwrap(), 1)
        .await
        .unwrap();
    let ticket = store.compare_definition(draft, false).await.unwrap();
    let before = publication_counts(dir.path());
    assert!(
        matches!(store.approve_definition(draft, false, request(ticket.comparison_hash)).await, Err(DefinitionError::Invalid(report)) if report.diagnostics[0].code == "published-version-overflow")
    );
    assert_eq!(publication_counts(dir.path()), before);
}

#[tokio::test]
async fn concurrent_cli_approvals_have_one_winner_and_no_partial_version() {
    let _guard = LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (store, draft, _, _) = setup(dir.path()).await;
    let check = store.compare_definition(draft, false).await.unwrap();
    let mut threads = Vec::new();
    for _ in 0..2 {
        let path = dir.path().join("publication.sqlite3");
        let hash = check.comparison_hash.clone();
        threads.push(std::thread::spawn(move || {
            std::process::Command::new(env!("CARGO_BIN_EXE_mimik"))
                .arg("--database")
                .arg(path)
                .arg("approve-definition")
                .args([
                    "--id",
                    &draft.to_string(),
                    "--comparison-hash",
                    &hash,
                    "--actor",
                    "fixture",
                    "--rationale",
                    "Synthetic comparison",
                    "--originals-compared",
                ])
                .output()
                .unwrap()
        }));
    }
    assert_eq!(
        threads
            .into_iter()
            .map(|thread| usize::from(thread.join().unwrap().status.success()))
            .sum::<usize>(),
        1
    );
    let db = rusqlite::Connection::open(dir.path().join("publication.sqlite3")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM definition_versions", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM definition_approvals", [], |row| row
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn approval_assertion_partial_wiring_and_deduplicated_regions_are_explicit() {
    let _guard = LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (store, draft, _, _) = setup(dir.path()).await;
    let mut body =
        serde_json::to_value(&store.definition_draft(draft).await.unwrap().definition).unwrap();
    let mut extra = body["fields"][0].clone();
    extra["id"] = "second".into();
    body["fields"].as_array_mut().unwrap().push(extra);
    let mut unresolved = body["fields"][0].clone();
    unresolved["id"] = "unknown".into();
    unresolved["mapping"] = json!({"state":"unresolved","reason":"Known gap"});
    body["fields"].as_array_mut().unwrap().push(unresolved);
    body["groups"][0]["fields"] = json!(["name", "second", "unknown"]);
    store
        .save_definition_draft(serde_json::to_vec(&body).unwrap(), 1)
        .await
        .unwrap();
    let check = store.compare_definition(draft, false).await.unwrap();
    assert_eq!(check.original_region_urls.len(), 1);
    let mut unchecked = request(check.comparison_hash.clone());
    unchecked.originals_compared = false;
    assert!(
        store
            .approve_definition(draft, false, unchecked)
            .await
            .is_err()
    );
    let pubversion = store
        .approve_definition(draft, false, request(check.comparison_hash))
        .await
        .unwrap();
    assert_eq!(pubversion.unresolved_fields, 1);
    assert_eq!(pubversion.wiring_status, WiringStatus::Current);
    assert_eq!(pubversion.advisory_tasks, "unsupported");
    let db = rusqlite::Connection::open(dir.path().join("publication.sqlite3")).unwrap();
    db.execute_batch("DROP TRIGGER units_finalized_update;")
        .unwrap();
    db.execute(
        "UPDATE source_units SET raw_text=?1",
        ["界".repeat(100_000)],
    )
    .unwrap();
    let error = store.definition_version(pubversion.id).await.unwrap_err();
    assert!(
        matches!(error,DefinitionError::Invalid(report) if report.diagnostics[0].code=="unit-missing-or-invalid-metadata")
    );
}

#[tokio::test]
async fn retained_approval_provenance_identifies_each_actual_revision_and_version_target() {
    let _guard = LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (store, draft, _, _) = setup(dir.path()).await;
    let check = store.compare_definition(draft, false).await.unwrap();
    let first = store
        .approve_definition(draft, false, request(check.comparison_hash))
        .await
        .unwrap();
    let body =
        serde_json::to_vec(&store.definition_draft(draft).await.unwrap().definition).unwrap();
    store.save_definition_draft(body, 1).await.unwrap();
    let check = store.compare_definition(draft, false).await.unwrap();
    let second = store
        .approve_definition(draft, false, request(check.comparison_hash))
        .await
        .unwrap();
    assert_eq!(first.id, second.id);
    let check = store.compare_definition(first.id, true).await.unwrap();
    store
        .approve_definition(first.id, true, request(check.comparison_hash))
        .await
        .unwrap();
    let db = rusqlite::Connection::open(dir.path().join("publication.sqlite3")).unwrap();
    let mut query = db
        .prepare(
            "SELECT target_kind,target_id,target_revision FROM definition_approvals ORDER BY id",
        )
        .unwrap();
    let rows = query
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            ("draft".into(), draft, 1),
            ("draft".into(), draft, 2),
            ("version".into(), first.id, 1)
        ]
    );
}

#[tokio::test]
async fn corrupt_retained_snapshot_is_an_integrity_error_even_when_withdrawn() {
    let _guard = LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (store, draft, _, _) = setup(dir.path()).await;
    let check = store.compare_definition(draft, false).await.unwrap();
    let version = store
        .approve_definition(draft, false, request(check.comparison_hash))
        .await
        .unwrap();
    let db = rusqlite::Connection::open(dir.path().join("publication.sqlite3")).unwrap();
    db.execute_batch("DROP TRIGGER definition_approvals_update;")
        .unwrap();
    db.execute("UPDATE definition_approvals SET snapshot='{}'", [])
        .unwrap();
    assert!(store.definition_version(version.id).await.is_err());
    store
        .withdraw_definition(
            version.id,
            version.approval_id,
            "fixture".into(),
            "Synthetic withdrawal".into(),
        )
        .await
        .unwrap();
    assert!(store.definition_version(version.id).await.is_err());
}

#[tokio::test]
async fn oversized_candidate_is_rejected_at_sql_projection_not_admission() {
    let _guard = LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (store, draft, _, _) = setup(dir.path()).await;
    let check = store.compare_definition(draft, false).await.unwrap();
    let version = store
        .approve_definition(draft, false, request(check.comparison_hash))
        .await
        .unwrap();
    let db = rusqlite::Connection::open(dir.path().join("publication.sqlite3")).unwrap();
    db.execute_batch(
        "PRAGMA ignore_check_constraints=ON; DROP TRIGGER definition_versions_update;",
    )
    .unwrap();
    for (table, id, published) in [
        ("definition_drafts", draft, false),
        ("definition_versions", version.id, true),
    ] {
        db.execute(
            &format!("UPDATE {table} SET content=?1 WHERE id=?2"),
            rusqlite::params!["x".repeat(262_145), id],
        )
        .unwrap();
        let error = store.compare_definition(id, published).await.unwrap_err();
        assert!(
            matches!(error,DefinitionError::Invalid(report) if report.diagnostics[0].code=="stored-publication-content-byte-limit")
        );
    }
}

async fn setup(path: &std::path::Path) -> (Store, i64, i64, i64) {
    setup_regions(path, false).await
}

async fn setup_regions(path: &std::path::Path, extra_regions: bool) -> (Store, i64, i64, i64) {
    fixture::fixture_pair(path).unwrap();
    if extra_regions {
        extend_synthetic_regions(path);
    }
    let store = Store::open(&path.join("publication.sqlite3")).unwrap();
    store
        .register_ruleset(RegisterRuleset {
            game_id: "synthetic-game".into(),
            game_label: "Synthetic game".into(),
            edition_id: "first".into(),
            edition_label: "First edition".into(),
        })
        .await
        .unwrap();
    let source = store
        .import_source(&path.join("unknown-bundle"), &path.join("original.pdf"))
        .await
        .unwrap();
    let source = store
        .sources()
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id == source)
        .unwrap();
    let page = store.source_page(source.pages[0].id).await.unwrap();
    let mut definition: serde_json::Value =
        serde_json::from_str(include_str!("../schemas/definition-example.json")).unwrap();
    definition["sources"] =
        json!([{"source_id":source.id,"pdf_sha256":source.pdf_hash,"roles":["sheet"]}]);
    definition["fields"][0]["mapping"] = json!({"state":"mapped","regions":[{"source_id":source.id,"page_id":page.id,"unit_id":page.units[0].id}]});
    let draft = store
        .save_definition_draft(serde_json::to_vec(&definition).unwrap(), 0)
        .await
        .unwrap();
    (store, draft.id, page.id, page.units[0].id)
}

fn extend_synthetic_regions(root: &std::path::Path) {
    use lopdf::{Object, dictionary};
    use std::fs;
    let pdf = root.join("original.pdf");
    let mut document = lopdf::Document::load(&pdf).unwrap();
    let first = *document.get_pages().get(&1).unwrap();
    let parent = document
        .get_object(first)
        .unwrap()
        .as_dict()
        .unwrap()
        .get(b"Parent")
        .unwrap()
        .as_reference()
        .unwrap();
    let second = document.add_object(dictionary! {"Type"=>"Page", "Parent"=>parent, "MediaBox"=>vec![0.into(),0.into(),200.into(),300.into()]});
    let pages = document
        .get_object_mut(parent)
        .unwrap()
        .as_dict_mut()
        .unwrap();
    pages.set(
        "Kids",
        vec![Object::Reference(first), Object::Reference(second)],
    );
    pages.set("Count", 2);
    document.save(&pdf).unwrap();
    let bundle = root.join("unknown-bundle");
    let mut raw: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join("source.json")).unwrap()).unwrap();
    let mut other_unit = raw["children"][0]["children"][0].clone();
    other_unit["id"] = "/page/0/Text/2".into();
    raw["children"][0]["children"]
        .as_array_mut()
        .unwrap()
        .push(other_unit);
    let mut second_page = raw["children"][0].clone();
    second_page["id"] = "/page/1/Page/0".into();
    second_page["children"].as_array_mut().unwrap().truncate(1);
    second_page["children"][0]["id"] = "/page/1/Text/1".into();
    raw["children"].as_array_mut().unwrap().push(second_page);
    let raw = serde_json::to_vec(&raw).unwrap();
    fs::write(bundle.join("source.json"), &raw).unwrap();
    fs::copy(bundle.join("page-1.png"), bundle.join("page-2.png")).unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join("bundle.json")).unwrap()).unwrap();
    let mut other_unit = manifest["pages"][0]["units"][0].clone();
    other_unit["id"] = "/page/0/Text/2".into();
    manifest["pages"][0]["units"]
        .as_array_mut()
        .unwrap()
        .push(other_unit);
    let mut second_page = manifest["pages"][0].clone();
    second_page["pdf_page"] = 2.into();
    second_page["units"].as_array_mut().unwrap().truncate(1);
    second_page["units"][0]["id"] = "/page/1/Text/1".into();
    manifest["pages"].as_array_mut().unwrap().push(second_page);
    manifest["source_sha256"] = fixture::hash(&fs::read(pdf).unwrap()).into();
    manifest["extraction_sha256"] = fixture::hash(&raw).into();
    fs::write(
        bundle.join("bundle.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}

async fn review(store: &Store, page: i64, unit: i64, state: &str, correction: &str) {
    let current = store.source_page(page).await.unwrap();
    let revision = current
        .units
        .iter()
        .find(|row| row.id == unit)
        .unwrap()
        .revision;
    store
        .review_source_unit(
            page,
            unit,
            mimik::sources::UnitReview {
                revision,
                state: state.into(),
                correction: correction.into(),
                actor: "fixture".into(),
                rationale: "Synthetic lifecycle".into(),
            },
        )
        .await
        .unwrap();
}

fn publication_counts(root: &std::path::Path) -> (i64, i64, i64) {
    let db = rusqlite::Connection::open(root.join("publication.sqlite3")).unwrap();
    db.query_row("SELECT (SELECT count(*) FROM definition_versions),(SELECT count(*) FROM definition_approvals),(SELECT count(*) FROM definition_withdrawals)", [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap()
}

#[tokio::test]
async fn captured_page_changes_invalidate_but_uncaptured_pages_do_not() {
    let _guard = LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (store, draft, page, mapped) = setup_regions(dir.path(), true).await;
    let check = store.compare_definition(draft, false).await.unwrap();
    let version = store
        .approve_definition(draft, false, request(check.comparison_hash))
        .await
        .unwrap();
    let captured = store.source_page(page).await.unwrap();
    let unrelated = captured
        .units
        .iter()
        .find(|row| row.id != mapped)
        .unwrap()
        .id;
    let source = store.sources().await.unwrap().remove(0);
    let other = store
        .source_page(source.pages.iter().find(|row| row.id != page).unwrap().id)
        .await
        .unwrap();
    review(&store, other.id, other.units[0].id, "accepted", "").await;
    assert_eq!(
        store
            .definition_version(version.id)
            .await
            .unwrap()
            .wiring_status,
        WiringStatus::Current
    );
    review(&store, page, unrelated, "accepted", "").await;
    assert_eq!(
        store
            .definition_version(version.id)
            .await
            .unwrap()
            .wiring_status,
        WiringStatus::Stale
    );
}

#[tokio::test]
async fn restored_effective_content_never_revives_approval_and_stale_tickets_write_nothing() {
    let _guard = LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (store, draft, page, unit) = setup(dir.path()).await;
    review(&store, page, unit, "accepted", "").await;
    let check = store.compare_definition(draft, false).await.unwrap();
    let version = store
        .approve_definition(draft, false, request(check.comparison_hash))
        .await
        .unwrap();
    for published in [false, true] {
        let id = if published { version.id } else { draft };
        let ticket = store.compare_definition(id, published).await.unwrap();
        review(&store, page, unit, "corrected", "Synthetic changed content").await;
        let before = publication_counts(dir.path());
        assert!(matches!(
            store
                .approve_definition(id, published, request(ticket.comparison_hash))
                .await,
            Err(DefinitionError::Conflict)
        ));
        assert_eq!(publication_counts(dir.path()), before);
        review(&store, page, unit, "accepted", "").await;
        assert_eq!(
            store
                .definition_version(version.id)
                .await
                .unwrap()
                .wiring_status,
            WiringStatus::Stale
        );
        let ticket = store.compare_definition(id, published).await.unwrap();
        let current = store.source_page(page).await.unwrap();
        store
            .complete_source_page(
                page,
                mimik::sources::PageReview {
                    revision: current.revision,
                    printed_label: "1".into(),
                    actor: "fixture".into(),
                    rationale: "Synthetic complete".into(),
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            store
                .approve_definition(id, published, request(ticket.comparison_hash))
                .await,
            Err(DefinitionError::Conflict)
        ));
        assert_eq!(publication_counts(dir.path()), before);
    }
}
fn request(hash: String) -> ApprovalRequest {
    ApprovalRequest {
        comparison_hash: hash,
        actor: "synthetic operator".into(),
        rationale: "Synthetic wiring comparison, not real manual approval".into(),
        originals_compared: true,
    }
}

#[tokio::test]
async fn immutable_publication_stale_evidence_explicit_reapproval_and_withdrawal() {
    let _guard = LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (store, draft, page, unit) = setup(dir.path()).await;
    let compare = store.compare_definition(draft, false).await.unwrap();
    let json = serde_json::to_string(&compare).unwrap();
    assert!(!json.contains("Strength capacity"));
    assert!(!json.contains("raw_html\":"));
    let version = store
        .approve_definition(draft, false, request(compare.comparison_hash.clone()))
        .await
        .unwrap();
    assert_eq!(version.wiring_status, WiringStatus::Current);
    assert_eq!(version.version, 1);
    assert!(matches!(
        store
            .approve_definition(draft, false, request(compare.comparison_hash))
            .await,
        Err(DefinitionError::Conflict)
    ));
    store
        .review_source_unit(
            page,
            unit,
            mimik::sources::UnitReview {
                revision: 1,
                state: "corrected".into(),
                correction: "Private corrected rule text".into(),
                actor: "fixture".into(),
                rationale: "Synthetic".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .definition_version(version.id)
            .await
            .unwrap()
            .wiring_status,
        WiringStatus::Stale
    );
    let compare = store.compare_definition(version.id, true).await.unwrap();
    assert!(
        !serde_json::to_string(&compare)
            .unwrap()
            .contains("Private corrected rule text")
    );
    let renewed = store
        .approve_definition(version.id, true, request(compare.comparison_hash))
        .await
        .unwrap();
    assert_eq!(renewed.id, version.id);
    assert_eq!(renewed.wiring_status, WiringStatus::Current);
    assert_ne!(renewed.approval_id, version.approval_id);
    let stale_ticket = store.compare_definition(version.id, true).await.unwrap();
    store
        .withdraw_definition(
            version.id,
            renewed.approval_id,
            "fixture".into(),
            "Synthetic withdrawal".into(),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .definition_version(version.id)
            .await
            .unwrap()
            .wiring_status,
        WiringStatus::Withdrawn
    );
    assert!(matches!(
        store
            .approve_definition(version.id, true, request(stale_ticket.comparison_hash))
            .await,
        Err(DefinitionError::Conflict)
    ));
    assert!(
        store
            .withdraw_definition(
                version.id,
                renewed.approval_id,
                "fixture".into(),
                "Duplicate".into()
            )
            .await
            .is_err()
    );
    let fresh = store.compare_definition(version.id, true).await.unwrap();
    assert!(fresh.withdrawn);
    let restored = store
        .approve_definition(version.id, true, request(fresh.comparison_hash))
        .await
        .unwrap();
    assert_eq!(restored.id, version.id);
    assert_eq!(restored.wiring_status, WiringStatus::Current);
    assert_ne!(restored.approval_id, renewed.approval_id);
    assert_eq!(publication_counts(dir.path()), (1, 3, 1));
    assert_immutable(dir.path());
}

fn assert_immutable(root: &std::path::Path) {
    let db = rusqlite::Connection::open(root.join("publication.sqlite3")).unwrap();
    for sql in [
        "UPDATE definition_versions SET version=2",
        "DELETE FROM definition_approvals",
        "INSERT OR REPLACE INTO definition_versions SELECT * FROM definition_versions",
        "INSERT OR REPLACE INTO definition_approvals SELECT * FROM definition_approvals",
        "INSERT OR REPLACE INTO definition_withdrawals SELECT * FROM definition_withdrawals",
    ] {
        assert!(
            db.execute(sql, []).is_err(),
            "Immutable guard accepted {sql}"
        );
    }
}

#[tokio::test]
async fn stale_draft_revision_never_publishes_and_new_drafts_do_not_change_frozen_versions() {
    let _guard = LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (store, draft, _, _) = setup(dir.path()).await;
    let compare = store.compare_definition(draft, false).await.unwrap();
    let original = store.definition_draft(draft).await.unwrap();
    let mut changed = serde_json::to_value(&original.definition).unwrap();
    changed["title"] = "Changed proposal".into();
    store
        .save_definition_draft(serde_json::to_vec(&changed).unwrap(), 1)
        .await
        .unwrap();
    assert!(matches!(
        store
            .approve_definition(draft, false, request(compare.comparison_hash))
            .await,
        Err(DefinitionError::Conflict)
    ));
    let compare = store.compare_definition(draft, false).await.unwrap();
    let first = store
        .approve_definition(draft, false, request(compare.comparison_hash))
        .await
        .unwrap();
    changed["title"] = "Next proposal".into();
    store
        .save_definition_draft(serde_json::to_vec(&changed).unwrap(), 2)
        .await
        .unwrap();
    assert_eq!(
        store
            .definition_version(first.id)
            .await
            .unwrap()
            .wiring_status,
        WiringStatus::Current
    );
    let compare = store.compare_definition(draft, false).await.unwrap();
    let second = store
        .approve_definition(draft, false, request(compare.comparison_hash))
        .await
        .unwrap();
    assert_eq!(second.version, 2);
    assert_ne!(second.id, first.id);
    assert_eq!(store.definition_version(first.id).await.unwrap().version, 1);
    assert!(
        !store
            .rulesets()
            .await
            .unwrap()
            .into_iter()
            .any(|ruleset| ruleset.generic_sheet_available)
    );
}

#[tokio::test]
async fn cli_comparison_is_redacted_and_never_automatically_approves() {
    let _guard = LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (store, draft, _, _) = setup(dir.path()).await;
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mimik"))
        .arg("--database")
        .arg(dir.path().join("publication.sqlite3"))
        .arg("compare-definition")
        .args(["--id", &draft.to_string()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let data: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        data["comparison_hash"]
            .as_str()
            .is_some_and(|hash| hash.len() == 64)
    );
    assert_eq!(data["current_approval_id"], 0);
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Strength capacity"));
    assert!(
        !store
            .rulesets()
            .await
            .unwrap()
            .into_iter()
            .any(|ruleset| ruleset.generic_sheet_available)
    );
}

#[tokio::test]
async fn schema9_retrofit_schema8_provenance_absent_preserves_history_and_rejects_inserts() {
    schema9_retrofit_publication(false).await;
}

#[tokio::test]
async fn schema9_retrofit_schema8_provenance_present_preserves_history_and_rejects_inserts() {
    schema9_retrofit_publication(true).await;
}

async fn schema9_retrofit_publication(provenance_present: bool) {
    let _guard = LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (store, draft, page, unit) = setup(dir.path()).await;
    review(&store, page, unit, "accepted", "").await;
    let ticket = store.compare_definition(draft, false).await.unwrap();
    let version = store
        .approve_definition(draft, false, request(ticket.comparison_hash))
        .await
        .unwrap();
    let before_page = store.source_page(page).await.unwrap();
    let path = dir.path().join("publication.sqlite3");
    let db = rusqlite::Connection::open(&path).unwrap();
    schema_fixture::remove_empty_account_tables(&db);
    db.execute_batch("DROP TRIGGER IF EXISTS page_completion_integrity; DROP TRIGGER IF EXISTS page_completion_event_integrity; DROP TRIGGER IF EXISTS definition_approvals_provenance; PRAGMA user_version=8;").unwrap();
    if provenance_present {
        // An earlier installation may have the named trigger with incomplete checks.
        db.execute_batch("CREATE TRIGGER definition_approvals_provenance BEFORE INSERT ON definition_approvals WHEN NEW.target_kind='version' AND NOT EXISTS(SELECT 1 FROM definition_versions WHERE id=NEW.target_id) BEGIN SELECT RAISE(ABORT,'Historical provenance'); END;").unwrap();
    }
    let retained = |db: &rusqlite::Connection| -> Vec<String> {
        ["definition_versions", "definition_approvals", "source_review_events", "source_pages", "source_units"].into_iter().map(|table| {
            // Compare every historical column across the upgrade; schema 24 adds
            // defaulted origin/operation columns, checked separately below.
            let columns: Vec<String> = db.prepare(&format!("PRAGMA table_info({table})")).unwrap().query_map([], |r| r.get(1)).unwrap().collect::<Result<Vec<_>, _>>().unwrap().into_iter().filter(|column: &String| !matches!(column.as_str(), "origin" | "operation_id")).collect();
            let values = columns.iter().map(|column| format!("CASE WHEN typeof({column})='blob' THEN hex({column}) ELSE {column} END")).collect::<Vec<_>>().join(",");
            db.query_row(&format!("SELECT json_group_array(json_array({values})) FROM (SELECT * FROM {table} ORDER BY id)"), [], |r| r.get(0)).unwrap()
        }).collect()
    };
    let before = retained(&db);
    let invalid = "INSERT INTO definition_approvals(version_id,encoding,snapshot,snapshot_hash,actor,rationale,originals_compared,target_kind,target_id,target_revision,comparison_encoding,comparison_hash) SELECT version_id,encoding,json_set(snapshot,'$.encoding',2),snapshot_hash,actor,rationale,originals_compared,target_kind,target_id,target_revision,comparison_encoding,comparison_hash FROM definition_approvals WHERE id=?1";
    db.execute_batch("SAVEPOINT historical").unwrap();
    assert_eq!(db.execute(invalid, [version.approval_id]).unwrap(), 1);
    db.execute_batch("ROLLBACK TO historical; RELEASE historical")
        .unwrap();
    drop(store);
    for _ in 0..2 {
        let reopened = Store::open(&path).unwrap();
        assert!(
            provenance_rejected(db.execute(invalid, [version.approval_id])),
            "schema8 startup accepted invalid approval (old provenance present={provenance_present})"
        );
        assert_eq!(retained(&db), before);
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM source_units WHERE origin<>'extracted'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM source_review_events WHERE operation_id IS NOT NULL",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        let current_page = reopened.source_page(page).await.unwrap();
        assert_eq!(current_page.revision, before_page.revision);
        assert_eq!(current_page.events.len(), before_page.events.len());
        let current = reopened.definition_version(version.id).await.unwrap();
        assert_eq!(current.id, version.id);
        assert_eq!(current.version, version.version);
        assert_eq!(current.approval_id, version.approval_id);
        assert_eq!(current.wiring_status, WiringStatus::Current);
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            mimik::SCHEMA_VERSION
        );
    }
}
