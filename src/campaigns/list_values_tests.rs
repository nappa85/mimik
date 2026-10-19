//! M4 step 3 (plan test 12): list rows from an admitted form, reconciled against the saved
//! list (`values::form`, `values::reconcile`).
use super::list_tests::list_definition;
use crate::AppError;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn form(rows: &[(&str, &str, &str, Option<&str>, &str)]) -> BTreeMap<String, String> {
    let mut form: BTreeMap<String, String> = [("value-label", "x")]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    for (r, (id, name, quantity, armor, armor_state)) in rows.iter().enumerate() {
        for (key, value) in [
            (format!("id-1-{r}"), *id),
            (format!("row-1-{r}-0"), *name),
            (format!("row-1-{r}-1"), *quantity),
            (format!("row-1-{r}-2"), armor.unwrap_or_default()),
            (format!("set-1-{r}-2"), *armor_state),
            (format!("row-1-{r}-3"), ""),
            (format!("set-1-{r}-3"), "absent"),
            (format!("row-1-{r}-4"), "false"),
        ] {
            form.insert(key, value.to_owned());
        }
    }
    form
}

fn values(
    form: &BTreeMap<String, String>,
    saved: &Value,
    revision: i64,
) -> Result<Value, AppError> {
    let pin = crate::definitions::private_pin::for_tests(&list_definition());
    // Revision 0 is a create: there is no saved sheet, so every row is new.
    let saved = (revision > 0).then_some(saved);
    super::values::form(pin.definition(), form, saved, revision, &BTreeMap::new())
        .map(|(data, _notes)| data)
}

fn saved() -> Value {
    json!({"label": "x", "tags": [], "equipment": [
        {"id": "r1-1", "name": "Old", "quantity": 1, "equipped": false}]})
}

fn invalid(result: Result<Value, AppError>, context: &str) {
    match result {
        Err(AppError::Invalid(_)) => {}
        other => panic!("{context}: want Invalid, got {other:?}"),
    }
}

#[test]
fn list_rows_parse_in_order_new_rows_get_host_ids() {
    let rows = [
        ("", "First new", "2", None, "absent"),
        ("r1-1", "Kept", "3", Some("4"), "present"),
        ("", "Second new", "-1", None, "absent"),
    ];
    let value = values(&form(&rows), &saved(), 1).unwrap();
    assert_eq!(
        value["equipment"],
        json!([
            {"id": "r2-1", "name": "First new", "quantity": 2, "equipped": false},
            {"id": "r1-1", "name": "Kept", "quantity": 3, "armor": 4, "equipped": false},
            {"id": "r2-2", "name": "Second new", "quantity": -1, "equipped": false},
        ])
    );
    assert_eq!(value["tags"], json!([]));
    // On create (revision 0) new IDs are r1-k.
    let created = values(&form(&rows[..1]), &json!({}), 0).unwrap();
    assert_eq!(created["equipment"][0]["id"], "r1-1");
}

#[test]
fn optional_state_is_explicit() {
    let present = values(&form(&[("", "N", "1", Some("7"), "present")]), &saved(), 1).unwrap();
    assert_eq!(present["equipment"][0]["armor"], 7);
    let absent = values(&form(&[("", "N", "1", Some("7"), "absent")]), &saved(), 1).unwrap();
    assert!(
        absent["equipment"][0].get("armor").is_none(),
        "Not set drops the value"
    );
    for state in ["", "known", "PRESENT"] {
        invalid(
            values(&form(&[("", "N", "1", Some("7"), state)]), &saved(), 1),
            state,
        );
    }
    // Present but not a number: unknown since slice 1.4.
    let unknown = values(&form(&[("", "N", "1", Some("x"), "present")]), &saved(), 1).unwrap();
    assert_eq!(unknown["equipment"][0]["armor"], Value::Null);
}

#[test]
fn item_integers_exact_i64() {
    // Row integers are player entries (assisted sheet slice 1.1): zero padding, a plus sign,
    // surrounding spaces and sums are read. Since slice 1.4 anything else in a new row is
    // unknown (`null`, with a scribble note); control characters are still refused.
    for raw in ["1.0", "", "9223372036854775808", "1 2", "84--15", "x"] {
        let read = values(&form(&[("", "N", raw, None, "absent")]), &saved(), 1).unwrap();
        assert_eq!(read["equipment"][0]["quantity"], Value::Null, "{raw:?}");
    }
    invalid(
        values(&form(&[("", "N", "1\u{7}", None, "absent")]), &saved(), 1),
        "control character",
    );
    for (raw, want) in [("01", 1), ("+1", 1), (" 1", 1), ("2+1", 3), ("84 -15", 69)] {
        let read = values(&form(&[("", "N", raw, None, "absent")]), &saved(), 1).unwrap();
        assert_eq!(read["equipment"][0]["quantity"], want, "{raw:?}");
    }
    let edge = values(
        &form(&[("", "N", "-9223372036854775808", None, "absent")]),
        &saved(),
        1,
    )
    .unwrap();
    assert_eq!(edge["equipment"][0]["quantity"], i64::MIN);
}

#[test]
fn unknown_or_duplicate_ids_refused() {
    invalid(
        values(&form(&[("r9-9", "N", "1", None, "absent")]), &saved(), 1),
        "unknown",
    );
    invalid(
        values(
            &form(&[
                ("r1-1", "N", "1", None, "absent"),
                ("r1-1", "M", "1", None, "absent"),
            ]),
            &saved(),
            1,
        ),
        "duplicate",
    );
}

#[test]
fn rows_over_256_refused() {
    let pin = crate::definitions::private_pin::for_tests(&list_definition());
    let field = &pin.definition().fields[2];
    let row = super::values::DraftRow {
        id: String::new(),
        values: vec!["t".into()],
        states: vec![String::new()],
    };
    let current = BTreeMap::new();
    let mut recorder = super::values::Recorder::new(&current);
    let full = super::values::ListDraft {
        rows: vec![row.clone(); 256],
    };
    assert_eq!(
        super::values::reconcile(field, &full, &json!([]), 1, &mut recorder)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        256
    );
    let over = super::values::ListDraft {
        rows: vec![row; 257],
    };
    invalid(
        super::values::reconcile(field, &over, &json!([]), 1, &mut recorder),
        "257 rows",
    );
}
