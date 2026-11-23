use mimik::{Store, registry::RegisterRuleset};
use serde_json::json;
use std::process::Command;

#[path = "support/schema_fixture.rs"]
mod schema_fixture;
#[path = "support/source_fixture.rs"]
mod source_fixture;

fn draft() -> serde_json::Value {
    serde_json::from_str(include_str!("../schemas/definition-example.json")).unwrap()
}

static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn store(path: &std::path::Path) -> Store {
    let store = Store::open(path).unwrap();
    store
        .register_ruleset(RegisterRuleset {
            game_id: "synthetic-game".into(),
            game_label: "Synthetic game".into(),
            edition_id: "first".into(),
            edition_label: "First edition".into(),
        })
        .await
        .unwrap();
    store
}

#[tokio::test]
async fn cli_imports_strict_definition_as_nonactivating_revision_one_draft() {
    let _guard = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("draft.sqlite3");
    store(&database).await;
    let file = dir.path().join("definition.json");
    std::fs::write(&file, serde_json::to_vec(&draft()).unwrap()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mimik"))
        .arg("--database")
        .arg(&database)
        .arg("import-definition-draft")
        .arg("--file")
        .arg(&file)
        .args(["--expected-revision", "0"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["status"], "draft");
    assert_eq!(result["revision"], 1);
}

#[tokio::test]
async fn strict_decoder_and_schema_reject_malformed_or_unsupported_proposals_without_rows() {
    let _guard = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir.path().join("reject.sqlite3")).await;
    let compact = serde_json::to_string(&draft()).unwrap();
    for raw in [
        compact.replace(
            "\"contract_version\":1",
            "\"contract_version\":1,\"contract_version\":1",
        ),
        compact.replace(
            "\"contract_version\":1",
            "\"contract_version\":1,\"contract\\u005fversion\":1",
        ),
        compact.replace("\"contract_version\":1", "\"contract_version\":1.0"),
        compact.replace("\"contract_version\":1", "\"contract_version\":1e0"),
        compact.clone() + "{}",
        "[".repeat(13) + "0" + &"]".repeat(13),
        " ".repeat(mimik::definitions::MAX_BYTES + 1),
    ] {
        assert!(matches!(
            store.save_definition_draft(raw.into_bytes(), 0).await,
            Err(mimik::definitions::DefinitionError::Invalid(_))
        ));
    }
    for (property, value) in [
        ("$ref", json!("https://outside.invalid/schema")),
        ("approved", json!(true)),
        ("formula", json!("1+2")),
        ("contract_version", json!(2)),
    ] {
        let mut value_object = draft();
        value_object[property] = value;
        assert!(
            store
                .save_definition_draft(serde_json::to_vec(&value_object).unwrap(), 0)
                .await
                .is_err()
        );
    }
    for control in ["script", "decimal", "list", "nested-group"] {
        let mut value = draft();
        value["fields"][0]["control"] = control.into();
        assert!(
            store
                .save_definition_draft(serde_json::to_vec(&value).unwrap(), 0)
                .await
                .is_err()
        );
    }
    let mut value = draft();
    value["groups"][0]["fields"] = json!(["missing"]);
    assert!(
        store
            .save_definition_draft(serde_json::to_vec(&value).unwrap(), 0)
            .await
            .is_err()
    );
    value = draft();
    let duplicate = value["fields"][0].clone();
    value["fields"].as_array_mut().unwrap().push(duplicate);
    assert!(
        store
            .save_definition_draft(serde_json::to_vec(&value).unwrap(), 0)
            .await
            .is_err()
    );
    assert!(
        store
            .definition_drafts("synthetic-game".into(), "first".into(), 0)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn revisions_full_content_identity_and_hash_persist_without_activating_capabilities() {
    let _guard = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("persist.sqlite3");
    let store = store(&path).await;
    let bytes = serde_json::to_vec(&draft()).unwrap();
    let first = store.save_definition_draft(bytes.clone(), 0).await.unwrap();
    assert_eq!(first.revision, 1);
    assert_eq!(first.status, "draft");
    assert_eq!(first.content_hash.len(), 64);
    assert!(matches!(
        store.save_definition_draft(bytes.clone(), 0).await,
        Err(mimik::definitions::DefinitionError::Conflict)
    ));
    let second = store.save_definition_draft(bytes.clone(), 1).await.unwrap();
    assert_eq!(second.id, first.id);
    assert_eq!(second.revision, 2);
    assert_eq!(second.content_hash, first.content_hash);
    assert!(matches!(
        store.save_definition_draft(bytes, 1).await,
        Err(mimik::definitions::DefinitionError::Conflict)
    ));
    assert_eq!(store.definition_draft(first.id).await.unwrap().revision, 2);
    store
        .register_ruleset(RegisterRuleset {
            game_id: "synthetic-game".into(),
            game_label: "Synthetic game".into(),
            edition_id: "second".into(),
            edition_label: "Second edition".into(),
        })
        .await
        .unwrap();
    let mut other = draft();
    other["edition_id"] = "second".into();
    let other = store
        .save_definition_draft(serde_json::to_vec(&other).unwrap(), 0)
        .await
        .unwrap();
    assert_ne!(other.id, first.id);
    assert_eq!(other.revision, 1);
    assert_eq!(
        store
            .definition_drafts("synthetic-game".into(), "first".into(), first.id)
            .await
            .unwrap()
            .len(),
        0
    );
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute(
        "UPDATE definition_drafts SET revision=?1 WHERE id=?2",
        rusqlite::params![i64::MAX, first.id],
    )
    .unwrap();
    assert!(
        store
            .save_definition_draft(serde_json::to_vec(&draft()).unwrap(), i64::MAX)
            .await
            .is_err()
    );
    assert_eq!(
        store.definition_draft(first.id).await.unwrap().revision,
        i64::MAX
    );
    drop(db);
    drop(store);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(
        reopened
            .definition_draft(other.id)
            .await
            .unwrap()
            .content_hash,
        other.content_hash
    );
    assert!(
        reopened
            .rulesets()
            .await
            .unwrap()
            .iter()
            .all(|entry| !entry.generic_sheet_available)
    );
}

#[tokio::test]
async fn enum_group_options_and_resource_bounds_are_strict() {
    let _guard = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir.path().join("bounds.sqlite3")).await;
    for (key, value) in [
        ("options", json!([{"id":"first","label":"First"}])),
        ("label", json!("界".repeat(101))),
        ("help", json!("x".repeat(2001))),
        ("id", json!("Upper")),
    ] {
        let mut value_object = draft();
        value_object["fields"][0][key] = value;
        assert!(
            store
                .save_definition_draft(serde_json::to_vec(&value_object).unwrap(), 0)
                .await
                .is_err()
        );
    }
    for options in [
        serde_json::Value::Null,
        json!([]),
        json!([{"id":"one","label":"One"},{"id":"one","label":"Two"}]),
    ] {
        let mut value = draft();
        value["fields"][0]["control"] = "enum".into();
        value["fields"][0]["options"] = options;
        assert!(
            store
                .save_definition_draft(serde_json::to_vec(&value).unwrap(), 0)
                .await
                .is_err()
        );
    }
    let mut value = draft();
    value["fields"][0]["control"] = "enum".into();
    value["fields"][0]["options"] = json!([{"id":"one","label":"One"},{"id":"two","label":"Two"}]);
    let saved = store
        .save_definition_draft(serde_json::to_vec(&value).unwrap(), 0)
        .await
        .unwrap();
    assert_eq!(
        saved.definition.fields[0].options.as_ref().unwrap()[1].id,
        "two"
    );
    for (array, count) in [("fields", 257), ("groups", 33), ("sources", 33)] {
        let mut value = draft();
        value[array] = serde_json::Value::Array(vec![json!({}); count]);
        assert!(
            store
                .save_definition_draft(serde_json::to_vec(&value).unwrap(), 1)
                .await
                .is_err()
        );
    }
    assert_eq!(store.definition_draft(saved.id).await.unwrap().revision, 1);
}

#[tokio::test]
async fn bounded_reports_are_safe_and_write_failure_leaves_prior_revision_unchanged() {
    let _guard = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("atomic.sqlite3");
    let store = store(&path).await;
    let first = store
        .save_definition_draft(serde_json::to_vec(&draft()).unwrap(), 0)
        .await
        .unwrap();
    let mut value = draft();
    value["fields"] = serde_json::Value::Array(vec![json!({}); 20]);
    let error = store
        .save_definition_draft(serde_json::to_vec(&value).unwrap(), 1)
        .await
        .unwrap_err();
    let mimik::definitions::DefinitionError::Invalid(report) = error else {
        panic!("Expected bounded diagnostics")
    };
    assert_eq!(report.diagnostics.len(), 32);
    assert!(report.truncated);
    assert!(
        report
            .to_string()
            .contains("additional diagnostics omitted")
    );
    for diagnostic in report.diagnostics {
        assert!(diagnostic.path.chars().count() <= 256);
        assert!(diagnostic.code.chars().count() <= 256);
        assert!(!diagnostic.path.chars().any(char::is_control));
    }
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER reject_draft BEFORE UPDATE ON definition_drafts BEGIN SELECT RAISE(ABORT,'Synthetic write failure'); END;").unwrap();
    let mut value = draft();
    value["title"] = "Unsaved title".into();
    assert!(matches!(
        store
            .save_definition_draft(serde_json::to_vec(&value).unwrap(), 1)
            .await,
        Err(mimik::definitions::DefinitionError::Storage(_))
    ));
    let after = store.definition_draft(first.id).await.unwrap();
    assert_eq!(after.revision, 1);
    assert_eq!(after.content_hash, first.content_hash);
    assert_eq!(after.definition.title, first.definition.title);
}

#[tokio::test]
async fn source_mapping_requires_exact_registered_pair_hash_and_joined_database_ids() {
    let _guard = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    source_fixture::fixture_pair(dir.path()).unwrap();
    let store = store(&dir.path().join("refs.sqlite3")).await;
    store
        .import_source(
            &dir.path().join("unknown-bundle"),
            &dir.path().join("original.pdf"),
        )
        .await
        .unwrap();
    let source = store.sources().await.unwrap().remove(0);
    let page = store.source_page(source.pages[0].id).await.unwrap();
    let mut value = draft();
    value["sources"] =
        json!([{"source_id":source.id,"pdf_sha256":source.pdf_hash,"roles":["sheet","core"]}]);
    value["fields"][0]["mapping"] = json!({"state":"mapped","regions":[{"source_id":source.id,"page_id":page.id,"unit_id":page.units[0].id}]});
    let saved = store
        .save_definition_draft(serde_json::to_vec(&value).unwrap(), 0)
        .await
        .unwrap();
    assert_eq!(
        saved.definition.sources[0].roles,
        [
            mimik::definitions::SourceRole::Core,
            mimik::definitions::SourceRole::Sheet
        ]
    );
    for (key, bad) in [("source_id", 999), ("page_id", 999), ("unit_id", 999)] {
        let mut changed = value.clone();
        changed["fields"][0]["mapping"]["regions"][0][key] = bad.into();
        assert!(
            store
                .save_definition_draft(serde_json::to_vec(&changed).unwrap(), 1)
                .await
                .is_err()
        );
    }
    for roles in [json!(["core"]), json!(["sheet", "sheet"]), json!(["other"])] {
        let mut changed = value.clone();
        changed["sources"][0]["roles"] = roles;
        assert!(
            store
                .save_definition_draft(serde_json::to_vec(&changed).unwrap(), 1)
                .await
                .is_err()
        );
    }
    let mut changed = value.clone();
    changed["sources"][0]["pdf_sha256"] = "0".repeat(64).into();
    assert!(
        store
            .save_definition_draft(serde_json::to_vec(&changed).unwrap(), 1)
            .await
            .is_err()
    );
    changed = value.clone();
    changed["game_id"] = "mork-borg".into();
    changed["edition_id"] = "it-2025".into();
    assert!(
        store
            .save_definition_draft(serde_json::to_vec(&changed).unwrap(), 0)
            .await
            .is_err()
    );
    store
        .register_ruleset(RegisterRuleset {
            game_id: "synthetic-game".into(),
            game_label: "Synthetic game".into(),
            edition_id: "second".into(),
            edition_label: "Second edition".into(),
        })
        .await
        .unwrap();
    changed = value.clone();
    changed["edition_id"] = "second".into();
    assert!(
        store
            .save_definition_draft(serde_json::to_vec(&changed).unwrap(), 0)
            .await
            .is_err()
    );
    assert_eq!(store.definition_draft(saved.id).await.unwrap().revision, 1);
    let after = store.source_page(page.id).await.unwrap();
    assert!(!after.complete);
    assert_eq!(after.units[0].state, "unreviewed");
    assert!(after.events.is_empty());
}

#[tokio::test]
async fn canonical_output_limit_is_enforced_before_storage_even_when_input_fits() {
    let _guard = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir.path().join("canonical.sqlite3")).await;
    let mut boundary = None;
    for length in 1700..=2000 {
        let mut value = draft();
        let mut fields = Vec::new();
        let mut ids = Vec::new();
        for index in 0..128 {
            let mut field = value["fields"][0].clone();
            field["id"] = format!("field-{index}").into();
            field["help"] = "x".repeat(length).into();
            ids.push(field["id"].clone());
            fields.push(field);
        }
        value["fields"] = fields.into();
        value["groups"][0]["fields"] = ids.into();
        let input = serde_json::to_vec(&value).unwrap();
        for field in value["fields"].as_array_mut().unwrap() {
            field["options"] = serde_json::Value::Null;
        }
        if input.len() <= mimik::definitions::MAX_BYTES
            && serde_json::to_vec(&value).unwrap().len() > mimik::definitions::MAX_BYTES
        {
            boundary = Some(input);
            break;
        }
    }
    let error = store
        .save_definition_draft(boundary.expect("Boundary vector"), 0)
        .await
        .unwrap_err();
    assert!(
        matches!(error,mimik::definitions::DefinitionError::Invalid(report) if report.diagnostics[0].code=="canonical-byte-limit")
    );
    assert!(
        store
            .definition_drafts("synthetic-game".into(), "first".into(), 0)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn strict_storage_rejects_wrong_types_hash_shapes_and_identity_rebinding() {
    let _guard = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("storage.sqlite3");
    let store = store(&path).await;
    let first = store
        .save_definition_draft(serde_json::to_vec(&draft()).unwrap(), 0)
        .await
        .unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    for sql in [
        "UPDATE definition_drafts SET revision=0",
        "UPDATE definition_drafts SET revision=1.5",
        "UPDATE definition_drafts SET id=0",
        "UPDATE definition_drafts SET definition_id='other'",
        "UPDATE definition_drafts SET content='[]'",
        "UPDATE definition_drafts SET content='{}'",
        "UPDATE definition_drafts SET content_hash='bad'",
        "UPDATE definition_drafts SET content_hash=upper(content_hash)",
    ] {
        assert!(
            db.execute(sql, []).is_err(),
            "Accepted invalid stored draft: {sql}"
        );
    }
    let after = store.definition_draft(first.id).await.unwrap();
    assert_eq!(after.revision, 1);
    assert_eq!(after.content_hash, first.content_hash);
    db.execute(
        "UPDATE definition_drafts SET content_hash=?1",
        ["0".repeat(64)],
    )
    .unwrap();
    assert!(store.definition_draft(first.id).await.is_err());
}

#[tokio::test]
async fn local_draft_files_reject_symlinks_and_oversize_and_cross_process_create_has_one_winner() {
    let _guard = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("process.sqlite3");
    let store = store(&database).await;
    let file = dir.path().join("draft.json");
    std::fs::write(&file, serde_json::to_vec(&draft()).unwrap()).unwrap();
    let link = dir.path().join("link.json");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    assert!(store.import_definition_draft(&link, 0).await.is_err());
    let oversized = dir.path().join("large.json");
    std::fs::write(&oversized, vec![b' '; mimik::definitions::MAX_BYTES + 1]).unwrap();
    assert!(store.import_definition_draft(&oversized, 0).await.is_err());
    let mut threads = Vec::new();
    for _ in 0..2 {
        let database = database.clone();
        let file = file.clone();
        threads.push(std::thread::spawn(move || {
            Command::new(env!("CARGO_BIN_EXE_mimik"))
                .arg("--database")
                .arg(database)
                .arg("import-definition-draft")
                .arg("--file")
                .arg(file)
                .args(["--expected-revision", "0"])
                .output()
                .unwrap()
        }));
    }
    let successes = threads
        .into_iter()
        .map(|thread| usize::from(thread.join().unwrap().status.success()))
        .sum::<usize>();
    assert_eq!(successes, 1);
    let summaries = store
        .definition_drafts("synthetic-game".into(), "first".into(), 0)
        .await
        .unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].revision, 1);
}

#[tokio::test]
async fn schema_six_migration_is_atomic_and_concurrent_updates_have_one_committed_winner() {
    let _guard = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("upgrade.sqlite3");
    let store = store(&path).await;
    let first = store
        .save_definition_draft(serde_json::to_vec(&draft()).unwrap(), 0)
        .await
        .unwrap();
    let file = dir.path().join("draft.json");
    std::fs::write(&file, serde_json::to_vec(&draft()).unwrap()).unwrap();
    let mut threads = Vec::new();
    for _ in 0..2 {
        let path = path.clone();
        let file = file.clone();
        threads.push(std::thread::spawn(move || {
            Command::new(env!("CARGO_BIN_EXE_mimik"))
                .arg("--database")
                .arg(path)
                .arg("import-definition-draft")
                .arg("--file")
                .arg(file)
                .args(["--expected-revision", "1"])
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
    assert_eq!(store.definition_draft(first.id).await.unwrap().revision, 2);
    drop(store);
    let db = rusqlite::Connection::open(&path).unwrap();
    schema_fixture::pre_finalization(&db, false);
    db.execute_batch("DROP TABLE definition_drafts; PRAGMA user_version=5;")
        .unwrap();
    db.execute_batch("CREATE TABLE definition_drafts(sentinel TEXT);")
        .unwrap();
    let before: i64 = db
        .query_row("SELECT count(*) FROM registry_editions", [], |row| {
            row.get(0)
        })
        .unwrap();
    drop(db);
    assert!(Store::open(&path).is_err());
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        5
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM registry_editions", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        before
    );
    db.execute("DROP TABLE definition_drafts", []).unwrap();
    drop(db);
    Store::open(&path).unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        mimik::SCHEMA_VERSION
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
}

fn assert_check_failure(result: rusqlite::Result<usize>) {
    let error = result.expect_err("SQLite CHECK must reject malformed stored identity/hash");
    assert!(
        matches!(error,rusqlite::Error::SqliteFailure(code,_) if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_CHECK)
    );
}

#[tokio::test]
async fn database_rejects_nul_suffixes_in_definition_ids() {
    let _guard = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nul.sqlite3");
    let store = store(&path).await;
    let first = store
        .save_definition_draft(serde_json::to_vec(&draft()).unwrap(), 0)
        .await
        .unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    let mut value = serde_json::to_value(&first.definition).unwrap();
    let id = "sheet\0INVALID";
    value["definition_id"] = id.into();
    assert_check_failure(db.execute("INSERT INTO definition_drafts(game_id,edition_id,definition_id,revision,content,content_hash) VALUES ('synthetic-game','first',?1,1,?2,?3)", rusqlite::params![id,serde_json::to_string(&value).unwrap(),"0".repeat(64)]));
    assert_eq!(
        store.definition_draft(first.id).await.unwrap().content_hash,
        first.content_hash
    );
}

#[tokio::test]
async fn database_rejects_nul_suffixes_after_exact_hash_bytes() {
    let _guard = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nul-hash.sqlite3");
    let store = store(&path).await;
    let first = store
        .save_definition_draft(serde_json::to_vec(&draft()).unwrap(), 0)
        .await
        .unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_check_failure(db.execute(
        "UPDATE definition_drafts SET content_hash=?1 WHERE id=?2",
        rusqlite::params!["0".repeat(64) + "\0INVALID", first.id],
    ));
    assert_eq!(
        store.definition_draft(first.id).await.unwrap().content_hash,
        first.content_hash
    );
}

#[tokio::test]
async fn database_rejects_nul_game_and_edition_slugs_even_with_valid_foreign_keys() {
    let _guard = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nul-pairs.sqlite3");
    let store = store(&path).await;
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    for (game, edition) in [
        ("synthetic-game\0INVALID", "first"),
        ("synthetic-game", "first\0INVALID"),
    ] {
        if game.contains('\0') {
            db.execute(
                "INSERT INTO registry_games VALUES (?1,'Invalid fixture')",
                [game],
            )
            .unwrap();
        }
        db.execute(
            "INSERT INTO registry_editions(game_id,edition_id,label) VALUES (?1,?2,'Invalid fixture')",
            rusqlite::params![game, edition],
        )
        .unwrap();
        let mut value = draft();
        value["game_id"] = game.into();
        value["edition_id"] = edition.into();
        assert_check_failure(db.execute("INSERT INTO definition_drafts(game_id,edition_id,definition_id,revision,content,content_hash) VALUES (?1,?2,'sheet',1,?3,?4)",rusqlite::params![game,edition,serde_json::to_string(&value).unwrap(),"0".repeat(64)]));
    }
    assert!(
        store
            .definition_drafts("synthetic-game".into(), "first".into(), 0)
            .await
            .unwrap()
            .is_empty()
    );
}
