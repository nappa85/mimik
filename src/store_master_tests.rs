//! Migration 039: the Master view's NPC classification and seen marks (CAMP-14, ROAD-8). The
//! classification is added without touching a stored sheet or its history, and a database
//! rewound below 039 is not migrated twice.
use super::{MIGRATIONS, SCHEMA_VERSION, Store, register_auth_clock};
use rusqlite::Connection;
use std::sync::Arc;

const VERIFIER: &str = "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn names(db: &Connection, kind: &str) -> Vec<String> {
    db.prepare("SELECT name FROM sqlite_schema WHERE type=?1 AND (name LIKE '%npc%' OR name LIKE 'master_sheet_seen%' OR name='private_sheets_classes') ORDER BY name")
        .unwrap()
        .query_map([kind], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn assert_039(db: &Connection) {
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM pragma_table_info('private_sheets') WHERE name='is_npc'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        names(db, "trigger"),
        [
            "master_sheet_seen_delete",
            "master_sheet_seen_insert",
            "master_sheet_seen_update",
            "private_sheets_npc_immutable",
            "private_sheets_npc_insert",
        ]
    );
    assert_eq!(names(db, "table"), ["master_sheet_seen"]);
    assert_eq!(names(db, "index"), ["private_sheets_classes"]);
}

/// Schema 038 with one stored private sheet and its history (planted with the guards out of
/// the way: only the bytes matter here), migrated to the current schema.
#[test]
fn a_populated_database_keeps_every_sheet_and_history_byte_and_gets_player_sheets() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let connection = Connection::open(&path).unwrap();
    crate::auth::register_domain_guard(&connection).unwrap();
    register_auth_clock(&connection, Arc::new(|| Ok(100))).unwrap();
    for migration in &MIGRATIONS[..38] {
        connection.execute_batch(migration).unwrap();
    }
    let snapshot = r#"{"encoding":1,"name":"Kept"}"#;
    // The guards (triggers, foreign keys, CHECKs) stay out of the way while rows they would
    // refuse are planted; the triggers come back before the migration runs.
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
        .execute_batch(&format!(
            "PRAGMA foreign_keys=OFF; PRAGMA ignore_check_constraints=ON;
             INSERT INTO accounts(id,login,display_name,state,epoch,verifier) VALUES(1,'a','A','enabled',1,'{VERIFIER}');
             INSERT INTO private_sheets(id,campaign_id,custodian_id,game_id,edition_id,kind,pin,revision,state,snapshot,hash)
              VALUES(1,1,1,'g','e','runtime','{{}}',1,'active','{snapshot}','{HASH}');
             INSERT INTO private_sheet_history(sheet_id,revision,hash,snapshot,operation,previous_revision,actor_id,actor_epoch,session_digest,occurred_at)
              VALUES(1,1,'{HASH}','{snapshot}','create',0,1,1,'{HASH}',100);
             PRAGMA ignore_check_constraints=OFF;"
        ))
        .unwrap();
    for (_, sql) in &triggers {
        connection.execute_batch(sql).unwrap();
    }
    let before: (String, String, i64, String, String) = connection
        .query_row(
            "SELECT s.snapshot,h.snapshot,s.revision,s.hash,h.hash FROM private_sheets s JOIN private_sheet_history h ON h.sheet_id=s.id",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    let violations = |db: &Connection| -> Vec<String> {
        db.prepare("PRAGMA foreign_key_check")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    let violations_before = violations(&connection);
    let store = Store::migrate(connection).unwrap();
    store
        .upload_storage(|db| {
            assert_039(db);
            let after: (String, String, i64, String, String, i64) = db.query_row(
                "SELECT s.snapshot,h.snapshot,s.revision,s.hash,h.hash,s.is_npc FROM private_sheets s JOIN private_sheet_history h ON h.sheet_id=s.id",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )?;
            assert_eq!((after.0, after.1, after.2, after.3, after.4), before);
            assert_eq!(after.5, 0, "an existing sheet is a player sheet");
            assert_eq!(violations(db), violations_before, "no new foreign key violation");
            let seen: i64 = db.query_row("SELECT count(*) FROM master_sheet_seen", [], |r| r.get(0))?;
            assert_eq!(seen, 0, "no mark is manufactured");
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_fresh_database_has_the_039_objects() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db")).unwrap();
    store
        .upload_storage(|db| {
            assert_039(db);
            Ok(())
        })
        .unwrap();
}

/// Plants one NPC sheet with a Master's seen mark on revision 1 into the database at `path`
/// (guards out of the way, as above) and sets its version back to `version`.
fn plant_marked_npc_and_rewind(path: &std::path::Path, version: i64) {
    let connection = Connection::open(path).unwrap();
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
        .execute_batch(&format!(
            "PRAGMA foreign_keys=OFF;
             INSERT OR IGNORE INTO accounts(id,login,display_name,state,epoch,verifier) VALUES(1,'a','A','enabled',1,'{VERIFIER}');
             INSERT OR IGNORE INTO private_sheets(id,campaign_id,custodian_id,game_id,edition_id,kind,pin,revision,state,snapshot,hash,is_npc)
              VALUES(1,1,1,'g','e','runtime','{{}}',1,'active','{{}}','{HASH}',1);
             INSERT OR IGNORE INTO private_sheet_history(sheet_id,revision,hash,snapshot,operation,previous_revision,actor_id,actor_epoch,session_digest,occurred_at)
              VALUES(1,1,'{HASH}','{{}}','create',0,1,1,'{HASH}',100);
             INSERT OR IGNORE INTO master_sheet_seen(master_id,sheet_id,seen_revision) VALUES(1,1,1);
             PRAGMA user_version={version};"
        ))
        .unwrap();
    for (_, sql) in &triggers {
        connection.execute_batch(sql).unwrap();
    }
}

/// Regression: a database whose version was set back below 039 (a restore, or a test fixture
/// rewinding the schema) already has `is_npc`; reopening must skip only the `ALTER`, run the
/// idempotent rest and keep the classification, the marks and the triggers.
#[test]
fn a_database_rewound_below_039_is_not_migrated_twice() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    drop(Store::open(&path).unwrap());
    // Rewound to 038, then much further (36) with the 039 objects present: the later
    // migrations' own guards and 039's still leave one consistent schema and the rows.
    for version in [38, 36] {
        plant_marked_npc_and_rewind(&path, version);
        let store = Store::open(&path).unwrap();
        store
            .upload_storage(|db| {
                assert_039(db);
                let kept: (i64, i64) = db.query_row(
                    "SELECT s.is_npc,m.seen_revision FROM private_sheets s JOIN master_sheet_seen m ON m.sheet_id=s.id",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                assert_eq!(kept, (1, 1), "the classification and the mark survive at {version}");
                Ok(())
            })
            .unwrap();
    }
}
