use mimik::Store;
use mimik::{
    CharacterInput,
    registry::{RegisterAlias, RegisterRuleset},
};
use std::process::Command;

#[path = "support/source_fixture.rs"]
mod fixture;
#[path = "support/schema_fixture.rs"]
mod schema_fixture;

#[test]
fn cli_registers_unknown_ruleset_and_lists_it_without_claiming_a_sheet() {
    let dir = tempfile::tempdir().unwrap();
    fixture::fixture_pair(dir.path()).unwrap();
    let database = dir.path().join("registry.sqlite3");
    let output = Command::new(env!("CARGO_BIN_EXE_mimik"))
        .arg("--database")
        .arg(&database)
        .args([
            "register-ruleset",
            "--game-id",
            "synthetic-game",
            "--game-label",
            "Synthetic game",
            "--edition-id",
            "first",
            "--edition-label",
            "First edition",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = Command::new(env!("CARGO_BIN_EXE_mimik"))
        .arg("--database")
        .arg(&database)
        .arg("list-rulesets")
        .output()
        .unwrap();
    assert!(output.status.success());
    let entries: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let entry = entries
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["game_id"] == "synthetic-game")
        .unwrap();
    assert_eq!(entry["edition_id"], "first");
    assert_eq!(entry["generic_sheet_available"], false);
    Store::open(&database).unwrap();
}

fn registration(game: &str, edition: &str, label: &str) -> RegisterRuleset {
    RegisterRuleset {
        game_id: game.into(),
        game_label: if game == "mork-borg" {
            "MÖRK BORG".into()
        } else {
            "Synthetic game".into()
        },
        edition_id: edition.into(),
        edition_label: label.into(),
    }
}

/// The label the removed seed (migration 005) gave the Italian 2025 edition: a legacy record or
/// version-1 source carrying it is attached by the replayed migration when it is applied again.
const SEEDED_LABEL: &str = "MÖRK BORG — IT 2025";

fn legacy_character(edition: &str) -> CharacterInput {
    CharacterInput {
        name: "Legacy".into(),
        system_id: "mork-borg".into(),
        edition: edition.into(),
        notes: "Retained notes".into(),
        revision: 0,
    }
}

async fn accept_source_page(store: &Store, source_id: i64) {
    let source = store
        .sources()
        .await
        .unwrap()
        .into_iter()
        .find(|source| source.id == source_id)
        .unwrap();
    let page = store.source_page(source.pages[0].id).await.unwrap();
    store
        .review_source_unit(
            page.id,
            page.units[0].id,
            mimik::sources::UnitReview {
                revision: page.units[0].revision,
                state: "accepted".into(),
                correction: String::new(),
                actor: "fixture".into(),
                rationale: "Synthetic lookup proof".into(),
            },
        )
        .await
        .unwrap();
    let page = store.source_page(page.id).await.unwrap();
    store
        .complete_source_page(
            page.id,
            mimik::sources::PageReview {
                revision: page.revision,
                printed_label: "1".into(),
                actor: "fixture".into(),
                rationale: "Synthetic complete page".into(),
            },
        )
        .await
        .unwrap();
}

async fn lookup_html(store: &Store, query: &str) -> (axum::http::StatusCode, String) {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let response = mimik::app(store.clone())
        .oneshot(
            Request::builder()
                .uri(format!("/sources/search?{query}"))
                .header("host", "localhost:3000")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    (
        response.status(),
        String::from_utf8(
            to_bytes(response.into_body(), 200_000)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap(),
    )
}

#[tokio::test]
async fn lookup_aggregates_canonical_alias_sources_and_excludes_other_edition_coverage() {
    let dir = tempfile::tempdir().unwrap();
    fixture::fixture_pair(dir.path()).unwrap();
    let store = Store::open(&dir.path().join("lookup.sqlite3")).unwrap();
    store
        .register_ruleset(registration("synthetic-game", "first", "First edition"))
        .await
        .unwrap();
    store
        .register_ruleset(registration("synthetic-game", "second", "Second edition"))
        .await
        .unwrap();
    store
        .register_ruleset_alias(RegisterAlias {
            game_id: "synthetic-game".into(),
            edition_id: "first".into(),
            legacy_edition: "Alias edition".into(),
        })
        .await
        .unwrap();
    let bundle = dir.path().join("unknown-bundle");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(bundle.join("bundle.json")).unwrap()).unwrap();
    for (id, edition_id, label, title) in [
        ("canonical", "first", "First edition", "Canonical source"),
        ("alias", "first", "Alias edition", "Aliased source"),
        ("other", "second", "Second edition", "Other edition source"),
    ] {
        let mut changed = manifest.clone();
        changed["source_id"] = id.into();
        changed["edition_id"] = edition_id.into();
        changed["edition"] = label.into();
        changed["title"] = title.into();
        // These are separate manuals for edition/alias aggregation, not alternative
        // readings of the same PDF (which now share one primary per page).
        let pdf = dir.path().join("original.pdf");
        let mut document = lopdf::Document::load(&pdf).unwrap();
        let info = document.add_object(lopdf::dictionary! {"Title" => id});
        document.trailer.set("Info", info);
        document.save(&pdf).unwrap();
        changed["source_sha256"] = fixture::hash(&std::fs::read(&pdf).unwrap()).into();
        std::fs::write(
            bundle.join("bundle.json"),
            serde_json::to_vec(&changed).unwrap(),
        )
        .unwrap();
        let source = store
            .import_source(&bundle, &dir.path().join("original.pdf"))
            .await
            .unwrap();
        if id == "other" {
            let db = rusqlite::Connection::open(dir.path().join("lookup.sqlite3")).unwrap();
            schema_fixture::pre_finalization(&db, true);
            db.execute("UPDATE sources SET total_pages=0 WHERE id=?1", [source])
                .unwrap();
        } else {
            accept_source_page(&store, source).await;
        }
    }
    let result = store
        .search_evidence(
            "synthetic-game".into(),
            "First edition".into(),
            "capacity".into(),
        )
        .await
        .unwrap();
    assert_eq!(result.source_count, 2);
    assert_eq!(result.incomplete_pages, 0);
    assert_eq!(result.partial_runs, 0);
    assert_eq!(result.matches.len(), 2);
    assert!(
        result
            .matches
            .iter()
            .all(|entry| entry.title != "Other edition source")
    );
    let (status, html) = lookup_html(
        &store,
        "system=synthetic-game&edition_id=first&query=capacity",
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(html.contains("Strength capacity"));
    assert!(html.contains("Canonical source"));
    assert!(html.contains("Aliased source"));
    assert!(!html.contains("Other edition source"));
}

#[tokio::test]
async fn lookup_selectors_fail_closed_and_legacy_attachment_is_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let store = Store::open(&dir.path().join("selectors.sqlite3")).unwrap();
    let id = store.import_source(&bundle, &pdf).await.unwrap();
    accept_source_page(&store, id).await;
    assert_eq!(
        store
            .search_evidence(
                "mork-borg".into(),
                "fixture-edition".into(),
                "capacity".into()
            )
            .await
            .unwrap()
            .matches
            .len(),
        1
    );
    store
        .register_ruleset(registration("mork-borg", "fixture", "fixture-edition"))
        .await
        .unwrap();
    assert!(store.sources().await.unwrap()[0].ruleset.is_none());
    assert_eq!(
        store
            .search_ruleset_evidence("mork-borg".into(), "fixture".into(), "capacity".into())
            .await
            .unwrap()
            .source_count,
        0
    );
    store
        .register_ruleset_alias(RegisterAlias {
            game_id: "mork-borg".into(),
            edition_id: "fixture".into(),
            legacy_edition: "fixture-edition".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        store
            .search_ruleset_evidence("mork-borg".into(), "fixture".into(), "capacity".into())
            .await
            .unwrap()
            .matches
            .len(),
        1
    );
    // Former LIKE wildcards carry no searchable word and are rejected, never match-all.
    for text in ["%", "_", "\\"] {
        assert!(matches!(
            store
                .search_ruleset_evidence("mork-borg".into(), "fixture".into(), text.into())
                .await,
            Err(mimik::AppError::Invalid(_))
        ));
    }
    for query in [
        "system=mork-borg&query=capacity",
        "system=mork-borg&edition=fixture-edition&edition_id=fixture&query=capacity",
        "system=mork-borg&edition_id=First+edition&query=capacity",
        "system=mork-borg&edition_id=+&query=capacity",
    ] {
        assert_eq!(
            lookup_html(&store, query).await.0,
            axum::http::StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let (status, html) = lookup_html(
        &store,
        "system=mork-borg&edition_id=unregistered&query=capacity",
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(html.contains("Unsupported source/edition"));
}

#[tokio::test]
async fn alias_backfill_failure_rolls_back_alias_and_prior_character_attachment() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("rollback.sqlite3");
    let store = Store::open(&path).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    let character = store
        .create(legacy_character("fixture-edition"))
        .await
        .unwrap();
    let page_id = store.sources().await.unwrap()[0].pages[0].id;
    let before = store.source_page(page_id).await.unwrap();
    store
        .register_ruleset(registration("mork-borg", "fixture", "Fixture edition"))
        .await
        .unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER reject_backfill BEFORE INSERT ON source_rulesets BEGIN SELECT RAISE(ABORT,'Synthetic backfill failure'); END;").unwrap();
    assert!(
        store
            .register_ruleset_alias(RegisterAlias {
                game_id: "mork-borg".into(),
                edition_id: "fixture".into(),
                legacy_edition: "fixture-edition".into()
            })
            .await
            .is_err()
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM legacy_ruleset_aliases WHERE edition='fixture-edition'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert!(store.get(character.id).await.unwrap().ruleset.is_none());
    assert!(store.sources().await.unwrap()[0].ruleset.is_none());
    assert_eq!(
        store.get(character.id).await.unwrap().input.revision,
        character.input.revision
    );
    assert_eq!(
        store.get(character.id).await.unwrap().input.notes,
        character.input.notes
    );
    assert_eq!(
        store.source_page(page_id).await.unwrap().revision,
        before.revision
    );
    assert_eq!(
        store.source_page(page_id).await.unwrap().events.len(),
        before.events.len()
    );
    db.execute_batch("DROP TRIGGER reject_backfill").unwrap();
    store
        .register_ruleset_alias(RegisterAlias {
            game_id: "mork-borg".into(),
            edition_id: "fixture".into(),
            legacy_edition: "fixture-edition".into(),
        })
        .await
        .unwrap();
    assert!(store.get(character.id).await.unwrap().ruleset.is_some());
}

#[tokio::test]
async fn fresh_source_and_alias_constraints_reject_partial_or_unknown_target_pairs() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("constraints.sqlite3");
    let store = Store::open(&path).unwrap();
    let source_id = store.import_source(&bundle, &pdf).await.unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    for (game, edition) in [
        (None, Some("it-2025")),
        (Some("mork-borg"), None),
        (Some("mork-borg"), Some("missing")),
    ] {
        assert!(
            db.execute(
                "INSERT INTO source_rulesets VALUES (?1,?2,?3)",
                rusqlite::params![source_id, game, edition]
            )
            .is_err()
        );
        assert!(
            db.execute(
                "INSERT INTO legacy_ruleset_aliases VALUES ('mork-borg','Invalid alias',?1,?2)",
                rusqlite::params![game, edition]
            )
            .is_err()
        );
    }
    assert!(store.sources().await.unwrap()[0].ruleset.is_none());
    assert_eq!(
        db.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn registry_is_exact_idempotent_and_cannot_shadow_alias_labels() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("registry.sqlite3")).unwrap();
    store
        .register_ruleset(registration("synthetic-game", "first", "First edition"))
        .await
        .unwrap();
    store
        .register_ruleset(registration("synthetic-game", "first", "First edition"))
        .await
        .unwrap();
    let before = serde_json::to_value(store.rulesets().await.unwrap()).unwrap();
    for game in ["", "UPPER", "../escape", "game_name", &"a".repeat(65)] {
        assert!(
            store
                .register_ruleset(registration(game, "first", "First edition"))
                .await
                .is_err()
        );
    }
    for input in [
        registration("synthetic-game", "first", "Changed"),
        registration("synthetic-game", "second", "First edition"),
    ] {
        assert!(store.register_ruleset(input).await.is_err());
    }
    assert_eq!(
        serde_json::to_value(store.rulesets().await.unwrap()).unwrap(),
        before
    );
    store
        .register_ruleset_alias(RegisterAlias {
            game_id: "synthetic-game".into(),
            edition_id: "first".into(),
            legacy_edition: "Alias label".into(),
        })
        .await
        .unwrap();
    assert!(
        store
            .register_ruleset(registration("synthetic-game", "second", "Alias label"))
            .await
            .is_err()
    );
    store
        .register_ruleset(registration("synthetic-game", "second", "Second edition"))
        .await
        .unwrap();
    for (game, target, old) in [
        ("synthetic-game", "first", "Second edition"),
        ("synthetic-game", "second", "Alias label"),
    ] {
        assert!(
            store
                .register_ruleset_alias(RegisterAlias {
                    game_id: game.into(),
                    edition_id: target.into(),
                    legacy_edition: old.into()
                })
                .await
                .is_err()
        );
    }
    assert_eq!(store.rulesets().await.unwrap().len(), 2);
}

#[tokio::test]
async fn unknown_v2_sources_require_exact_registered_ids_and_keep_unreviewed_state() {
    let dir = tempfile::tempdir().unwrap();
    fixture::fixture_pair(dir.path()).unwrap();
    let store = Store::open(&dir.path().join("registry.sqlite3")).unwrap();
    let bundle = dir.path().join("unknown-bundle");
    let pdf = dir.path().join("original.pdf");
    assert!(store.import_source(&bundle, &pdf).await.is_err());
    assert!(store.sources().await.unwrap().is_empty());
    store
        .register_ruleset(registration("synthetic-game", "first", "First edition"))
        .await
        .unwrap();
    let id = store.import_source(&bundle, &pdf).await.unwrap();
    assert_eq!(store.import_source(&bundle, &pdf).await.unwrap(), id);
    let source = store.sources().await.unwrap().remove(0);
    assert_eq!(source.ruleset.unwrap().edition_id, "first");
    let page = store.source_page(source.pages[0].id).await.unwrap();
    assert!(!page.complete);
    assert_eq!(page.units[0].state, "unreviewed");
    assert!(page.events.is_empty());
    let original: serde_json::Value =
        serde_json::from_slice(&std::fs::read(bundle.join("bundle.json")).unwrap()).unwrap();
    for (property, value) in [
        ("edition_id", serde_json::json!("other")),
        ("edition_id", serde_json::Value::Null),
        ("edition_id", serde_json::json!(42)),
        ("edition", serde_json::json!("Different edition")),
        ("system_id", serde_json::json!("other-game")),
        ("contract_version", serde_json::json!(3)),
        ("contract_version", serde_json::json!(1)),
    ] {
        let mut changed = original.clone();
        changed[property] = value;
        std::fs::write(
            bundle.join("bundle.json"),
            serde_json::to_vec(&changed).unwrap(),
        )
        .unwrap();
        assert!(store.import_source(&bundle, &pdf).await.is_err());
        assert_eq!(store.sources().await.unwrap().len(), 1);
        assert_eq!(
            store.source_page(page.id).await.unwrap().revision,
            page.revision
        );
    }
    let mut missing = original;
    missing.as_object_mut().unwrap().remove("edition_id");
    std::fs::write(
        bundle.join("bundle.json"),
        serde_json::to_vec(&missing).unwrap(),
    )
    .unwrap();
    assert!(store.import_source(&bundle, &pdf).await.is_err());
}

#[tokio::test]
async fn exact_alias_backfill_preserves_values_reviews_and_duplicate_import_identity() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("registry.sqlite3");
    let store = Store::open(&path).unwrap();
    let id = store.import_source(&bundle, &pdf).await.unwrap();
    let character = store
        .create(legacy_character("fixture-edition"))
        .await
        .unwrap();
    assert!(character.ruleset.is_none());
    let source = store.sources().await.unwrap().remove(0);
    assert!(source.ruleset.is_none());
    let page = store.source_page(source.pages[0].id).await.unwrap();
    store
        .review_source_unit(
            page.id,
            page.units[0].id,
            mimik::sources::UnitReview {
                revision: 1,
                state: "corrected".into(),
                correction: "Preserved correction".into(),
                actor: "fixture".into(),
                rationale: "Synthetic review".into(),
            },
        )
        .await
        .unwrap();
    let before = store.source_page(page.id).await.unwrap();
    store
        .register_ruleset(registration("mork-borg", "fixture", "Fixture edition"))
        .await
        .unwrap();
    for _ in 0..2 {
        store
            .register_ruleset_alias(RegisterAlias {
                game_id: "mork-borg".into(),
                edition_id: "fixture".into(),
                legacy_edition: "fixture-edition".into(),
            })
            .await
            .unwrap();
    }
    let attached = store.get(character.id).await.unwrap();
    assert_eq!(attached.ruleset.unwrap().edition_id, "fixture");
    assert_eq!(attached.input.revision, character.input.revision);
    assert_eq!(attached.input.notes, character.input.notes);
    assert_eq!(store.import_source(&bundle, &pdf).await.unwrap(), id);
    let after = store.source_page(page.id).await.unwrap();
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.units[0].correction, before.units[0].correction);
    assert_eq!(after.events.len(), before.events.len());
    let db = rusqlite::Connection::open(&path).unwrap();
    store
        .register_ruleset(registration("mork-borg", "it-2025", SEEDED_LABEL))
        .await
        .unwrap();
    schema_fixture::pre_finalization(&db, true);
    db.execute("DELETE FROM source_rulesets WHERE source_id=?1", [id])
        .unwrap();
    db.execute(
        "INSERT INTO source_rulesets VALUES (?1,'mork-borg','it-2025')",
        [id],
    )
    .unwrap();
    db.execute("UPDATE sources SET total_pages=0 WHERE id=?1", [id])
        .unwrap();
    assert!(store.import_source(&bundle, &pdf).await.is_err());
    assert_eq!(store.sources().await.unwrap()[0].total_pages, 0);
    assert_eq!(
        store.source_page(page.id).await.unwrap().units[0].correction,
        "Preserved correction"
    );
    drop(db);
    drop(store);
    assert_eq!(
        Store::open(&path)
            .unwrap()
            .get(character.id)
            .await
            .unwrap()
            .input
            .notes,
        "Retained notes"
    );
}

#[tokio::test]
async fn populated_v4_replay_preserves_the_legacy_record_and_source_review_artifacts() {
    let dir = tempfile::tempdir().unwrap();
    fixture::fixture_pair(dir.path()).unwrap();
    let path = dir.path().join("migration.sqlite3");
    let store = Store::open(&path).unwrap();
    let character = store.create(legacy_character(SEEDED_LABEL)).await.unwrap();
    // A version-1 source carrying the label the replayed seed resolves.
    let bundle = dir.path().join("bundle");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(bundle.join("bundle.json")).unwrap()).unwrap();
    manifest["edition"] = SEEDED_LABEL.into();
    std::fs::write(
        bundle.join("bundle.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    store
        .import_source(&bundle, &dir.path().join("original.pdf"))
        .await
        .unwrap();
    let source = store.sources().await.unwrap().remove(0);
    let page = store.source_page(source.pages[0].id).await.unwrap();
    store
        .review_source_unit(
            page.id,
            page.units[0].id,
            mimik::sources::UnitReview {
                revision: 1,
                state: "corrected".into(),
                correction: "Preserved migration correction".into(),
                actor: "fixture".into(),
                rationale: "Synthetic review".into(),
            },
        )
        .await
        .unwrap();
    store
        .complete_source_page(
            page.id,
            mimik::sources::PageReview {
                revision: 2,
                printed_label: "1".into(),
                actor: "fixture".into(),
                rationale: "Synthetic complete page".into(),
            },
        )
        .await
        .unwrap();
    let before = store.source_page(page.id).await.unwrap();
    let before_character = store.get(character.id).await.unwrap();
    let image = store.source_image(page.id).await.unwrap();
    drop(store);
    let db = rusqlite::Connection::open(&path).unwrap();
    schema_fixture::pre_finalization(&db, false);
    db.execute_batch("PRAGMA foreign_keys=ON; DROP TABLE definition_drafts; DROP TABLE character_rulesets; DROP TABLE source_rulesets; DROP TABLE legacy_ruleset_aliases; DROP TABLE registry_editions; DROP TABLE registry_games; PRAGMA user_version=4;").unwrap();
    drop(db);
    let migrated = Store::open(&path).unwrap();
    // The replayed seed (005) attaches the legacy record and the source; the database is
    // already at 038's shape, so the replay leaves the pair alone.
    let after_character = migrated.get(character.id).await.unwrap();
    assert_eq!(
        after_character.input.revision,
        before_character.input.revision
    );
    assert_eq!(after_character.input.notes, before_character.input.notes);
    assert_eq!(after_character.ruleset.unwrap().edition_id, "it-2025");
    let after = migrated.source_page(page.id).await.unwrap();
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.complete, before.complete);
    assert_eq!(after.printed_label, before.printed_label);
    assert_eq!(after.units[0].correction, before.units[0].correction);
    assert_eq!(after.events.len(), before.events.len());
    assert_eq!(migrated.source_image(page.id).await.unwrap(), image);
    assert_eq!(
        migrated.sources().await.unwrap()[0]
            .ruleset
            .as_ref()
            .unwrap()
            .edition_id,
        "it-2025"
    );
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
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
    assert_registry_constraints(&db);
}

fn assert_registry_constraints(db: &rusqlite::Connection) {
    db.execute("INSERT INTO characters(name,system_id,edition,notes) VALUES ('Unmapped','mork-borg','unmapped','')", []).unwrap();
    let id = db.last_insert_rowid();
    for (game, edition) in [
        (None, Some("it-2025")),
        (Some("mork-borg"), None),
        (Some("mork-borg"), Some("missing")),
    ] {
        assert!(
            db.execute(
                "INSERT INTO character_rulesets VALUES (?1,?2,?3)",
                rusqlite::params![id, game, edition]
            )
            .is_err()
        );
    }
    for sql in [
        "INSERT INTO registry_editions(game_id,edition_id,label) VALUES ('missing','first','First')",
        "UPDATE registry_editions SET label='Changed'",
        "DELETE FROM legacy_ruleset_aliases",
    ] {
        assert!(db.execute(sql, []).is_err(), "Constraint accepted {sql}");
    }
}

#[test]
fn failed_registry_migration_rolls_back_schema_and_preserves_v4_records() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken.sqlite3");
    let db = rusqlite::Connection::open(&path).unwrap();
    for migration in [
        include_str!("../migrations/001_characters.sql"),
        include_str!("../migrations/002_mork_borg.sql"),
        include_str!("../migrations/003_sources.sql"),
        include_str!("../migrations/004_source_coverage.sql"),
    ] {
        db.execute_batch(migration).unwrap();
    }
    db.execute("INSERT INTO characters(name,system_id,edition,notes) VALUES ('Retained','mork-borg','fixture','Retained notes')", []).unwrap();
    db.execute("CREATE TABLE registry_editions (sentinel TEXT)", [])
        .unwrap();
    drop(db);
    assert!(Store::open(&path).is_err());
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        4
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='registry_games'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("SELECT notes FROM characters", [], |row| row
            .get::<_, String>(0))
            .unwrap(),
        "Retained notes"
    );
}

#[tokio::test]
async fn unicode_v2_labels_and_v1_missing_null_extension_are_explicit() {
    let dir = tempfile::tempdir().unwrap();
    fixture::fixture_pair(dir.path()).unwrap();
    let store = Store::open(&dir.path().join("unicode.sqlite3")).unwrap();
    let label = "界".repeat(100);
    store
        .register_ruleset(registration("synthetic-game", "first", &label))
        .await
        .unwrap();
    let bundle = dir.path().join("unknown-bundle");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(bundle.join("bundle.json")).unwrap()).unwrap();
    manifest["edition"] = label.clone().into();
    std::fs::write(
        bundle.join("bundle.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let source_id = store
        .import_source(&bundle, &dir.path().join("original.pdf"))
        .await
        .unwrap();
    accept_source_page(&store, source_id).await;
    assert_eq!(
        store
            .search_evidence("synthetic-game".into(), label, "capacity".into())
            .await
            .unwrap()
            .matches
            .len(),
        1
    );
    let (status, html) = lookup_html(
        &store,
        &format!(
            "system=synthetic-game&edition={}&query=capacity",
            "%E7%95%8C".repeat(100)
        ),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(html.contains("Strength capacity"));
    let legacy = dir.path().join("bundle");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(legacy.join("bundle.json")).unwrap()).unwrap();
    manifest["edition_id"] = serde_json::Value::Null;
    std::fs::write(
        legacy.join("bundle.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    assert!(
        store
            .import_source(&legacy, &dir.path().join("original.pdf"))
            .await
            .is_err()
    );
    assert_eq!(store.sources().await.unwrap().len(), 1);
}
