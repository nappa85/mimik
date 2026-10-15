#[path = "support/schema_fixture.rs"]
mod schema_fixture;

use rusqlite::{Connection, types::Value};

#[derive(Debug, PartialEq)]
struct Snapshot {
    version: i64,
    schema: Vec<(String, String, String, Option<String>)>,
    rows: Vec<(String, Vec<Vec<Value>>)>,
}

fn snapshot(db: &Connection) -> Snapshot {
    let schema = db
        .prepare("SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<Result<Vec<(String, String, String, Option<String>)>, _>>()
        .unwrap();
    let rows = schema
        .iter()
        .filter(|(kind, _, _, _)| kind == "table")
        .map(|(_, table, _, _)| {
            let mut query = db
                .prepare(&format!("SELECT * FROM \"{table}\" ORDER BY rowid"))
                .unwrap();
            let columns = query.column_count();
            let rows = query
                .query_map([], |row| {
                    (0..columns)
                        .map(|i| row.get(i))
                        .collect::<Result<Vec<Value>, _>>()
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            (table.clone(), rows)
        })
        .collect();
    Snapshot {
        version: db
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap(),
        schema,
        rows,
    }
}

#[test]
fn populated_session_rewind_rejects_before_any_drop_and_preserves_all_state() {
    for mode in 0..3 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.sqlite3");
        drop(mimik::Store::open(&path).unwrap());
        let db = Connection::open(&path).unwrap();
        db.create_scalar_function(
            "mimik_auth_now",
            0,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8,
            |_| Ok(100_i64),
        )
        .unwrap();
        let payload = serde_json::json!({"csrf":"a".repeat(64)}).to_string();
        db.execute(
            "INSERT INTO auth_sessions VALUES(?1,100,700,'active',?2,NULL,NULL)",
            rusqlite::params![tower_sessions::session::Id::default().to_string(), payload],
        )
        .unwrap();
        let before = snapshot(&db);
        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if mode == 2 {
                schema_fixture::remove_empty_account_tables(&db);
            } else {
                schema_fixture::pre_finalization(&db, mode == 1);
            }
        }))
        .expect_err("populated session fixture accepted");
        assert_eq!(snapshot(&db), before);
        let message = rejected.downcast_ref::<String>().unwrap();
        assert!(
            message.contains("Refusing populated account fixture table auth_sessions"),
            "{message}"
        );
    }
}

#[test]
fn populated_fixture_rejection_preserves_every_table_row_trigger_and_version() {
    for (table, mode) in [
        "accounts",
        "account_owner",
        "account_credentials",
        "account_events",
    ]
    .into_iter()
    .flat_map(|table| (0..3).map(move |mode| (table, mode)))
    {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.sqlite3");
        let store = mimik::Store::open(&path).unwrap();
        drop(store);
        let db = Connection::open(path).unwrap();
        // Accounts-only exercises the last count, after three empty tables.
        let verifier = format!("$argon2id$v=19$m=19456,t=2,p=1${}", "a".repeat(66));
        db.execute("INSERT INTO accounts(login,display_name,state,epoch,verifier) VALUES ('fixture','Fixture','enabled',1,?1)", [verifier]).unwrap();
        match table {
            "account_owner" => {
                db.execute("INSERT INTO account_owner VALUES (1,1)", [])
                    .unwrap();
            }
            "account_credentials" => {
                db.execute("INSERT INTO account_credentials VALUES (zeroblob(32),1,'reset',1,1,3601,'active')", []).unwrap();
            }
            "account_events" => {
                db.execute(
                    "INSERT INTO account_events VALUES (1,1,1,1,'local_operator','bootstrap')",
                    [],
                )
                .unwrap();
            }
            "accounts" => {}
            _ => unreachable!(),
        }
        let before = snapshot(&db);
        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if mode == 2 {
                schema_fixture::remove_empty_account_tables(&db);
            } else {
                schema_fixture::pre_finalization(&db, mode == 1);
            }
        }));
        assert!(
            snapshot(&db) == before,
            "Rejected fixture changed retained state"
        );
        let panic = rejected.expect_err("Populated fixture was accepted");
        let message = panic.downcast_ref::<String>().unwrap();
        assert!(message.contains(&format!("Refusing populated account fixture table {table}")));
    }
}

#[test]
fn unknown_version_rejection_preserves_all_state() {
    for version in [6, 9, 16] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unknown.sqlite3");
        drop(mimik::Store::open(&path).unwrap());
        let db = Connection::open(path).unwrap();
        db.pragma_update(None, "user_version", version).unwrap();
        let before = snapshot(&db);
        for remigrate in [false, true] {
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    schema_fixture::pre_finalization(&db, remigrate);
                }))
                .is_err()
            );
            assert!(snapshot(&db) == before);
        }
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                schema_fixture::remove_empty_account_tables(&db);
            }))
            .is_err()
        );
        assert!(snapshot(&db) == before);
    }
}

#[test]
fn later_rewind_failure_rolls_back_account_and_finalization_drops() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollback.sqlite3");
    drop(mimik::Store::open(&path).unwrap());
    let db = Connection::open(path).unwrap();
    db.execute_batch("DROP TRIGGER unit_review_page_cascade")
        .unwrap();
    let before = snapshot(&db);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            schema_fixture::pre_finalization(&db, true);
        }))
        .is_err()
    );
    assert!(snapshot(&db) == before);
}

#[test]
fn missing_account_table_rejects_before_drops_and_empty_fixtures_remigrate() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.sqlite3");
    drop(mimik::Store::open(&path).unwrap());
    let db = Connection::open(&path).unwrap();
    db.execute_batch("DROP TABLE accounts").unwrap();
    let before = snapshot(&db);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            schema_fixture::remove_empty_account_tables(&db);
        }))
        .is_err()
    );
    assert!(snapshot(&db) == before);
    drop(db);
    for remigrate in [false, true] {
        let path = dir.path().join(format!("empty-{remigrate}.sqlite3"));
        drop(mimik::Store::open(&path).unwrap());
        let db = Connection::open(&path).unwrap();
        schema_fixture::pre_finalization(&db, remigrate);
        assert_eq!(snapshot(&db).version, if remigrate { 15 } else { 6 });
        drop(db);
        drop(mimik::Store::open(&path).unwrap());
        let db = Connection::open(path).unwrap();
        assert_eq!(snapshot(&db).version, 15);
    }
}
