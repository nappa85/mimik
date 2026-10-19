//! Slice 1.4 "Unknown values" (assisted sheet plan §3a, §3f, §5 slice 1.4; decisions D-M and
//! D-X1): `null` is the unknown value of every integer cell. An unreadable number in a new
//! row, a newly present property or on create is stored as unknown with its scribble; a
//! blank number is erased to unknown and can be undone; formulas that read an unknown are
//! not computed, and the others still are.
use super::assist_tests::{click, edit, text};
use super::formula_tests::{body, setup};
use super::notes_tests::{
    assist, choices, create_path, created, form, history_len, inner, note, notes_at, page,
    player_sheet, row, saved, value,
};
use crate::AppError;
use axum::http::StatusCode;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const UNKNOWN: &str = r#"{"unknown":true}"#;

/// What the sheet shows under a cell whose scribble left it unknown (plan §3a: "?" plus the
/// scribble).
fn unknown_scribble(raw: &str) -> String {
    format!("? (unknown) · ‘<bdi>{raw}</bdi>’ not understood")
}

/// The `placeholder` of the control named `name`.
fn placeholder(html: &str, name: &str) -> Option<String> {
    let page = scraper::Html::parse_document(html);
    page.select(&scraper::Selector::parse(&format!("[name=\"{name}\"]")).unwrap())
        .next()
        .and_then(|element| element.value().attr("placeholder").map(str::to_owned))
}

/// The text of the derived value Load on a sheet or history page.
fn load(html: &str) -> String {
    text(html, "dl.derived dd").unwrap_or_default()
}

const LOAD_UNKNOWN: &str = "Not computed: a value it uses is unknown (null). null_value";

// ---- Pure: values::form, the schema and the pin's domain checks ----

/// `list_definition()` (equipment is field 1: name, quantity, optional armor, optional
/// kind, equipped) with one form row per `(id, quantity, armor)`; armor `None` is Not set.
fn list_form(rows: &[(&str, &str, Option<&str>)]) -> BTreeMap<String, String> {
    let mut form = BTreeMap::from([("value-label".to_owned(), "x".to_owned())]);
    for (r, (id, quantity, armor)) in rows.iter().enumerate() {
        for (key, value) in [
            (format!("id-1-{r}"), *id),
            (format!("row-1-{r}-0"), "Rope"),
            (format!("row-1-{r}-1"), *quantity),
            (format!("row-1-{r}-2"), armor.unwrap_or_default()),
            (
                format!("set-1-{r}-2"),
                if armor.is_some() { "present" } else { "absent" },
            ),
            (format!("row-1-{r}-3"), ""),
            (format!("set-1-{r}-3"), "absent"),
            (format!("row-1-{r}-4"), "false"),
        ] {
            form.insert(key, value.to_owned());
        }
    }
    form
}

fn list_saved() -> Value {
    json!({"label": "x", "tags": [], "equipment": [
        {"id": "r1-1", "name": "Rope", "quantity": 3, "equipped": false},
        {"id": "r1-2", "name": "Rope", "quantity": 1, "armor": 4, "equipped": false},
        {"id": "r1-3", "name": "Rope", "quantity": 1, "equipped": false}]})
}

type Saved = (
    Value,
    Vec<(String, &'static str, String, Option<String>, String)>,
);

/// The data and notes (address, op, raw, before, after) of a list-definition save.
fn list_save(form: &BTreeMap<String, String>, saved: Option<&Value>) -> Result<Saved, AppError> {
    let pin = crate::definitions::private_pin::for_tests(&super::list_tests::list_definition());
    let (data, notes) =
        super::super::values::form(pin.definition(), form, saved, 1, &BTreeMap::new())?;
    pin.validate_values(&data)?;
    let notes = notes
        .into_iter()
        .map(|note| {
            (
                note.address,
                note.op.as_str(),
                note.raw,
                note.before.map(|before| before.encode().unwrap()),
                note.after.encode().unwrap(),
            )
        })
        .collect();
    Ok((data, notes))
}

/// D-M: an unreadable number in a new row, a newly present property or on create is stored
/// as unknown with an `unresolved` note holding the scribble. A blank one is unknown with no
/// note on a new cell. A known saved value is still kept under a scribble (slice 1.2).
#[test]
fn unreadable_numbers_in_new_cells_are_unknown_with_a_note() {
    let saved = list_saved();
    let form = list_form(&[
        ("r1-1", "lots", Some("many")),
        ("r1-2", "2", Some("4")),
        ("", "lots", None),
        ("", "", Some("")),
    ]);
    let (data, notes) = list_save(&form, Some(&saved)).unwrap();
    let rows = &data["equipment"];
    assert_eq!(rows[0]["quantity"], json!(3), "kept");
    assert_eq!(rows[0]["armor"], Value::Null, "newly present: unknown");
    assert_eq!(rows[2]["quantity"], Value::Null);
    assert_eq!(
        (&rows[3]["quantity"], &rows[3]["armor"]),
        (&Value::Null, &Value::Null)
    );
    let unresolved = |address: &str, raw: &str, before: Option<&str>, after: &str| {
        (
            address.to_owned(),
            "unresolved",
            raw.to_owned(),
            before.map(str::to_owned),
            after.to_owned(),
        )
    };
    assert_eq!(
        notes,
        vec![
            unresolved(
                "cell:equipment/r1-1/quantity",
                "lots",
                Some(r#"{"int":"3"}"#),
                r#"{"int":"3"}"#
            ),
            unresolved(
                "cell:equipment/r1-1/armor",
                "many",
                Some(r#"{"absent":true}"#),
                UNKNOWN
            ),
            (
                "cell:equipment/r1-2/quantity".into(),
                "literal",
                "2".into(),
                Some(r#"{"int":"1"}"#.into()),
                r#"{"int":"2"}"#.into()
            ),
            // Blank in a new cell (r2-2): unknown with no note.
            unresolved("cell:equipment/r2-1/quantity", "lots", None, UNKNOWN),
        ]
    );
    // Create: every cell is new.
    let (data, notes) = list_save(&list_form(&[("", "lots", None)]), None).unwrap();
    assert_eq!(data["equipment"][0]["quantity"], Value::Null);
    assert_eq!(
        notes,
        vec![unresolved(
            "cell:equipment/r2-1/quantity",
            "lots",
            None,
            UNKNOWN
        )]
    );
}

/// D-M and D-X1: a blank number erases a known value to unknown with an `erased` note; a
/// blank over unknown changes nothing. Over-long and control-character input is still
/// refused everywhere (it cannot be kept as a faithful scribble).
#[test]
fn blank_erases_a_known_number_and_unkeepable_input_is_refused() {
    let saved = list_saved();
    let form = list_form(&[
        ("r1-1", " \t", None),
        ("r1-2", "1", Some("")),
        ("r1-3", "1", Some("")),
    ]);
    let (data, notes) = list_save(&form, Some(&saved)).unwrap();
    assert_eq!(data["equipment"][0]["quantity"], Value::Null);
    assert_eq!(data["equipment"][1]["armor"], Value::Null);
    assert_eq!(data["equipment"][2]["armor"], Value::Null);
    // The note keeps what was typed, spaces included.
    let erased = |address: &str, raw: &str, before: &str| {
        (
            address.to_owned(),
            "erased",
            raw.to_owned(),
            Some(before.to_owned()),
            UNKNOWN.to_owned(),
        )
    };
    assert_eq!(
        notes,
        vec![
            erased("cell:equipment/r1-1/quantity", " \t", r#"{"int":"3"}"#),
            erased("cell:equipment/r1-2/armor", "", r#"{"int":"4"}"#),
            // A newly present property left blank: from not set to unknown.
            erased("cell:equipment/r1-3/armor", "", r#"{"absent":true}"#),
        ]
    );
    let (_, notes) = list_save(&list_form(&[("r1-1", "", None)]), Some(&data)).unwrap();
    assert!(notes.is_empty(), "blank over unknown: {notes:?}");
    let long = format!("1{}", " ".repeat(200));
    for raw in ["84\u{7}", "8\u{0}4", long.as_str()] {
        for (rows, saved) in [
            (vec![("", raw, None)], None),
            (vec![("r1-1", "3", None), ("", raw, None)], Some(&saved)),
            (vec![("r1-1", raw, None)], Some(&saved)),
        ] {
            assert!(
                matches!(
                    list_save(&list_form(&rows), saved),
                    Err(AppError::Invalid(_))
                ),
                "{raw:?} {rows:?}"
            );
        }
    }
}

/// Plan §3a (Sol M5): every integer cell admits `null`, list properties included, and the
/// nullable wrapper is idempotent: an existing nullable integer definition still compiles
/// its schema, with no nested type array. Other types still refuse `null`.
#[test]
fn the_value_schema_admits_null_for_every_integer_cell() {
    let pin = crate::definitions::private_pin::for_tests(&assist());
    let schema = super::super::values::schema_json(pin.definition()).unwrap();
    assert_eq!(
        schema["properties"]["luck"]["type"],
        json!(["integer", "null"]),
        "nullable: not nested"
    );
    assert_eq!(
        schema["properties"]["hp"]["type"],
        json!(["integer", "null"])
    );
    assert_eq!(
        schema["properties"]["equipment"]["items"]["properties"]["quantity"]["type"],
        json!(["integer", "null"])
    );
    let data = json!({"hp": null, "max_hp": 84, "armour": -1, "luck": null, "equipment": [
        {"id": "r1-1", "name": "Rope", "quantity": null, "weight": null}]});
    pin.validate_values(&data).unwrap();
    // A text or boolean property, and integers above i64, are still refused.
    for (property, refused) in [
        ("name", Value::Null),
        ("broken", Value::Null),
        ("quantity", json!(u64::MAX)),
    ] {
        let mut data = data.clone();
        data["equipment"][0][property] = refused.clone();
        assert!(
            pin.validate_values(&data).is_err(),
            "{property} = {refused}"
        );
    }
    let mut data = data;
    data["hp"] = json!(u64::MAX);
    assert!(pin.validate_values(&data).is_err(), "hp above i64");
}

// ---- Through the router ----

/// The assist definition plus a check on Load, so a dependent of an unknown is shown.
fn with_load_check() -> Value {
    let mut definition = assist();
    definition["constraints"].as_array_mut().unwrap().push(
        json!({"id": "load-cap", "expr": "load <= 10", "rule_ids": ["synthetic-3"],
                     "message": "Load is at most 10.",
                     "evidence": {"state": "unresolved", "reason": "Synthetic fixture"}}),
    );
    definition
}

/// Visible behaviour: "lots" in a new row's Quantity keeps the row, shown as "?" with the
/// scribble; Load says "Not computed: a value it uses is unknown (null)"; its dependent
/// check says "it uses Load, which could not be computed"; the other check still runs.
/// Re-posting the page writes nothing; a number then replaces the unknown, and Undo
/// restores it.
#[tokio::test]
async fn lots_in_a_new_row_is_unknown_with_its_scribble() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&with_load_check()).await;
    created(&setup, &[]).await;
    edit(&setup, 1, &[], &[row("r1-1", "1"), row("", "lots")]).await;
    let (revision, data) = saved(&setup, 1).await;
    assert_eq!(revision, 2);
    assert_eq!(data["equipment"][1]["id"], json!("r2-1"));
    assert_eq!(data["equipment"][1]["quantity"], Value::Null);
    assert_eq!(
        notes_at(&setup, 1, 2).await,
        vec![note(
            2,
            "cell:equipment/r2-1/quantity",
            "unresolved",
            "lots",
            None,
            UNKNOWN
        )]
    );
    let html = page(&setup).await;
    assert_eq!(value(&html, "row-4-1-1").as_deref(), Some("lots"));
    assert_eq!(
        inner(&html, "#note-4-1-1").as_deref(),
        Some(unknown_scribble("lots").as_str())
    );
    assert_eq!(load(&html), LOAD_UNKNOWN);
    assert!(
        html.contains("Load is at most 10. Reason: it uses Load, which could not be computed."),
        "dependent"
    );
    assert!(
        html.contains("0 violated, 1 not evaluated, 1 passed."),
        "hp-cap still runs"
    );
    // Re-posting the page (the scribble echoed in the control) writes nothing (D-U).
    let (status, html) = setup
        .post(
            "/private-sheets/1",
            form(2, &[], &[row("r1-1", "1"), row("r2-1", "lots")]),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains("No change"));
    assert_eq!(history_len(&setup, 1).await, 2);
    // A number replaces the unknown; Undo restores it (plan §3f).
    edit(&setup, 2, &[], &[row("r1-1", "1"), row("r2-1", "2")]).await;
    let html = page(&setup).await;
    assert_eq!(load(&html), "3");
    assert_eq!(
        text(&html, "#was-4-1-1").as_deref(),
        Some("Last change: was unknown")
    );
    let (status, _) = click(&setup, &html, "undo-4-1-1").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        saved(&setup, 1).await.1["equipment"][1]["quantity"],
        Value::Null
    );
    assert_eq!(load(&page(&setup).await), LOAD_UNKNOWN);
}

/// D-M on create: "lots" creates the sheet with unknown values and notes without a value
/// before; the cell shows "?" with the scribble and the check that reads it is not
/// evaluated.
#[tokio::test]
async fn create_with_lots_is_created_with_unknown() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    let create = form(0, &[("value-hp", "lots")], &[row("", "lots"), row("", "")]);
    let (status, html) = setup.post(&create_path(&setup), create).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
    let (revision, data) = saved(&setup, 1).await;
    assert_eq!(revision, 1);
    assert_eq!(data["hp"], Value::Null);
    assert_eq!(data["equipment"][0]["quantity"], Value::Null);
    assert_eq!(data["equipment"][1]["quantity"], Value::Null);
    assert_eq!(
        notes_at(&setup, 1, 1).await,
        vec![
            note(
                1,
                "cell:equipment/r1-1/quantity",
                "unresolved",
                "lots",
                None,
                UNKNOWN
            ),
            note(1, "field:hp", "unresolved", "lots", None, UNKNOWN),
        ]
    );
    let html = page(&setup).await;
    assert_eq!(value(&html, "value-hp").as_deref(), Some("lots"));
    assert_eq!(
        inner(&html, "#note-0").as_deref(),
        Some(unknown_scribble("lots").as_str())
    );
    // The blank row cell shows "?" as its placeholder; a known cell has none.
    assert_eq!(value(&html, "row-4-1-1").as_deref(), Some(""));
    assert_eq!(placeholder(&html, "row-4-1-1").as_deref(), Some("?"));
    assert_eq!(placeholder(&html, "value-max_hp"), None);
    assert!(
        html.contains(
            "<strong>Not evaluated</strong>: Current HP is at most Max HP. Reason: a value it uses is unknown (null). <code>null_value</code>"
        ),
        "hp-cap"
    );
    // Undo is not offered on create (plan §3f), and a scribble is not undoable.
    assert!(!html.contains("form=\"assist-undo\""));
}

/// Plan §3a (Sol rev-2 question 5): D-M covers unreadable numbers only. Create still refuses
/// an empty Name and unselected required enum and boolean controls.
#[tokio::test]
async fn an_empty_name_and_unset_required_choices_still_refuse_create() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&choices()).await;
    let chosen = [("value-stance", "guard"), ("value-inspired", "true")];
    for refused in [("name", "  "), ("value-stance", ""), ("value-inspired", "")] {
        let mut changes = chosen.to_vec();
        changes.push(refused);
        let create = form(0, &changes, &[row("", "lots")]);
        let (status, _) = setup.post(&create_path(&setup), create).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused:?}");
    }
    let (status, html) = setup
        .post(&create_path(&setup), form(0, &chosen, &[row("", "lots")]))
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
    assert_eq!(saved(&setup, 1).await.0, 1, "nothing was created before");
}

/// D-X1: a blank number erases the value (op `erased`), shown as "?" with Undo back to the
/// number. A change from unknown to a number is undone back to unknown. A nullable field's
/// Known with a blank value is unknown too.
#[tokio::test]
async fn blank_erases_and_undo_restores_the_number() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[("value-luck", "3"), ("null-luck", "known")]).await;
    let rope = [row("r1-1", "1")];
    edit(
        &setup,
        1,
        &[
            ("value-hp", " "),
            ("value-luck", ""),
            ("null-luck", "known"),
        ],
        &rope,
    )
    .await;
    let (_, data) = saved(&setup, 1).await;
    assert_eq!((&data["hp"], &data["luck"]), (&Value::Null, &Value::Null));
    assert_eq!(
        notes_at(&setup, 1, 2).await,
        vec![
            note(
                2,
                "field:hp",
                "erased",
                " ",
                Some(r#"{"int":"84"}"#),
                UNKNOWN
            ),
            note(
                2,
                "field:luck",
                "erased",
                "",
                Some(r#"{"int":"3"}"#),
                UNKNOWN
            ),
        ]
    );
    let html = page(&setup).await;
    assert_eq!(value(&html, "value-hp").as_deref(), Some(""));
    assert_eq!(placeholder(&html, "value-hp").as_deref(), Some("?"));
    assert_eq!(
        inner(&html, "#note-0"),
        None,
        "an erased value has no note line"
    );
    assert_eq!(
        text(&html, "#was-0").as_deref(),
        Some("Last change: was 84")
    );
    let (status, _) = click(&setup, &html, "undo-0").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        saved(&setup, 1).await,
        (3, {
            let mut data = data.clone();
            data["hp"] = json!(84);
            data
        })
    );
    // Unknown → 5, then Undo: unknown again.
    edit(
        &setup,
        3,
        &[("value-luck", "5"), ("null-luck", "known")],
        &rope,
    )
    .await;
    let html = page(&setup).await;
    assert_eq!(
        text(&html, "#was-3").as_deref(),
        Some("Last change: was unknown")
    );
    let (status, _) = click(&setup, &html, "undo-3").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(saved(&setup, 1).await.1["luck"], Value::Null);
}

/// Freeze copies unknown values unchanged; history counts null versus a number as a change,
/// lists the notes, and recomputes each revision's formulas from that revision's values.
#[tokio::test]
async fn unknowns_survive_freeze_and_history_uses_historical_values() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    let player = player_sheet(&setup).await;
    for (revision, hp, quantity) in [(1, "", "lots"), (2, "84", "2")] {
        let id = if revision == 1 { "" } else { "r2-1" };
        let edit = form(
            revision,
            &[("value-hp", hp)],
            &[row("r1-1", "1"), row(id, quantity)],
        );
        let response =
            super::router_request(&setup.app, &player, "/private-sheets/1", Some(edit)).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{revision}");
    }
    let history = |revision: i64| {
        let (app, player) = (setup.app.clone(), player.clone());
        async move {
            let path = format!("/private-sheets/1/history/{revision}");
            let response = super::router_request(&app, &player, &path, None).await;
            assert_eq!(response.status(), StatusCode::OK);
            body(response).await
        }
    };
    let erased = history(2).await;
    let summary = inner(&erased, "#change-summary").unwrap();
    assert!(
        summary.contains("Current HP changed <code>hp</code>."),
        "{summary}"
    );
    assert!(
        summary.contains("<code>field:hp</code> · erased · typed ‘<bdi></bdi>’ · 84 → unknown"),
        "{summary}"
    );
    assert_eq!(load(&erased), LOAD_UNKNOWN);
    let known = history(3).await;
    assert!(
        inner(&known, "#change-summary")
            .unwrap()
            .contains("Current HP changed <code>hp</code>.")
    );
    assert!(
        inner(&known, "#change-summary").unwrap().contains(
            "Row <code>equipment/r2-1</code> of Equipment edited: Quantity <code>equipment/r2-1/quantity</code>."
        ),
        "an unknown property versus a number is a change"
    );
    assert_eq!(load(&known), "3");
    assert_eq!(load(&history(1).await), "1");
    assert_eq!(load(&history(2).await), LOAD_UNKNOWN, "still historical");
    // Back to unknown, then freeze: the frozen revision keeps the nulls.
    let edit = form(3, &[("value-hp", "")], &[row("r1-1", "1"), row("r2-1", "")]);
    let response =
        super::router_request(&setup.app, &player, "/private-sheets/1", Some(edit)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let campaign = setup.campaign;
    let member: i64 = setup
        .store
        .run(move |db| {
            Ok(db.query_row(
                "SELECT revision FROM campaign_members WHERE campaign_id=?1 AND account_id=3",
                [campaign],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    setup
        .store
        .depart(setup.auth[1].clone(), setup.campaign, 3, member)
        .await
        .unwrap();
    let (revision, data) = saved(&setup, 1).await;
    assert_eq!(revision, 5, "frozen");
    assert_eq!(data["hp"], Value::Null);
    assert_eq!(data["equipment"][1]["quantity"], Value::Null);
    let frozen =
        body(super::router_request(&setup.app, &player, "/private-sheets/1", None).await).await;
    assert_eq!(load(&frozen), LOAD_UNKNOWN);
    assert!(!frozen.contains("form=\"assist-undo\""), "frozen: no Undo");
}

/// Review round 1 (Sol Minor 3): an unknown cell with a scribble, then a blank save, writes
/// one data-unchanged revision whose `literal` note resolves the scribble, which disappears;
/// a further blank save writes nothing.
#[tokio::test]
async fn a_blank_save_resolves_the_scribble_of_an_unknown_cell() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    edit(&setup, 1, &[], &[row("r1-1", "1"), row("", "lots")]).await;
    let (_, before) = saved(&setup, 1).await;
    edit(&setup, 2, &[], &[row("r1-1", "1"), row("r2-1", "")]).await;
    assert_eq!(saved(&setup, 1).await, (3, before), "data unchanged");
    assert_eq!(
        notes_at(&setup, 1, 3).await,
        vec![note(
            3,
            "cell:equipment/r2-1/quantity",
            "literal",
            "",
            Some(UNKNOWN),
            UNKNOWN
        )]
    );
    let html = page(&setup).await;
    assert_eq!(value(&html, "row-4-1-1").as_deref(), Some(""));
    assert_eq!(inner(&html, "#note-4-1-1"), None, "the scribble is gone");
    assert_eq!(placeholder(&html, "row-4-1-1").as_deref(), Some("?"));
    let (status, html) = setup
        .post(
            "/private-sheets/1",
            form(3, &[], &[row("r1-1", "1"), row("r2-1", "")]),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains("No change"));
    assert_eq!(history_len(&setup, 1).await, 3);
}

/// Review round 1 (Sol): a scribble on an unknown cell is escaped inside `<bdi>`, on the
/// sheet and in history.
#[tokio::test]
async fn a_scribble_on_an_unknown_cell_renders_escaped_inside_bdi() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let scribble = "\u{5e9}\u{5dc}\u{5d5}\u{5e9} <b>x</b>";
    edit(&setup, 1, &[], &[row("r1-1", "1"), row("", scribble)]).await;
    let escaped = "\u{5e9}\u{5dc}\u{5d5}\u{5e9} &lt;b&gt;x&lt;/b&gt;";
    let html = page(&setup).await;
    assert_eq!(
        inner(&html, "#note-4-1-1").as_deref(),
        Some(unknown_scribble(escaped).as_str())
    );
    assert!(!html.contains("<b>x</b>"), "never raw markup");
    let (_, history) = setup.get("/private-sheets/1/history/2").await;
    let history = inner(&history, "#change-summary").unwrap();
    assert!(
        history.contains(&format!("‘<bdi>{escaped}</bdi>’ · new → unknown")),
        "{history}"
    );
}

/// The ids the control named `name` is described by.
fn described(html: &str, name: &str) -> Vec<String> {
    let page = scraper::Html::parse_document(html);
    page.select(&scraper::Selector::parse(&format!("[name=\"{name}\"]")).unwrap())
        .next()
        .and_then(|element| element.value().attr("aria-describedby"))
        .map(|ids| ids.split(' ').map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Review round 1 (Sonnet Minor 5): an empty (unknown) integer control is described as
/// "Unknown value" for assistive technology, not only by its `placeholder="?"`; a known
/// control is not, and a described control keeps its other descriptions.
#[tokio::test]
async fn an_empty_integer_control_is_described_as_unknown() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    edit(
        &setup,
        1,
        &[("value-hp", "")],
        &[row("r1-1", "1"), row("", "")],
    )
    .await;
    let html = page(&setup).await;
    assert_eq!(
        text(&html, "#unknown-hint").as_deref(),
        Some("Unknown value")
    );
    assert_eq!(described(&html, "value-hp"), vec!["unknown-hint"]);
    assert_eq!(described(&html, "row-4-1-1"), vec!["unknown-hint"]);
    assert!(described(&html, "value-max_hp").is_empty(), "known");
    assert!(described(&html, "row-4-0-1").is_empty(), "known row cell");
    // A scribbled unknown echoes its scribble: described by its note, not the hint.
    edit(&setup, 2, &[], &[row("r1-1", "1"), row("r2-1", "lots")]).await;
    let html = page(&setup).await;
    assert_eq!(described(&html, "row-4-1-1"), vec!["note-4-1-1"]);
}
