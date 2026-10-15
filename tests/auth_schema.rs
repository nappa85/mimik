use mimik::Store;
use rusqlite::Connection;

const BEFORE_SESSIONS: [&str; 10] = [
    include_str!("../migrations/001_characters.sql"),
    include_str!("../migrations/002_mork_borg.sql"),
    include_str!("../migrations/003_sources.sql"),
    include_str!("../migrations/004_source_coverage.sql"),
    include_str!("../migrations/005_registry.sql"),
    include_str!("../migrations/006_definition_drafts.sql"),
    include_str!("../migrations/007_source_finalization.sql"),
    include_str!("../migrations/008_definition_publication.sql"),
    include_str!("../migrations/009_review_integrity.sql"),
    include_str!("../migrations/010_accounts.sql"),
];

#[test]
fn schema10_to11_is_additive_reopens_and_collision_rolls_back() {
    let dir = tempfile::tempdir().unwrap();
    for collision in [false, true] {
        let path = dir.path().join(format!("schema-{collision}.sqlite3"));
        let db = Connection::open(&path).unwrap();
        for migration in BEFORE_SESSIONS {
            db.execute_batch(migration).unwrap();
        }
        db.execute("INSERT INTO accounts(login,display_name,state,epoch) VALUES('reserved','Retained identity','pending',1)",[]).unwrap();
        if collision {
            db.execute_batch("CREATE TABLE auth_sessions(retained TEXT); INSERT INTO auth_sessions VALUES('user-owned retained row');").unwrap();
        }
        drop(db);
        if collision {
            assert!(Store::open(&path).is_err());
        } else {
            drop(Store::open(&path).unwrap());
            drop(Store::open(&path).unwrap());
        }
        let db = Connection::open(&path).unwrap();
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            if collision { 10 } else { mimik::SCHEMA_VERSION }
        );
        assert_eq!(
            db.query_row("SELECT display_name FROM accounts", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "Retained identity"
        );
        if collision {
            assert_eq!(
                db.query_row("SELECT retained FROM auth_sessions", [], |r| r
                    .get::<_, String>(0))
                    .unwrap(),
                "user-owned retained row"
            );
        } else {
            assert_eq!(
                db.query_row("SELECT count(*) FROM auth_sessions", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
    }
}
