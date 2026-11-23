//! Migration 036 (SRC-15 slice 2) drops and recreates the four upload tables under the new
//! bounds. Dropping is intended: no release shipped and staged uploads are disposable (owner
//! decision 2026-11-11), so rows are not copied.
use super::{MIGRATIONS, SCHEMA_VERSION, Store, register_auth_clock};
use rusqlite::{Connection, params};
use serde_json::json;
use std::sync::Arc;

const ID: &str = "11111111111111111111111111111111";
const TABLES: [&str; 4] = [
    "source_uploads",
    "source_upload_pages",
    "source_upload_exclusions",
    "source_jobs",
];

fn at_version_35() -> (tempfile::TempDir, std::path::PathBuf, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .unwrap();
    crate::auth::register_domain_guard(&connection).unwrap();
    register_auth_clock(&connection, Arc::new(|| Ok(100))).unwrap();
    for migration in &MIGRATIONS[..35] {
        connection.execute_batch(migration).unwrap();
    }
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        35
    );
    (dir, path, connection)
}

fn inspection(pages: u32) -> String {
    let page = |n: u32| {
        json!({"n": n, "width": 10, "height": 10, "rotate": 0, "user_unit": 1, "tagged": false,
               "transform": [1, 0, 0, -1, 0, 10], "visible": [0, 0, 10, 10]})
    };
    let document = crate::sources::worker::records::Document {
        pages,
        encrypted: false,
        mupdf: "1.27.2".into(),
    };
    json!({"document": document,
           "pages": (1..=pages).map(page).collect::<Vec<_>>()})
    .to_string()
}

fn insert_upload(db: &Connection, pages: u32, selected: &str) -> rusqlite::Result<usize> {
    db.execute(
        "INSERT INTO source_uploads(id,pdf_sha256,pdf_bytes,actor,actor_id,actor_epoch,
         session_digest,created_at,updated_at,state,inspection,selected_pages,system_id,edition,language)
         VALUES(?1,?2,10,'account:1',1,1,?3,40,50,'rendered',?4,?5,'mork-borg','Synthetic','en')",
        params![ID, "a".repeat(64), "b".repeat(64), inspection(pages), selected],
    )
}

fn insert_page(db: &Connection, n: u32) -> rusqlite::Result<usize> {
    let stats = json!({"n": n, "chars": 0, "unmapped": 0, "image_area": 0, "shading_area": 0,
        "fonts_substituted": [], "fonts_skipped": 0, "incomplete": false, "dropped_lines": 0});
    db.execute(
        "INSERT INTO source_upload_pages(upload_id,pdf_page,rendered_revision,png_sha256,png_bytes,
         width_px,height_px,dpi,width_points,height_points,transform,stats,settings,text_sha256,
         text_bytes,renderer_crate_version,mupdf_version)
         VALUES(?1,?2,1,?3,100,10,10,100,10,10,'[1,0,0,-1,0,10]',?4,
         '{\"dpi\":100,\"colourspace\":\"rgb\",\"anti_aliasing\":8}',?3,0,'0.1.0','1.27.2')",
        params![ID, n, "c".repeat(64), stats.to_string()],
    )
}

fn insert_exclusion(
    db: &Connection,
    pages: &str,
    boxes: &str,
    fingerprints: &str,
) -> rusqlite::Result<usize> {
    db.execute(
        "INSERT INTO source_upload_exclusions(upload_id,revision,actor,actor_id,actor_epoch,
         session_digest,rationale,pages,boxes,proposal_fingerprints,created_at)
         VALUES(?1,(SELECT coalesce(max(revision),0)+1 FROM source_upload_exclusions),
         'account:1',1,1,?2,'Synthetic rationale',?3,?4,?5,60)",
        params![ID, "b".repeat(64), pages, boxes, fingerprints],
    )
}

fn count(db: &Connection, sql: &str) -> i64 {
    db.query_row(sql, [], |r| r.get(0)).unwrap()
}

fn seed_version_35(db: &Connection) {
    db.execute("INSERT INTO accounts(id,login,display_name,state,epoch,verifier) VALUES(1,'owner','Owner','enabled',1,'$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA')", []).unwrap();
    db.execute("INSERT INTO account_owner VALUES(1,1)", [])
        .unwrap();
    insert_upload(db, 1, "[1]").unwrap();
    db.execute(
        "UPDATE source_uploads SET fingerprint_key=?1",
        [vec![7_u8; 32]],
    )
    .unwrap();
    insert_page(db, 1).unwrap();
    // Version 35 stores one box per page and one fingerprint column.
    for (revision, fingerprint, removed) in [(1, None, 0), (2, Some("d".repeat(64)), 1)] {
        db.execute(
            "INSERT INTO source_upload_exclusions(upload_id,revision,actor,actor_id,actor_epoch,
             session_digest,rationale,pages,boxes,proposal_fingerprint,removed,created_at)
             VALUES(?1,?2,'account:1',1,1,?3,'Synthetic rationale','[1]','{\"1\":[1,1,2,2]}',?4,?5,60)",
            params![ID, revision, "b".repeat(64), fingerprint, removed],
        )
        .unwrap();
    }
    db.execute(
        "INSERT INTO source_jobs(upload_id,upload_revision,kind,state,pages_done,pages_total,reason,created_at,finished_at)
         VALUES(?1,1,'render','failed',1,1,'source_png_limit',40,50)",
        [ID],
    )
    .unwrap();
    for table in TABLES {
        assert!(
            count(db, &format!("SELECT count(*) FROM {table}")) > 0,
            "{table}"
        );
    }
}

#[test]
fn version_035_upload_rows_are_dropped_and_the_tables_come_back_under_the_new_bounds() {
    let (_dir, _path, connection) = at_version_35();
    seed_version_35(&connection);
    // The old bounds are what 036 replaces.
    assert!(insert_page(&connection, 2001).is_err());
    let store = Store::migrate(connection).unwrap();
    store
        .upload_storage(|db| {
            assert_eq!(
                db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
                SCHEMA_VERSION
            );
            // Intended: the four tables are recreated empty; nothing else is touched.
            for table in TABLES {
                assert_eq!(count(db, &format!("SELECT count(*) FROM {table}")), 0, "{table}");
            }
            assert_eq!(count(db, "SELECT count(*) FROM accounts"), 1);
            let violations = db
                .prepare("PRAGMA foreign_key_check")?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            assert!(violations.is_empty(), "{violations:?}");
            for view in ["source_upload_invalid_inspection", "source_upload_invalid_exclusion"] {
                assert_eq!(count(db, &format!("SELECT count(*) FROM {view}")), 0, "{view}");
            }
            assert_eq!(
                count(db, "SELECT count(*) FROM sqlite_master WHERE type='trigger' AND name LIKE 'source_upload%'"),
                8
            );
            assert_eq!(
                count(db, "SELECT count(*) FROM sqlite_master WHERE name='source_upload_single_slot'"),
                1
            );
            // Both columns exist with their defaults.
            insert_upload(db, 1, "[1]")?;
            assert_eq!(count(db, "SELECT document_changed FROM source_uploads"), 0);
            assert!(db
                .query_row("SELECT fingerprint_key FROM source_uploads", [], |r| r
                    .get::<_, Option<Vec<u8>>>(0))?
                .is_none());
            db.execute("UPDATE source_uploads SET document_changed=1", [])?;
            assert!(db.execute("UPDATE source_uploads SET document_changed=2", []).is_err());
            db.execute(
                "INSERT INTO source_jobs(upload_id,upload_revision,kind,state,reason,created_at)
                 VALUES(?1,1,'render','failed','document_changed',40)",
                [ID],
            )?;
            Ok(())
        })
        .unwrap();
}

/// A quadratic validation rule shows as minutes at 20,000 pages; each statement must stay fast.
fn within<T>(what: &str, work: impl FnOnce() -> T) -> T {
    let start = std::time::Instant::now();
    let result = work();
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "{what} took {elapsed:?}"
    );
    result
}

/// Boxes for pages 1..=n in the numeric key order the writer emits (a `BTreeMap<u32, _>`).
fn boxes_for(n: u32) -> String {
    format!(
        "{{{}}}",
        (1..=n)
            .map(|p| format!("\"{p}\":[[1.0,1.0,2.0,2.0]]"))
            .collect::<Vec<_>>()
            .join(",")
    )
}

#[test]
fn the_new_checks_accept_twenty_thousand_pages_and_refuse_one_more() {
    let (_dir, _path, connection) = at_version_35();
    seed_version_35(&connection);
    let store = Store::migrate(connection).unwrap();
    let all = |n: u32| {
        format!(
            "[{}]",
            (1..=n).map(|p| p.to_string()).collect::<Vec<_>>().join(",")
        )
    };
    store
        .upload_storage(|db| {
            // 20,001 pages in the inspection or the selection is refused; 20,000 is stored.
            assert!(insert_upload(db, 20_001, "[1]").is_err());
            assert!(insert_upload(db, 20_000, &all(20_001)).is_err());
            assert_eq!(count(db, "SELECT count(*) FROM source_uploads"), 0);
            within("20,000-page upload", || insert_upload(db, 20_000, &all(20_000)))?;
            assert_eq!(count(db, "SELECT count(*) FROM source_upload_invalid_inspection"), 0);
            assert!(db
                .execute("UPDATE source_uploads SET inspection=?1", [inspection(20_001)])
                .is_err());
            assert!(db
                .execute("UPDATE source_uploads SET selected_pages=?1", [all(20_001)])
                .is_err());
            within("20,000-entry selection", || {
                db.execute("UPDATE source_uploads SET selected_pages=?1", [all(20_000)])
            })?;
            // The old selection trigger limit of 2,000 is gone, 20,001 is refused by the trigger.
            assert!(db
                .execute("UPDATE source_uploads SET selected_pages='[20001]'", [])
                .is_err());
            insert_page(db, 2001)?;
            insert_page(db, 20_000)?;
            assert!(insert_page(db, 20_001).is_err());
            // A PDF has no size bound of its own any more.
            db.execute("UPDATE source_uploads SET pdf_bytes=1099511627776", [])?;
            assert!(db.execute("UPDATE source_uploads SET pdf_bytes=0", []).is_err());
            db.execute(
                "INSERT INTO source_jobs(upload_id,upload_revision,kind,state,pages_done,pages_total,created_at)
                 VALUES(?1,1,'render','running',20000,20000,40)",
                [ID],
            )?;
            for assignment in ["pages_total=20001", "pages_done=20001"] {
                assert!(
                    db.execute(&format!("UPDATE source_jobs SET {assignment}"), []).is_err(),
                    "{assignment}"
                );
            }
            // An exclusion over 20,000 pages, one box each; 20,001 pages are refused.
            within("20,000-page exclusion", || {
                insert_exclusion(db, &all(20_000), &boxes_for(20_000), "[]")
            })?;
            assert_eq!(
                within("view over the 20,000-page exclusion", || count(
                    db,
                    "SELECT count(*) FROM source_upload_invalid_exclusion"
                )),
                0
            );
            assert!(
                within("20,001-page exclusion", || insert_exclusion(
                    db,
                    &all(20_001),
                    &boxes_for(20_001),
                    "[]"
                ))
                .is_err()
            );
            // Descending pages, or keys in another order than the pages, are refused.
            let descending = format!(
                "[{}]",
                (1..=20_000).rev().map(|p: u32| p.to_string()).collect::<Vec<_>>().join(",")
            );
            assert!(
                within("descending pages", || insert_exclusion(
                    db,
                    &descending,
                    &boxes_for(20_000),
                    "[]"
                ))
                .is_err()
            );
            Ok(())
        })
        .unwrap();
}

fn eight_boxes(count: usize) -> String {
    let boxes: Vec<_> = (0..count).map(|i| json!([i, 1, i + 1, 2])).collect();
    json!({"1": boxes}).to_string()
}

#[test]
fn the_exclusion_shape_is_a_bounded_box_list_per_page_with_a_fingerprint_list() {
    let (_dir, _path, connection) = at_version_35();
    seed_version_35(&connection);
    let store = Store::migrate(connection).unwrap();
    let fingerprint = |c: char| c.to_string().repeat(64);
    store
        .upload_storage(|db| {
            insert_upload(db, 1, "[1]")?;
            // One box, eight boxes, and up to eight distinct fingerprints are valid.
            insert_exclusion(db, "[1]", &eight_boxes(1), "[]")?;
            insert_exclusion(
                db,
                "[1]",
                &eight_boxes(8),
                &json!([fingerprint('a')]).to_string(),
            )?;
            let eight: Vec<String> = "abcdef01".chars().map(fingerprint).collect();
            insert_exclusion(db, "[1]", &eight_boxes(2), &json!(eight).to_string())?;
            // Invalid rows are refused at insert...
            for (boxes, fingerprints) in [
                (eight_boxes(9), "[]".to_string()),
                (eight_boxes(0), "[]".into()),
                (json!({"1": [1, 1, 2, 2]}).to_string(), "[]".into()),
                (json!({"1": [[1, 1, 2]]}).to_string(), "[]".into()),
                (json!({"1": [[2, 1, 1, 2]]}).to_string(), "[]".into()),
                (json!({"1": [[1, 1, 4001, 2]]}).to_string(), "[]".into()),
                (
                    json!({"1": [[1, 1, 2, 2], [1, 1, 2, 2]]}).to_string(),
                    "[]".into(),
                ),
                (json!({"2": [[1, 1, 2, 2]]}).to_string(), "[]".into()),
                (eight_boxes(1), json!(["raw text"]).to_string()),
                (
                    eight_boxes(1),
                    json!([fingerprint('a'), fingerprint('a')]).to_string(),
                ),
                (eight_boxes(1), json!([fingerprint('A')]).to_string()),
                (eight_boxes(1), json!([1]).to_string()),
                (eight_boxes(1), json!(vec![fingerprint('a'); 9]).to_string()),
            ] {
                assert!(
                    insert_exclusion(db, "[1]", &boxes, &fingerprints).is_err(),
                    "{boxes} {fingerprints}"
                );
            }
            // ... and the view itself flags them (the trigger is dropped inside a savepoint).
            db.execute_batch("SAVEPOINT flagged; DROP TRIGGER source_upload_exclusion_insert;")?;
            let long: Vec<_> = (0..8)
                .map(|i| {
                    json!([
                        1_234.567_890_123_457 + f64::from(i),
                        1.0,
                        3_000.123_456_789_012,
                        2000.0
                    ])
                })
                .collect();
            for (boxes, fingerprints) in [
                (eight_boxes(9), "[]".to_string()),
                (eight_boxes(0), "[]".into()),
                (json!({"1": [1, 1, 2, 2]}).to_string(), "[]".into()),
                (json!({"1": [[2, 1, 1, 2]]}).to_string(), "[]".into()),
                (
                    json!({"1": [[1, 1, 2, 2], [1, 1, 2, 2]]}).to_string(),
                    "[]".into(),
                ),
                (json!({"1": long}).to_string(), "[]".into()),
                (eight_boxes(1), json!(["raw text"]).to_string()),
                (
                    eight_boxes(1),
                    json!([fingerprint('a'), fingerprint('a')]).to_string(),
                ),
            ] {
                let before = count(db, "SELECT count(*) FROM source_upload_invalid_exclusion");
                insert_exclusion(db, "[1]", &boxes, &fingerprints)?;
                assert_eq!(
                    count(db, "SELECT count(*) FROM source_upload_invalid_exclusion"),
                    before + 1,
                    "{boxes} {fingerprints}"
                );
            }
            db.execute_batch("ROLLBACK TO flagged; RELEASE flagged;")?;
            assert_eq!(
                count(db, "SELECT count(*) FROM source_upload_invalid_exclusion"),
                0
            );
            Ok(())
        })
        .unwrap();
}
