//! Owner replacement rows (migration 030): trigger refusals, reading order and the backfill.
use super::documents::{
    OwnerTable, OwnerUnit, REPLACEMENT_INSERT, TableSource, add_owner_operation, add_owner_table,
    first_in_reading_order, new_operation_id, record_replacement,
};
use super::{UnitReview, materials::ORDERED_UNITS};
use crate::{AppError, Store};
use rusqlite::{Connection, Transaction, params};

const BOX: &str = "[[10,10],[60,10],[60,40],[10,40]]";
const AREA: [[f64; 2]; 4] = [[10.0, 10.0], [60.0, 10.0], [60.0, 40.0], [10.0, 40.0]];

/// A finalized source with one page (200 x 300 points) of `units` extracted units (ids 1..) and
/// a second page with one unit (id `units + 1`).
fn fixture(units: i64) -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db")).unwrap();
    store
        .upload_storage(|db| {
            db.execute("INSERT INTO sources(id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages) VALUES(1,'fixture','fixture','T','mork-borg','E','en',?1,x'',x'',x'',2)", ["a".repeat(64)])?;
            db.execute("INSERT INTO source_pages(id,source_id,pdf_page,png) VALUES(1,1,1,x'89504e47')", [])?;
            db.execute("INSERT INTO source_pages(id,source_id,pdf_page,png) VALUES(2,1,2,x'89504e47')", [])?;
            for id in 1..=units {
                db.execute("INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(?1,1,?2,'Text',?3,'<p>Fixture</p>','Fixture')", params![id, format!("unit-{id}"), BOX])?;
            }
            db.execute("INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(?1,2,'other','Text',?2,'<p>Fixture</p>','Fixture')", params![units + 1, BOX])?;
            crate::sources::search::ensure(db)?;
            let prepared = serde_json::json!({"contract_version":1,"extraction_sha256":"b".repeat(64),"pages":[{"pdf_page":1,"width_points":200,"height_points":300},{"pdf_page":2,"width_points":200,"height_points":300}]});
            db.execute("UPDATE sources SET bundle_hash=?1,prepared_json=?2 WHERE id=1", params!["a".repeat(64), serde_json::to_vec(&prepared)?])?;
            db.execute("INSERT INTO source_page_hashes VALUES(1,1,?1)", ["c".repeat(64)])?;
            db.execute("INSERT INTO source_page_hashes VALUES(2,1,?1)", ["d".repeat(64)])?;
            db.execute("INSERT INTO source_finalizations VALUES(1,?1,?2)", params!["a".repeat(64), "b".repeat(64)])?;
            super::documents::register_import(db, 1)?;
            Ok(())
        })
        .unwrap();
    (dir, store)
}

fn owner_text(tx: &Transaction<'_>, operation: &str, text: &str) -> Result<i64, AppError> {
    let revision = tx.query_row("SELECT revision FROM source_pages WHERE id=1", [], |r| {
        r.get(0)
    })?;
    let unit = OwnerUnit {
        revision,
        polygon: AREA,
        text: text.into(),
        actor: "owner".into(),
        rationale: "Checked against the page".into(),
    };
    let event = add_owner_operation(tx, 1, &unit, Some(operation))?;
    Ok(tx.query_row(
        "SELECT unit_id FROM source_review_events WHERE id=?1",
        [event],
        |r| r.get(0),
    )?)
}

fn reject(tx: &Transaction<'_>, operation: &str, unit: i64) -> Result<(), AppError> {
    reject_on(tx, 1, operation, unit)
}

fn reject_on(tx: &Transaction<'_>, page: i64, operation: &str, unit: i64) -> Result<(), AppError> {
    let revision = tx.query_row(
        "SELECT revision FROM source_units WHERE id=?1",
        [unit],
        |r| r.get(0),
    )?;
    let review = UnitReview {
        revision,
        state: "rejected".into(),
        correction: String::new(),
        actor: "owner".into(),
        rationale: "Checked against the page".into(),
    };
    super::store::review_unit_operation(tx, page, unit, &review, Some(operation))?;
    Ok(())
}

/// Merge `targets` into one owner Text unit the way slice 2 will write it.
fn merge(tx: &Transaction<'_>, targets: &[i64]) -> Result<i64, AppError> {
    let operation = new_operation_id();
    let unit = owner_text(tx, &operation, "merged")?;
    for target in targets {
        reject(tx, &operation, *target)?;
    }
    let anchor = first_in_reading_order(tx, 1, targets.iter().copied())?;
    record_replacement(tx, unit, &operation, "merge", anchor, 1)?;
    Ok(unit)
}

/// Split `unit` into `parts` owner Text units the way slice 3 will write it.
fn split(tx: &Transaction<'_>, unit: i64, parts: i64) -> Result<Vec<i64>, AppError> {
    let operation = new_operation_id();
    let mut created = Vec::new();
    for part in 1..=parts {
        created.push(owner_text(tx, &operation, &format!("part {part}"))?);
    }
    reject(tx, &operation, unit)?;
    for (part, new) in (1..).zip(&created) {
        record_replacement(tx, *new, &operation, "split", unit, part)?;
    }
    Ok(created)
}

fn table(tx: &Transaction<'_>, units: &[i64]) -> Result<i64, AppError> {
    let revision = tx.query_row("SELECT revision FROM source_pages WHERE id=1", [], |r| {
        r.get(0)
    })?;
    let sources = units
        .iter()
        .map(|unit| {
            Ok(TableSource {
                unit: *unit,
                revision: tx.query_row(
                    "SELECT revision FROM source_units WHERE id=?1",
                    [unit],
                    |r| r.get(0),
                )?,
            })
        })
        .collect::<Result<_, AppError>>()?;
    let table = OwnerTable {
        revision,
        sources,
        cells: vec![vec!["a".into(), "b".into()]],
        header: false,
        actor: "owner".into(),
        rationale: "Columns split".into(),
    };
    Ok(add_owner_table(tx, 1, &table, |_, _| Ok(()))?.0)
}

fn order(db: &Connection) -> Vec<i64> {
    db.prepare(&format!(
        "{ORDERED_UNITS} SELECT id FROM ordered ORDER BY position"
    ))
    .unwrap()
    .query_map([1], |r| r.get(0))
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn key(db: &Connection, unit: i64) -> String {
    db.query_row(
        "SELECT order_key FROM source_owner_replacements WHERE unit_id=?1",
        [unit],
        |r| r.get(0),
    )
    .unwrap()
}

fn refused(result: Result<(), AppError>) {
    let message = format!("{:?}", result.expect_err("the insert must be refused"));
    assert!(
        message.contains("Owner replacement must name"),
        "unexpected refusal: {message}"
    );
}

/// An owner Text unit created with a `corrected` event in `operation`, and the anchor rejected
/// in it: everything the trigger needs, so each test removes exactly one fact.
fn scenario(tx: &Transaction<'_>, anchor: i64) -> Result<(String, i64), AppError> {
    let operation = new_operation_id();
    let unit = owner_text(tx, &operation, "text")?;
    reject(tx, &operation, anchor)?;
    Ok((operation, unit))
}

#[test]
fn trigger_accepts_the_valid_rows() {
    let (_dir, store) = fixture(10);
    store
        .upload_storage(|db| {
            let tx = db.transaction()?;
            let (op, unit) = scenario(&tx, 1)?;
            record_replacement(&tx, unit, &op, "split", 1, 1)?;
            let second = owner_text(&tx, &op, "second part")?;
            record_replacement(&tx, second, &op, "split", 1, 2)?;
            assert_eq!(key(&tx, unit), "00000000000000000001.0001");
            assert_eq!(key(&tx, second), "00000000000000000001.0002");
            let tbl = table(&tx, &[2])?;
            assert_eq!(key(&tx, tbl), "00000000000000000002.0001");
            let merged = merge(&tx, &[3, 4])?;
            assert_eq!(key(&tx, merged), "00000000000000000003.0001");
            tx.commit()?;
            Ok(())
        })
        .unwrap();
}

/// Correct `unit` (any origin, at revision 1) in `operation`.
fn correct(tx: &Transaction<'_>, operation: &str, unit: i64) -> Result<(), AppError> {
    let review = UnitReview {
        revision: 1,
        state: "corrected".into(),
        correction: "corrected".into(),
        actor: "owner".into(),
        rationale: "Checked against the page".into(),
    };
    super::store::review_unit_operation(tx, 1, unit, &review, Some(operation))?;
    Ok(())
}

/// Each case below holds every other fact of a valid row, so exactly one clause of the trigger
/// refuses it; the comment names that clause.
#[test]
fn trigger_refuses_every_broken_fact() {
    let (_dir, store) = fixture(10);
    store
        .upload_storage(|db| {
            let tx = db.transaction()?;
            let tbl = table(&tx, &[2])?;
            let tbl_op: String = tx.query_row(
                "SELECT operation_id FROM source_owner_tables WHERE unit_id=?1",
                [tbl],
                |r| r.get(0),
            )?;
            let (op, unit) = scenario(&tx, 3)?;
            // kind='Text' (merge/split): a Table unit corrected in the operation, anchor
            // rejected in it, so only the unit kind is wrong.
            correct(&tx, &op, tbl)?;
            refused(record_replacement(&tx, tbl, &op, "merge", 3, 1));
            refused(record_replacement(&tx, tbl, &op, "split", 3, 1));
            // kind='Table' (table): a Text unit cannot hold a source_owner_tables link (the
            // 028 trigger refuses it), so the kind clause and the link clause both refuse it.
            refused(record_replacement(&tx, unit, &op, "table", 3, 1));
            // The table link must be this operation's: the table is linked to its own batch
            // while this operation rejected the anchor, so only the link clause refuses it.
            refused(record_replacement(&tx, tbl, &op, "table", 3, 1));
            // page_id equality: operation rejected the anchor on page 2 too, the unit is on page 1.
            let (op_p, unit_p) = scenario(&tx, 5)?;
            reject_on(&tx, 2, &op_p, 11)?;
            refused(record_replacement(&tx, unit_p, &op_p, "merge", 11, 1));
            // origin='owner': an extracted unit corrected in the operation, anchor rejected.
            let (op_x, _) = scenario(&tx, 6)?;
            correct(&tx, &op_x, 4)?;
            refused(record_replacement(&tx, 4, &op_x, "merge", 6, 1));
            // u.id<>anc.id: the unit was corrected in the operation (previous revision 1) and
            // then rejected in it, so it satisfies the anchor and corrected clauses itself.
            let op_s = new_operation_id();
            let same = owner_text(&tx, &op_s, "same")?;
            reject(&tx, &op_s, same)?;
            refused(record_replacement(&tx, same, &op_s, "merge", same, 1));
            // Anchor rejected in this operation: unit 5 was never rejected in `op`.
            refused(record_replacement(&tx, unit, &op, "merge", 5, 1));
            // Same, for an operation that rejected another unit: the unit was corrected in `op2`.
            let (op2, unit2) = scenario(&tx, 7)?;
            refused(record_replacement(&tx, unit2, &op2, "merge", 3, 1));
            // part must be 1 for merge and table; the keys below are right for their part.
            refused(record_replacement(&tx, unit, &op, "merge", 3, 2));
            refused(record_replacement(&tx, tbl, &tbl_op, "table", 2, 2));
            // Column bounds: 0 and 10000 satisfy the trigger's key rule and fail the CHECK.
            for part in [0, 10_000] {
                let error = record_replacement(&tx, unit, &op, "split", 3, part)
                    .unwrap_err()
                    .to_string();
                assert!(error.contains("CHECK"), "{part}: {error}");
            }
            // The unit's corrected event belongs to another operation.
            let stray = owner_text(&tx, &new_operation_id(), "stray")?;
            refused(record_replacement(&tx, stray, &op, "merge", 3, 1));
            refused(record_replacement(&tx, stray, &op, "split", 3, 1));
            // A corrected event that is not the unit's first (previous_revision 2).
            let operation = new_operation_id();
            let later = owner_text(&tx, &new_operation_id(), "first")?;
            let review = UnitReview {
                revision: 2,
                state: "corrected".into(),
                correction: "second".into(),
                actor: "owner".into(),
                rationale: "Checked against the page".into(),
            };
            super::store::review_unit_operation(&tx, 1, later, &review, Some(&operation))?;
            reject(&tx, &operation, 8)?;
            refused(record_replacement(&tx, later, &operation, "merge", 8, 1));
            refused(record_replacement(&tx, later, &operation, "split", 8, 1));
            // Nothing above was written.
            let rows: i64 =
                tx.query_row("SELECT count(*) FROM source_owner_replacements", [], |r| {
                    r.get(0)
                })?;
            assert_eq!(rows, 1, "only the table row exists");
            Ok(())
        })
        .unwrap();
}

#[test]
fn trigger_refuses_duplicates_wrong_keys_updates_and_deletes() {
    let (_dir, store) = fixture(10);
    store
        .upload_storage(|db| {
            let tx = db.transaction()?;
            let (op, unit) = scenario(&tx, 3)?;
            let raw = |unit: i64, part: i64, key: Option<&str>| {
                tx.execute(
                    "INSERT INTO source_owner_replacements(unit_id,operation_id,kind,anchor_unit_id,part,order_key) VALUES(?1,?2,'split',3,?3,?4)",
                    params![unit, op, part, key],
                )
                .map(|_| ())
                .map_err(AppError::from)
            };
            // Wrong order keys: NULL (also refused by NOT NULL), another unit's key, the right key without the part, a bad part.
            refused(raw(unit, 1, None));
            refused(raw(unit, 1, Some("00000000000000000004.0001")));
            refused(raw(unit, 1, Some("00000000000000000003")));
            refused(raw(unit, 1, Some("00000000000000000003.0002")));
            refused(raw(unit, 1, Some("00000000000000000003.1")));
            raw(unit, 1, Some("00000000000000000003.0001"))?;
            // Duplicate (operation, part).
            let twin = owner_text(&tx, &op, "twin")?;
            let duplicate = format!("{:?}", raw(twin, 1, Some("00000000000000000003.0001")).unwrap_err());
            assert!(duplicate.contains("UNIQUE"), "{duplicate}");
            // A second row for the same unit.
            assert!(raw(unit, 2, Some("00000000000000000003.0002")).is_err());
            // Append only.
            for sql in [
                "UPDATE source_owner_replacements SET order_key='x'",
                "UPDATE source_owner_replacements SET part=2",
                "DELETE FROM source_owner_replacements",
            ] {
                let error = tx.execute(sql, []).unwrap_err().to_string();
                assert!(error.contains("append only") || error.contains("retained"), "{sql}: {error}");
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn trigger_backfill_and_writer_spell_the_key_expression_of_the_rust_side() {
    let migration = include_str!("../../migrations/030_owner_replacements.sql");
    let (anc, b) = (unit_sort_key!("anc"), unit_sort_key!("b"));
    // Each alias appears once in the migration: the trigger for `anc`, the backfill for `b`.
    assert_eq!(migration.matches(anc).count(), 1);
    assert_eq!(migration.matches(b).count(), 1);
    assert!(migration.contains(&format!(
        "NEW.order_key IS (SELECT {anc}||'.'||printf('%04d',NEW.part))"
    )));
    assert!(migration.contains(&format!(
        "(SELECT {b}||w.suffix FROM source_units b WHERE b.id=w.cur)"
    )));
    // The writer inserts the same key from the same expression.
    assert_eq!(REPLACEMENT_INSERT.matches(anc).count(), 1);
    assert!(REPLACEMENT_INSERT.contains(&format!("{anc}||'.'||printf('%04d',?5) FROM")));
    // The ordering query sorts by it, then owner before extracted, then id.
    assert!(ORDERED_UNITS.contains(&format!(
        "ORDER BY {},CASE u.origin WHEN 'owner' THEN 0 ELSE 1 END,u.id",
        unit_sort_key!("u")
    )));
}

#[test]
fn table_merge_and_split_units_sort_at_their_anchors() {
    let (_dir, store) = fixture(8);
    store
        .upload_storage(|db| {
            let tx = db.transaction()?;
            // Table of 1 and 2; merge of 3 and 4; split of 6.
            let t = table(&tx, &[1, 2])?;
            let m = merge(&tx, &[3, 4])?;
            let [s1, s2] = split(&tx, 6, 2)?[..] else {
                panic!()
            };
            assert_eq!(order(&tx), [1, t, 2, 3, m, 4, 5, 6, s1, s2, 7, 8]);
            // A split of a split part sorts right after that part, before its sibling.
            let [a, b] = split(&tx, s1, 2)?[..] else {
                panic!()
            };
            assert_eq!(order(&tx), [1, t, 2, 3, m, 4, 5, 6, s1, a, b, s2, 7, 8]);
            assert_eq!(key(&tx, a), "00000000000000000006.0001.0001");
            // A merge of parts of two different splits anchors at the earlier part.
            let [u1, u2] = split(&tx, 7, 2)?[..] else {
                panic!()
            };
            let mm = merge(&tx, &[u1, b])?;
            assert_eq!(
                order(&tx),
                [1, t, 2, 3, m, 4, 5, 6, s1, a, b, mm, s2, 7, u1, u2, 8]
            );
            // A table built from split parts anchors at the first of them in reading order,
            // even when submitted last.
            let tt = table(&tx, &[u2, a])?;
            assert_eq!(
                order(&tx),
                [1, t, 2, 3, m, 4, 5, 6, s1, a, tt, b, mm, s2, 7, u1, u2, 8]
            );
            tx.commit()?;
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_table_submitted_out_of_reading_order_anchors_at_its_first_unit() {
    let (_dir, store) = fixture(5);
    store
        .upload_storage(|db| {
            let tx = db.transaction()?;
            let t = table(&tx, &[4, 2, 3])?;
            let anchor: i64 = tx.query_row(
                "SELECT anchor_unit_id FROM source_owner_replacements WHERE unit_id=?1",
                [t],
                |r| r.get(0),
            )?;
            assert_eq!(anchor, 2);
            assert_eq!(order(&tx), [1, 2, t, 3, 4, 5]);
            // The link stays as before.
            let links: i64 = tx.query_row(
                "SELECT count(*) FROM source_owner_tables WHERE unit_id=?1",
                [t],
                |r| r.get(0),
            )?;
            assert_eq!(links, 1);
            Ok(())
        })
        .unwrap();
}

/// A merge-suggestion owner unit as the application of a `merge` suggestion leaves it: only the
/// rows that `ORDERED_UNITS` reads are written (the application trigger's own checks are
/// 025's and are not under test here).
fn merge_suggestion_unit(
    tx: &Transaction<'_>,
    targets: &[i64],
    text: &str,
) -> Result<i64, AppError> {
    let operation = new_operation_id();
    let unit = owner_text(tx, &operation, text)?;
    let event: i64 = tx.query_row("SELECT max(id) FROM source_review_events", [], |r| r.get(0))?;
    let list: Vec<_> = targets
        .iter()
        .map(|t| serde_json::json!({"unit": t, "revision": 1}))
        .collect();
    let json = serde_json::json!({"type": "units", "targets": list}).to_string();
    tx.execute(
        "INSERT INTO source_suggestions(source_id,bundle_hash,origin,kind,targets,suggested_text,rationale) VALUES(1,?1,'fixture','merge',?2,'merged','fixture')",
        params!["a".repeat(64), json],
    )?;
    let suggestion = tx.last_insert_rowid();
    tx.execute_batch("DROP TRIGGER IF EXISTS source_suggestion_applications_insert")?;
    tx.execute(
        "INSERT INTO source_suggestion_applications(suggestion_id,event_id,operation_id,owner_unit_id,edited) VALUES(?1,?2,?3,?4,0)",
        params![suggestion, event, operation, unit],
    )?;
    for target in targets {
        reject(tx, &operation, *target)?;
    }
    Ok(unit)
}

#[test]
fn merge_suggestion_units_keep_their_position_and_text_keys_everywhere() {
    let (_dir, store) = fixture(6);
    store
        .upload_storage(|db| {
            let tx = db.transaction()?;
            let ms = merge_suggestion_unit(&tx, &[3, 4], "suggested")?;
            // The pre-030 rule: the owner unit precedes its smallest target, owner before
            // extracted at equal keys; units after it keep bundle order.
            assert_eq!(order(&tx), [1, 2, ms, 3, 4, 5, 6]);
            // The key is TEXT in every branch, so one type is compared throughout.
            let types: i64 = tx.query_row(
                &format!(
                    "SELECT count(DISTINCT typeof({})) FROM source_units u",
                    unit_sort_key!("u")
                ),
                [],
                |r| r.get(0),
            )?;
            assert_eq!(types, 1);
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_page_with_every_kind_of_unit_is_ordered_in_full() {
    let (_dir, store) = fixture(8);
    store
        .upload_storage(|db| {
            let tx = db.transaction()?;
            let ms = merge_suggestion_unit(&tx, &[2, 3], "suggested")?;
            // Two missing-text units (no replacement row, no suggestion) go to the end by id.
            let m = owner_text(&tx, &new_operation_id(), "missing text")?;
            let y = owner_text(&tx, &new_operation_id(), "other missing text")?;
            // A split of the merge-suggestion unit lands inline: its parts share the unit's key plus a
            // part suffix, so they follow the unit and its first target.
            let [p1, p2] = split(&tx, ms, 2)?[..] else {
                panic!()
            };
            assert_eq!(order(&tx), [1, ms, 2, p1, p2, 3, 4, 5, 6, 7, 8, m, y]);
            // A split of a missing-text unit sorts right after it, before the next one.
            let [q1, q2] = split(&tx, m, 2)?[..] else {
                panic!()
            };
            assert_eq!(key(&tx, q1), format!("~{m:020}.0001"));
            let split_end = order(&tx);
            assert_eq!(&split_end[split_end.len() - 4..], [m, q1, q2, y]);
            // And the extracted units around them are unchanged.
            let t = table(&tx, &[5, 6])?;
            assert_eq!(&order(&tx)[..11], [1, ms, 2, p1, p2, 3, 4, 5, t, 6, 7]);
            Ok(())
        })
        .unwrap();
}

#[test]
fn migration_backfills_existing_owner_tables_in_order() {
    let (dir, store) = fixture(7);
    let expected = store
        .upload_storage(|db| {
            let tx = db.transaction()?;
            let first = table(&tx, &[2, 3])?;
            // A table built from an earlier table, submitted with that table first.
            let second = table(&tx, &[first, 5])?;
            // A table whose only unit is a missing-text root sorts after it, at the end.
            let missing = owner_text(&tx, &new_operation_id(), "missing text")?;
            let third = table(&tx, &[missing])?;
            // A table whose first rejected unit is a merge-suggestion owner unit.
            let suggested = merge_suggestion_unit(&tx, &[6, 7], "suggested")?;
            let fourth = table(&tx, &[suggested])?;
            let order = order(&tx);
            assert_eq!(
                order,
                [
                    1, 2, first, second, 3, 4, 5, suggested, 6, fourth, 7, missing, third
                ]
            );
            tx.commit()?;
            // Rewind to a version 29 database: no replacement rows exist yet.
            db.execute_batch("DROP TABLE source_owner_replacements; PRAGMA user_version=29;")?;
            Ok((order, [first, second, third, fourth], missing))
        })
        .unwrap();
    drop(store);
    let upgraded = Store::open(&dir.path().join("db")).unwrap();
    upgraded
        .upload_storage(|db| {
            assert_eq!(
                db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
                crate::SCHEMA_VERSION
            );
            assert_eq!(order(db), expected.0);
            let [first, second, third, fourth] = expected.1;
            assert_eq!(key(db, first), "00000000000000000002.0001");
            assert_eq!(key(db, second), "00000000000000000002.0001.0001");
            assert_eq!(key(db, third), format!("~{:020}.0001", expected.2));
            assert_eq!(key(db, fourth), "00000000000000000006.0001");
            let kinds: i64 = db.query_row(
                "SELECT count(*) FROM source_owner_replacements WHERE kind='table' AND part=1",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(kinds, 4);
            Ok(())
        })
        .unwrap();
}
