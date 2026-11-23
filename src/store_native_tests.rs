//! Migration 038: the native MÖRK BORG sheet goes (PROD-25). Native sheet data is refused, not
//! converted (pre-release data is disposable); the seeded MÖRK BORG registry pair stays as an
//! ordinary game when anything refers to it and is deleted otherwise.
use super::{AppError, MIGRATIONS, SCHEMA_VERSION, Store, register_auth_clock};
use rusqlite::Connection;
use std::{path::PathBuf, sync::Arc};

const VERIFIER: &str = "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const SEED_ALIAS: &str = "MÖRK BORG — IT 2025";

/// Opens a database at schema 037, runs `setup` with triggers, foreign keys and CHECKs out of
/// the way (it plants rows the application can no longer create), then migrates it.
fn upgrade(
    setup: impl FnOnce(&Connection),
) -> (tempfile::TempDir, PathBuf, Result<Store, AppError>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .unwrap();
    crate::auth::register_domain_guard(&connection).unwrap();
    register_auth_clock(&connection, Arc::new(|| Ok(100))).unwrap();
    for migration in &MIGRATIONS[..37] {
        connection.execute_batch(migration).unwrap();
    }
    let triggers: Vec<(String, String)> = connection
        .prepare("SELECT name,sql FROM sqlite_schema WHERE type='trigger'")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for (name, _) in &triggers {
        connection
            .execute_batch(&format!("DROP TRIGGER \"{name}\""))
            .unwrap();
    }
    connection
        .execute_batch("PRAGMA foreign_keys = OFF; PRAGMA ignore_check_constraints = ON;")
        .unwrap();
    setup(&connection);
    connection
        .execute_batch("PRAGMA ignore_check_constraints = OFF; PRAGMA foreign_keys = ON;")
        .unwrap();
    for (_, sql) in &triggers {
        connection.execute_batch(sql).unwrap();
    }
    (dir, path, Store::migrate(connection))
}

fn counts(db: &Connection) -> (i64, i64, i64) {
    let count = |table: &str| {
        db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    };
    (
        count("registry_games"),
        count("registry_editions"),
        count("legacy_ruleset_aliases"),
    )
}

fn has_native_column(db: &Connection) -> bool {
    db.query_row(
        "SELECT count(*) FROM pragma_table_info('registry_editions') WHERE name='native'",
        [],
        |r| r.get::<_, i64>(0),
    )
    .unwrap()
        == 1
}

fn table_exists(db: &Connection, name: &str) -> bool {
    db.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name=?1",
        [name],
        |r| r.get::<_, i64>(0),
    )
    .unwrap()
        == 1
}

fn foreign_key_violations(db: &Connection) -> Vec<String> {
    db.prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn refused(result: Result<Store, AppError>) -> &'static str {
    match result {
        Err(AppError::Invalid(message)) => message,
        Err(other) => panic!("expected the native-data refusal, got {other}"),
        Ok(_) => panic!("expected the native-data refusal, the migration ran"),
    }
}

/// The refusing dependents: the native sheet table and a native private sheet.
const NATIVE_DATA: [(&str, &str); 2] = [
    (
        "mork_borg_sheets",
        "INSERT INTO characters(id,name,system_id,edition) VALUES(1,'Old','mork-borg','MÖRK BORG — IT 2025'); \
         INSERT INTO mork_borg_sheets(character_id,data) VALUES(1,'{}');",
    ),
    (
        "native private sheet",
        "INSERT INTO private_sheets(id,campaign_id,custodian_id,game_id,edition_id,kind,pin,revision,state,snapshot,hash) \
         VALUES(1,1,1,'mork-borg','it-2025','native','{}',1,'active','{}','0000000000000000000000000000000000000000000000000000000000000000');",
    ),
];

#[test]
fn native_sheet_data_is_refused_with_an_operator_message_and_nothing_changes() {
    for (name, sql) in NATIVE_DATA {
        let (_dir, path, result) = upgrade(|db| db.execute_batch(sql).unwrap());
        let message = refused(result);
        assert!(
            message.contains("native MÖRK BORG sheet data")
                && message.contains("`sources/` folder"),
            "{name}: {message}"
        );
        // The refusal rolls the migration back: version, seed and the native table stay.
        let db = Connection::open(&path).unwrap();
        let version: i64 = db
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 37, "{name}");
        assert_eq!(counts(&db), (1, 1, 1), "{name}");
        assert!(table_exists(&db, "mork_borg_sheets"), "{name}");
        assert!(has_native_column(&db), "{name}");
    }
}

#[test]
fn a_clean_upgrade_from_037_deletes_the_unused_seed_and_the_native_schema() {
    let (_dir, _path, store) = upgrade(|db| assert_eq!(counts(db), (1, 1, 1)));
    let store = store.unwrap();
    store
        .upload_storage(|db| {
            assert_eq!(
                db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
                SCHEMA_VERSION
            );
            assert_eq!(counts(db), (0, 0, 0));
            assert!(!has_native_column(db));
            assert!(!table_exists(db, "mork_borg_sheets"));
            assert!(!table_exists(db, "native_seed_unused"));
            assert_eq!(foreign_key_violations(db), Vec::<String>::new());
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_fresh_database_has_no_game_and_keeps_registry_history_append_only() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db")).unwrap();
    store
        .upload_storage(|db| {
            assert_eq!(counts(db), (0, 0, 0));
            assert!(!has_native_column(db));
            assert!(!table_exists(db, "mork_borg_sheets"));
            db.execute_batch(
                "INSERT INTO registry_games VALUES('g','G'); INSERT INTO registry_editions(game_id,edition_id,label) VALUES('g','e','E');",
            )?;
            // The immutable-delete triggers dropped and recreated by 038 still hold.
            for sql in [
                "DELETE FROM registry_editions",
                "DELETE FROM registry_games",
                "INSERT INTO legacy_ruleset_aliases VALUES('g','E','g','e'); DELETE FROM legacy_ruleset_aliases",
            ] {
                assert!(db.execute_batch(sql).is_err(), "{sql}");
            }
            let insert: String = db.query_row(
                "SELECT sql FROM sqlite_schema WHERE name='private_sheets_insert'",
                [],
                |r| r.get(0),
            )?;
            assert!(!insert.contains("'native'") && insert.contains("IS NOT 'runtime'"));
            Ok(())
        })
        .unwrap();
}

/// Everything that makes the seeded pair "used": each keeps it as an ordinary registered game.
const REFERENCES: [(&str, &str); 12] = [
    (
        "campaign",
        "INSERT INTO accounts(id,login,display_name,state,epoch,verifier) VALUES(1,'owner','Owner','enabled',1,'$VERIFIER'); \
         INSERT INTO campaigns(id,creator_id,game_id,edition_id,title,revision) VALUES(1,1,'mork-borg','it-2025','Table',1); \
         INSERT INTO campaign_members(campaign_id,account_id,revision,state,role) VALUES(1,1,1,'active','master');",
    ),
    (
        "legacy character",
        "INSERT INTO characters(id,name,system_id,edition) VALUES(1,'Old','mork-borg','MÖRK BORG — IT 2025');",
    ),
    (
        "character ruleset",
        "INSERT INTO characters(id,name,system_id,edition) VALUES(1,'Old','other','Other'); \
         INSERT INTO character_rulesets VALUES(1,'mork-borg','it-2025');",
    ),
    (
        "source",
        "INSERT INTO sources(id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,raw_json,prepared_json) \
         VALUES(1,'b','s','T','other','Other','en','h',x'7b7d',x'7b7d'); \
         INSERT INTO source_rulesets VALUES(1,'mork-borg','it-2025');",
    ),
    (
        "source with the seed alias",
        "INSERT INTO sources(id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,raw_json,prepared_json) \
         VALUES(1,'b','s','T','mork-borg','MÖRK BORG — IT 2025','en','h',x'7b7d',x'7b7d');",
    ),
    (
        "pending upload",
        "INSERT INTO accounts(id,login,display_name,state,epoch,verifier) VALUES(1,'owner','Owner','enabled',1,'$VERIFIER'); \
         INSERT INTO source_uploads(id,pdf_sha256,pdf_bytes,actor,actor_id,actor_epoch,session_digest,created_at,updated_at,state,system_id,edition,language) \
         VALUES('00000000000000000000000000000000','HASH',10,'account:1',1,1,'HASH',1,1,'uploaded','mork-borg','MÖRK BORG — IT 2025','en');",
    ),
    (
        "definition draft",
        "INSERT INTO definition_drafts(game_id,edition_id,definition_id,revision,content,content_hash) \
         VALUES('mork-borg','it-2025','d',1,'{\"contract_version\":1,\"game_id\":\"mork-borg\",\"edition_id\":\"it-2025\",\"definition_id\":\"d\"}','HASH');",
    ),
    (
        "definition version",
        "INSERT INTO definition_versions(game_id,edition_id,definition_id,version,content,content_hash) \
         VALUES('mork-borg','it-2025','d',1,'{\"contract_version\":1,\"game_id\":\"mork-borg\",\"edition_id\":\"it-2025\",\"definition_id\":\"d\"}','HASH');",
    ),
    (
        "catalogue",
        "INSERT INTO catalogues(game_id,edition_id,catalogue_id) VALUES('mork-borg','it-2025','c');",
    ),
    (
        "catalogue draft",
        "INSERT INTO catalogue_drafts(game_id,edition_id,catalogue_id,revision,content,content_hash) \
         VALUES('mork-borg','it-2025','c',1,'{\"contract_version\":1,\"game_id\":\"mork-borg\",\"edition_id\":\"it-2025\",\"catalogue_id\":\"c\"}','HASH');",
    ),
    (
        "registry event",
        "INSERT INTO registry_events(kind,game_id,edition_id,game_label,label) VALUES('ruleset','mork-borg','it-2025','MÖRK BORG','MÖRK BORG — IT 2025');",
    ),
    (
        "second alias",
        "INSERT INTO legacy_ruleset_aliases VALUES('mork-borg','Another label','mork-borg','it-2025');",
    ),
];

#[test]
fn the_seed_stays_as_an_ordinary_game_when_anything_refers_to_it() {
    for (name, sql) in REFERENCES {
        let sql = sql.replace("$VERIFIER", VERIFIER).replace("HASH", HASH);
        let (_dir, _path, store) = upgrade(|db| db.execute_batch(&sql).unwrap());
        let store = store.unwrap_or_else(|error| panic!("{name}: {error}"));
        store
            .upload_storage(|db| {
                let expected_aliases = i64::from(name == "second alias") + 1;
                assert_eq!(counts(db), (1, 1, expected_aliases), "{name}");
                assert!(!has_native_column(db), "{name}");
                assert!(!table_exists(db, "mork_borg_sheets"), "{name}");
                assert!(foreign_key_violations(db).is_empty(), "{name}");
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn only_the_seeded_edition_goes_when_the_game_has_another_one() {
    let (_dir, _path, store) = upgrade(|db| {
        db.execute_batch(
            "INSERT INTO registry_editions(game_id,edition_id,label) VALUES('mork-borg','en-2023','MÖRK BORG — EN 2023');",
        )
        .unwrap();
    });
    store
        .unwrap()
        .upload_storage(|db| {
            let editions: Vec<String> = db
                .prepare("SELECT edition_id FROM registry_editions")?
                .query_map([], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            assert_eq!(editions, ["en-2023"]);
            let games: i64 =
                db.query_row("SELECT count(*) FROM registry_games", [], |r| r.get(0))?;
            assert_eq!(games, 1);
            assert_eq!(counts(db).2, 0, "the seed alias {SEED_ALIAS} is gone");
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_database_rewound_below_038_is_not_migrated_twice() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let store = Store::open(&path).unwrap();
    store
        .upload_storage(|db| {
            db.execute_batch(
                "INSERT INTO registry_games VALUES('g','G'); INSERT INTO registry_editions(game_id,edition_id,label) VALUES('g','e','E');",
            )?;
            db.execute_batch("PRAGMA user_version=36;")?;
            Ok(())
        })
        .unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    store
        .upload_storage(|db| {
            assert_eq!(
                db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
                SCHEMA_VERSION
            );
            assert_eq!(counts(db), (1, 1, 0));
            assert!(!has_native_column(db));
            Ok(())
        })
        .unwrap();
}
