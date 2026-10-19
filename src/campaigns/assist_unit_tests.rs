//! Slice 1.3 helpers: note keys, the D-Q switch value (checked), Undo eligibility (D-N,
//! D-X1), and reading or writing one typed address of a stored snapshot.
use super::{key, locate, parse_key, set, switch_value, undoable, value_at};
use crate::campaigns::notes::{Note, NoteValue, Op, Stored};
use crate::definitions::Definition;
use serde_json::{Value, json};

fn stored(op: Op, raw: &str, before: Option<NoteValue>, after: NoteValue) -> Stored {
    Stored {
        revision: 4,
        note: Note {
            address: "field:hp".into(),
            op,
            raw: raw.into(),
            before,
            after,
            target: None,
        },
    }
}

fn sign(raw: &str, before: i64, after: i64) -> Stored {
    stored(
        Op::SignLiteral,
        raw,
        Some(NoteValue::Int(before)),
        NoteValue::Int(after),
    )
}

#[test]
fn keys_round_trip_and_refuse_malformed_text() {
    let note = sign("-15", 84, -15);
    assert_eq!(key(&note), "4:field:hp");
    assert_eq!(parse_key("4:field:hp"), Some((4, "field:hp")));
    assert_eq!(
        parse_key("12:cell:equipment/r1-1/quantity"),
        Some((12, "cell:equipment/r1-1/quantity"))
    );
    for raw in [
        "",
        "4",
        "4:",
        ":field:hp",
        "0:field:hp",
        "-4:field:hp",
        "04:field:hp",
        "+4:field:hp",
        "x:field:hp",
        "4:hp",
        "4:field:",
        "9223372036854775808:field:hp",
    ] {
        assert_eq!(parse_key(raw), None, "{raw:?}");
    }
    let long = format!("4:field:{}", "a".repeat(251));
    assert_eq!(parse_key(&long), None, "address above 256 bytes");
}

#[test]
fn the_switch_adds_the_literal_to_the_value_before() {
    let int = |value| Some(NoteValue::Int(value));
    assert_eq!(
        switch_value(&sign("-15", 84, -15), int(-15).as_ref()),
        Some(69)
    );
    assert_eq!(switch_value(&sign("+3", 84, 3), int(3).as_ref()), Some(87));
    assert_eq!(
        switch_value(&sign("-5+3", 84, -2), int(-2).as_ref()),
        Some(82)
    );
    assert_eq!(
        switch_value(&sign("-1", -1, -1), int(-1).as_ref()),
        Some(-2)
    );
    // Checked arithmetic: no offer when b + n leaves i64.
    assert_eq!(
        switch_value(&sign("-1", i64::MIN, -1), int(-1).as_ref()),
        None
    );
    assert_eq!(
        switch_value(&sign("+1", i64::MAX, 1), int(1).as_ref()),
        None
    );
    assert_eq!(
        switch_value(&sign("-1", i64::MIN + 1, -1), int(-1).as_ref()),
        Some(i64::MIN)
    );
    // Over 0 the relative reading is the literal itself: nothing to switch.
    assert_eq!(switch_value(&sign("-7", 0, -7), int(-7).as_ref()), None);
    // Bound to the displayed state: the cell must still hold the literal.
    assert_eq!(switch_value(&sign("-15", 84, -15), int(-14).as_ref()), None);
    assert_eq!(switch_value(&sign("-15", 84, -15), None), None);
    // Only a sign-led literal over a known number, whose raw text spells its value.
    for note in [
        stored(Op::Literal, "-15", int(84), NoteValue::Int(-15)),
        stored(Op::Sum, "-15", int(84), NoteValue::Int(-15)),
        stored(Op::Switched, "-15", int(84), NoteValue::Int(-15)),
        stored(Op::SignLiteral, "-15", None, NoteValue::Int(-15)),
        stored(
            Op::SignLiteral,
            "-15",
            Some(NoteValue::Unknown),
            NoteValue::Int(-15),
        ),
        stored(
            Op::SignLiteral,
            "-15",
            Some(NoteValue::Absent),
            NoteValue::Int(-15),
        ),
        sign("-16", 84, -15),
        sign("15", 84, 15),
        sign("lots", 84, -15),
    ] {
        assert_eq!(
            switch_value(&note, Some(&note.note.after)),
            None,
            "{note:?}"
        );
    }
}

#[test]
fn undo_needs_the_current_change_with_a_value_before() {
    let int = NoteValue::Int;
    for op in [
        Op::Literal,
        Op::Sum,
        Op::SignLiteral,
        Op::Switched,
        Op::Confirmed,
        Op::Reverted,
        Op::Erased,
    ] {
        let note = stored(op, "", Some(int(84)), int(69));
        assert!(undoable(&note, Some(&int(69))), "{op:?}");
        assert!(
            !undoable(&note, Some(&int(70))),
            "{op:?} after a newer value"
        );
        assert!(!undoable(&note, None), "{op:?} without a value");
    }
    // D-X1: an erased (unknown) value is undoable; so is present ↔ absent.
    let erased = stored(Op::Erased, "", Some(int(84)), NoteValue::Unknown);
    assert!(undoable(&erased, Some(&NoteValue::Unknown)));
    let absent = stored(
        Op::Literal,
        "",
        Some(NoteValue::Bool(true)),
        NoteValue::Absent,
    );
    assert!(undoable(&absent, Some(&NoteValue::Absent)));
    let present = stored(
        Op::Literal,
        "",
        Some(NoteValue::Absent),
        NoteValue::Bool(true),
    );
    assert!(undoable(&present, Some(&NoteValue::Bool(true))));
    // Not eligible: a scribble, a new cell (no before), a data-unchanged note, a row note.
    for note in [
        stored(Op::Unresolved, "lots", Some(int(84)), int(84)),
        stored(Op::Sum, "80+4", None, int(84)),
        stored(Op::Literal, "84", Some(int(84)), int(84)),
        stored(
            Op::Literal,
            "",
            Some(NoteValue::Row(true)),
            NoteValue::Row(false),
        ),
    ] {
        assert!(!undoable(&note, Some(&note.note.after)), "{note:?}");
    }
}

fn definition() -> Definition {
    serde_json::from_value(crate::definitions::source_fixture::assist_definition()).unwrap()
}

fn data() -> Value {
    json!({"hp": 84, "max_hp": 84, "armour": -1, "luck": null,
           "equipment": [{"id": "r1-1", "name": "Rope", "quantity": 2, "weight": 1},
                         {"id": "r2-1", "name": "Lamp", "quantity": 1, "weight": 2,
                          "broken": true}]})
}

#[test]
fn addresses_are_located_read_and_written() {
    let definition = definition();
    let mut data = data();
    let hp = locate(&definition, &data, "field:hp").unwrap();
    assert_eq!(hp.label, "Current HP");
    assert_eq!(value_at(&hp, &data), Some(NoteValue::Int(84)));
    let luck = locate(&definition, &data, "field:luck").unwrap();
    assert_eq!(value_at(&luck, &data), Some(NoteValue::Unknown));
    let quantity = locate(&definition, &data, "cell:equipment/r2-1/quantity").unwrap();
    assert_eq!(quantity.label, "Quantity, row 2 of Equipment");
    assert_eq!(value_at(&quantity, &data), Some(NoteValue::Int(1)));
    let broken = locate(&definition, &data, "cell:equipment/r1-1/broken").unwrap();
    assert_eq!(value_at(&broken, &data), Some(NoteValue::Absent));
    // Text cells, derived fields, unknown rows and malformed addresses are not located.
    for address in [
        "field:load",
        "field:equipment",
        "field:missing",
        "cell:equipment/r1-1/name",
        "cell:equipment/r9-9/quantity",
        "cell:equipment/r1-1",
        "cell:equipment/r1-1/quantity/x",
        "cell:hp/r1-1/quantity",
        "meta:name",
        "hp",
    ] {
        assert!(locate(&definition, &data, address).is_none(), "{address}");
    }
    set(&hp, &mut data, &NoteValue::Int(69)).unwrap();
    set(&luck, &mut data, &NoteValue::Int(3)).unwrap();
    set(&quantity, &mut data, &NoteValue::Int(-2)).unwrap();
    set(&broken, &mut data, &NoteValue::Bool(false)).unwrap();
    assert_eq!(data["hp"], json!(69));
    assert_eq!(data["luck"], json!(3));
    assert_eq!(data["equipment"][1]["quantity"], json!(-2));
    assert_eq!(data["equipment"][0]["broken"], json!(false));
    let broken = locate(&definition, &data, "cell:equipment/r1-1/broken").unwrap();
    set(&broken, &mut data, &NoteValue::Absent).unwrap();
    assert!(data["equipment"][0].get("broken").is_none());
    let luck = locate(&definition, &data, "field:luck").unwrap();
    set(&luck, &mut data, &NoteValue::Unknown).unwrap();
    assert_eq!(data["luck"], Value::Null);
    // A required cell is never made absent, and a row state is not a cell value.
    assert!(set(&hp, &mut data, &NoteValue::Absent).is_err());
    assert!(set(&quantity, &mut data, &NoteValue::Absent).is_err());
    assert!(set(&hp, &mut data, &NoteValue::Row(true)).is_err());
    assert_eq!(data["hp"], json!(69));
}
