//! The typed note value (assisted sheet plan §3a): canonical, bounded, exact round trip.
use super::{NoteValue, Op};
use crate::definitions::Control;
use serde_json::json;

#[test]
fn values_round_trip_exactly() {
    let enum_id = "a".repeat(64);
    let cases = [
        (NoteValue::Absent, r#"{"absent":true}"#.to_owned()),
        (NoteValue::Unknown, r#"{"unknown":true}"#.to_owned()),
        (NoteValue::Int(-15), r#"{"int":"-15"}"#.to_owned()),
        (NoteValue::Int(0), r#"{"int":"0"}"#.to_owned()),
        (
            NoteValue::Int(i64::MIN),
            r#"{"int":"-9223372036854775808"}"#.to_owned(),
        ),
        (
            NoteValue::Int(i64::MAX),
            r#"{"int":"9223372036854775807"}"#.to_owned(),
        ),
        (
            NoteValue::Enum(enum_id.clone()),
            format!(r#"{{"enum":"{enum_id}"}}"#),
        ),
        (NoteValue::Bool(true), r#"{"bool":true}"#.to_owned()),
        (NoteValue::Bool(false), r#"{"bool":false}"#.to_owned()),
        (NoteValue::Row(true), r#"{"row":"present"}"#.to_owned()),
        (NoteValue::Row(false), r#"{"row":"absent"}"#.to_owned()),
    ];
    for (value, text) in cases {
        let encoded = value.encode().unwrap();
        assert_eq!(encoded, text);
        assert!(encoded.len() <= super::MAX_VALUE_BYTES, "{encoded}");
        assert_eq!(NoteValue::decode(&encoded), Some(value), "{encoded}");
    }
    // Absence and unknown stay distinct.
    assert_ne!(
        NoteValue::decode(r#"{"absent":true}"#),
        NoteValue::decode(r#"{"unknown":true}"#)
    );
}

#[test]
fn non_canonical_or_oversized_values_are_never_read() {
    for text in [
        r#"{"int":"007"}"#,
        r#"{"int":"+5"}"#,
        r#"{"int":-15}"#,
        r#"{"int":"9223372036854775808"}"#,
        r#"{"absent":false}"#,
        r#"{"unknown":null}"#,
        r#"{"bool":"true"}"#,
        r#"{"row":"gone"}"#,
        r#"{"int":"1","bool":true}"#,
        r#"{ "int":"1"}"#,
        r"{}",
        r"[]",
        r#""int""#,
        "not json",
    ] {
        assert_eq!(NoteValue::decode(text), None, "{text}");
    }
    let long = NoteValue::Enum("a".repeat(70));
    assert!(long.encode().is_err(), "over 80 bytes is refused");
    assert_eq!(
        NoteValue::decode(&format!(r#"{{"enum":"{}"}}"#, "a".repeat(70))),
        None
    );
}

#[test]
fn stored_cells_map_to_typed_values() {
    assert_eq!(
        NoteValue::of(Control::Integer, &json!(84)),
        Some(NoteValue::Int(84))
    );
    assert_eq!(
        NoteValue::of(Control::Integer, &json!(null)),
        Some(NoteValue::Unknown)
    );
    assert_eq!(
        NoteValue::of(Control::Enumeration, &json!("guard")),
        Some(NoteValue::Enum("guard".into()))
    );
    assert_eq!(
        NoteValue::of(Control::Boolean, &json!(false)),
        Some(NoteValue::Bool(false))
    );
    assert_eq!(
        NoteValue::of(Control::Text, &json!("x")),
        None,
        "text is out of scope"
    );
    assert_eq!(NoteValue::of(Control::Integer, &json!("84")), None);
}

#[test]
fn ops_round_trip() {
    for op in [
        Op::Literal,
        Op::Sum,
        Op::SignLiteral,
        Op::Switched,
        Op::Unresolved,
        Op::Erased,
        Op::Confirmed,
        Op::Reverted,
        Op::Reminded,
    ] {
        assert_eq!(Op::parse(op.as_str()), Some(op));
    }
    assert_eq!(Op::parse("Literal"), None);
}

#[test]
fn addresses_are_typed() {
    assert_eq!(super::field_address("name"), "field:name");
    assert_eq!(
        super::cell_address("equipment", "r1-1", "quantity"),
        "cell:equipment/r1-1/quantity"
    );
}

/// Migration 034: the bundle lookups use the `(sheet_id, bundle)` index, so a bundle's members
/// and the next bundle number need no scan of the sheet's notes.
#[tokio::test]
async fn bundle_lookups_use_the_bundle_index() {
    let dir = tempfile::tempdir().unwrap();
    let store = crate::Store::open(&dir.path().join("plan.sqlite3")).unwrap();
    let plans = store
        .run(|db| {
            let plan =
                |sql: &str, params: &[&dyn rusqlite::ToSql]| -> Result<String, crate::AppError> {
                    let mut query = db.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
                    let rows = query
                        .query_map(params, |row| row.get::<_, String>(3))?
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(rows.join("\n"))
                };
            Ok((
                plan(super::BUNDLE_MEMBERS, &[&1_i64, &"[1,2]", &9_i64])?,
                plan(super::NEXT_BUNDLE, &[&1_i64])?,
            ))
        })
        .await
        .unwrap();
    for (name, plan) in [("members", &plans.0), ("next", &plans.1)] {
        assert!(
            plan.contains("private_sheet_notes_bundle"),
            "{name}: {plan}"
        );
        assert!(!plan.contains("SCAN private_sheet_notes"), "{name}: {plan}");
    }
}

/// Migration 034 admits the op `reminded` (and no other new one) in the note table.
#[tokio::test]
async fn the_note_table_admits_the_reminded_op() {
    let dir = tempfile::tempdir().unwrap();
    let store = crate::Store::open(&dir.path().join("ops.sqlite3")).unwrap();
    let checked = store
        .run(|db| {
            let sql: String = db.query_row(
                "SELECT sql FROM sqlite_master WHERE name='private_sheet_notes' AND type='table'",
                [],
                |row| row.get(0),
            )?;
            Ok(sql)
        })
        .await
        .unwrap();
    assert!(checked.contains("'reminded'"), "{checked}");
    assert!(!checked.contains("'bogus'"));
}
