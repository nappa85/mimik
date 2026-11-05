//! Construct real pre-finalization fixtures; never disable guards on normal finalized tests.
#[allow(
    dead_code,
    reason = "Shared support module; some integration suites only rewind accounts"
)]
pub fn pre_finalization(db: &rusqlite::Connection, remigrate: bool) {
    let transaction = db.unchecked_transaction().unwrap();
    remove_empty_accounts(&transaction);
    rewind_finalization(&transaction, remigrate);
    transaction.commit().unwrap();
}

/// Test-only rewind prerequisite, before any historical fixture drops.
#[allow(
    dead_code,
    reason = "Shared support module; some integration suites only rewind finalization"
)]
pub fn remove_empty_account_tables(db: &rusqlite::Connection) {
    let transaction = db.unchecked_transaction().unwrap();
    remove_empty_accounts(&transaction);
    drop_search_schema(&transaction);
    transaction.commit().unwrap();
}

fn remove_empty_accounts(db: &rusqlite::Connection) {
    let version: i64 = db
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        version,
        current_schema(),
        "Account fixture requires the current schema, found {version}"
    );
    let tables = [
        "registry_web_events",
        "source_web_events",
        "account_web_events",
        // Change notes (schema 20) reference history; children first.
        "private_sheet_notes",
        "private_sheet_history",
        "private_sheets",
        "campaign_events",
        "campaign_requests",
        "campaign_members",
        "campaigns",
        "auth_sessions",
        "account_events",
        "account_credentials",
        "account_owner",
        "accounts",
        // Catalogue objects (schema 19) are newer than every rewind target as well; children
        // first.
        "catalogue_names_fts",
        "catalogue_seals",
        "catalogue_entry_units",
        "catalogue_entries",
        "catalogue_entry_types",
        "catalogue_versions",
        "catalogues",
        "catalogue_drafts",
        // The assistant's model setting (schema 22) and downloads (schema 23); the choice references the models.
        "assistant_downloads",
        "assistant_model_choice",
        "assistant_models",
    ];
    // Collect every count before validating or dropping any table.
    let counts = tables.map(|table| {
        db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap_or_else(|error| panic!("Unexpected account fixture schema ({table}): {error}"))
    });
    for (table, count) in tables.into_iter().zip(counts) {
        assert_eq!(count, 0, "Refusing populated account fixture table {table}");
    }
    rewind_documents(db);
    db.execute_batch("DROP VIEW account_web_request; DROP VIEW campaign_request_auth;")
        .unwrap();
    db.execute_batch("DROP VIEW private_sheet_events_valid;")
        .unwrap();
    for table in tables {
        db.execute_batch(&format!("DROP TABLE {table};")).unwrap();
    }
    // Registry history is newer than every rewind target. Discarding its (unattributed CLI)
    // rows is intentional: older schemas never had history, and remigration starts empty
    // exactly like an upgraded pre-history database. Its attributed sidecar must be empty
    // (checked above). Drop after the checks so rejected fixtures stay unchanged.
    db.execute_batch("DROP TABLE registry_events;").unwrap();
    db.execute_batch("DROP INDEX source_review_page_history;")
        .unwrap();
}

/// Drop every `source_search_*` object listed in `sqlite_master` (derived search data;
/// the next `Store::open` re-applies the migration and rebuilds it).
pub fn drop_search_schema(db: &rusqlite::Connection) {
    rewind_documents(db);
    let objects = |kind: &str| -> Vec<String> {
        db.prepare("SELECT name FROM sqlite_master WHERE type=?1 AND name GLOB 'source_search_*' AND sql IS NOT NULL ORDER BY sql LIKE 'CREATE VIRTUAL TABLE%' DESC, name")
            .unwrap()
            .query_map([kind], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    for trigger in objects("trigger") {
        db.execute_batch(&format!("DROP TRIGGER \"{trigger}\";"))
            .unwrap();
    }
    for table in objects("table") {
        // Dropping the FTS table also removes its shadow tables listed after it.
        db.execute_batch(&format!("DROP TABLE IF EXISTS \"{table}\";"))
            .unwrap();
    }
    let left: i64 = db
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name GLOB 'source_search_*'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(left, 0, "Search schema objects remain after drop");
}

fn rewind_finalization(db: &rusqlite::Connection, remigrate: bool) {
    db.execute_batch("DROP TRIGGER IF EXISTS page_completion_integrity; DROP TRIGGER IF EXISTS page_completion_event_integrity; DROP TRIGGER IF EXISTS definition_approvals_provenance;").unwrap();
    db.execute_batch("DROP TABLE definition_withdrawals; DROP TABLE definition_approvals; DROP TABLE definition_versions;").unwrap();
    let triggers = [
        "source_page_hash_insert",
        "source_finalize_insert",
        "source_finalize_update",
        "source_finalize_delete",
        "source_page_hash_update",
        "source_page_hash_delete",
        "sources_finalized_insert",
        "sources_finalized_update",
        "sources_finalized_delete",
        "pages_finalized_insert",
        "pages_finalized_update",
        "pages_finalized_delete",
        "units_finalized_insert",
        "units_finalized_update",
        "units_finalized_delete",
        "source_rulesets_finalized_insert",
        "source_rulesets_finalized_update",
        "source_rulesets_finalized_delete",
        "page_review_revision",
        "unit_review_revision",
        "source_review_event_insert",
        "source_review_event_update",
        "source_review_event_delete",
    ];
    for trigger in triggers {
        db.execute_batch(&format!("DROP TRIGGER {trigger};"))
            .unwrap();
    }
    db.execute_batch(
        "DROP TABLE source_page_hashes; DROP TABLE source_finalizations; PRAGMA user_version=6;",
    )
    .unwrap();
    db.execute_batch("DROP TRIGGER unit_review_page_cascade;")
        .unwrap();
    if !remigrate {
        // A real schema-6 database has no search objects; Store::open rebuilds them.
        drop_search_schema(db);
    }
    if remigrate {
        db.execute_batch(include_str!("../../migrations/007_source_finalization.sql"))
            .unwrap();
        db.execute_batch(include_str!(
            "../../migrations/008_definition_publication.sql"
        ))
        .unwrap();
        db.execute_batch(include_str!("../../migrations/009_review_integrity.sql"))
            .unwrap();
        db.execute_batch(include_str!("../../migrations/010_accounts.sql"))
            .unwrap();
        db.execute_batch(include_str!("../../migrations/011_auth_sessions.sql"))
            .unwrap();
        db.execute_batch(include_str!("../../migrations/012_campaigns.sql"))
            .unwrap();
        db.execute_batch(include_str!(
            "../../migrations/013_owner_administration.sql"
        ))
        .unwrap();
        db.execute_batch(include_str!(
            "../../migrations/014_account_administration.sql"
        ))
        .unwrap();
        db.execute_batch(include_str!(
            "../../migrations/015_material_administration.sql"
        ))
        .unwrap();
        db.execute_batch(include_str!(
            "../../migrations/016_registry_administration.sql"
        ))
        .unwrap();
        db.execute_batch(include_str!("../../migrations/019_catalogues.sql"))
            .unwrap();
        db.execute_batch(include_str!("../../migrations/024_source_documents.sql"))
            .unwrap();
        db.execute_batch(include_str!("../../migrations/025_source_review.sql"))
            .unwrap();
        db.execute_batch(include_str!("../../migrations/026_source_uploads.sql"))
            .unwrap();
        // Search objects (017+) were kept intact for the live session, so the schema is
        // current again; derived rows need no re-application.
        db.pragma_update(None, "user_version", current_schema())
            .unwrap();
    }
}

/// Recreate a genuine pre-24 schema before historical migration fixtures.
pub fn rewind_documents(db: &rusqlite::Connection) {
    rewind_review(db);
    let present: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('source_units') WHERE name='origin')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    if present {
        // ALTER reparses schema triggers on this standalone fixture connection. This
        // closed dummy never authenticates a request; Store::open installs the real guard.
        db.create_scalar_function(
            "mimik_request_auth",
            0,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8,
            |_| Ok(Option::<String>::None),
        )
        .unwrap();
        db.create_scalar_function(
            "mimik_auth_now",
            0,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8,
            |_| Ok(0_i64),
        )
        .unwrap();
        db.execute_batch(include_str!("source_documents_rewind.sql"))
            .unwrap();
    }
}

/// Recreate schema 24 suggestions, including its original guards. Refuse new-only fixture data.
#[allow(dead_code, reason = "Shared historical fixture helper")]
pub fn rewind_review(db: &rusqlite::Connection) {
    rewind_uploads(db);
    db.create_scalar_function(
        "mimik_request_auth",
        0,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8,
        |_| Ok(Option::<String>::None),
    )
    .unwrap();
    db.create_scalar_function(
        "mimik_auth_now",
        0,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8,
        |_| Ok(0_i64),
    )
    .unwrap();
    let present: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='source_suggestion_reports')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    if !present {
        return;
    }
    for table in [
        "source_suggestion_reports",
        "source_suggestion_applications",
        "source_suggestion_web_events",
    ] {
        assert_eq!(
            db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "Cannot rewind populated {table}"
        );
    }
    let tx = db;
    tx.execute_batch(include_str!("source_review_rewind.sql"))
        .unwrap();
    let sql = include_str!("../../migrations/024_source_documents.sql")
        .split_once("CREATE TABLE source_suggestions (")
        .unwrap()
        .1;
    // Copy before installing the insert guard, which rejects pre-dismissed imports.
    let (table, triggers) = sql
        .split_once("CREATE INDEX source_suggestions_source")
        .unwrap();
    tx.execute_batch(&format!("CREATE TABLE source_suggestions ({table} INSERT INTO source_suggestions SELECT * FROM legacy_suggestions; DROP TABLE legacy_suggestions; CREATE INDEX source_suggestions_source{triggers}")).unwrap();
}

fn rewind_uploads(db: &rusqlite::Connection) {
    let exists: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='source_uploads')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    if !exists {
        return;
    }
    for table in [
        "source_upload_pages",
        "source_upload_exclusions",
        "source_jobs",
        "source_uploads",
    ] {
        assert_eq!(
            db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "Cannot rewind populated {table}"
        );
    }
    db.execute_batch("DROP VIEW source_upload_invalid_inspection; DROP VIEW source_upload_invalid_exclusion; DROP TABLE source_upload_pages; DROP TABLE source_upload_exclusions; DROP TABLE source_jobs; DROP TABLE source_uploads;").unwrap();
}

/// The migration-chain test pins the file count to the application schema version.
fn current_schema() -> i64 {
    let count = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"))
        .unwrap()
        .count();
    i64::try_from(count).unwrap()
}
