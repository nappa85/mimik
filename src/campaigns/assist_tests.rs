//! Slice 1.3 "Switch and Undo" (assisted sheet plan §3a D-Q and D-X4, §3f D-N and D-X1,
//! §5 slice 1.3): sign-led input is applied literally with a "Did you mean" switch, and the
//! last change of each number, choice or yes/no cell can be undone. Both are bound to the
//! displayed note and rechecked inside the save's transaction.
use super::formula_tests::{Setup, body, default_submitter, setup, submitted};
use super::notes_tests::{
    Row, assist, choices, created, form, history_len, inner, note, notes, notes_at, page,
    player_sheet, row, saved, value,
};
use crate::AppError;
use axum::http::StatusCode;
use serde_json::{Value, json};

type Pairs = Vec<(String, String)>;

/// The switch or Undo button with the DOM id `id`, and what it submits: the action of the
/// form its `form` attribute names, that form's pairs, then the button's own pair.
fn action(html: &str, id: &str) -> Option<(String, Pairs)> {
    let page = scraper::Html::parse_document(html);
    let buttons = scraper::Selector::parse(&format!("button[form][id=\"{id}\"]")).unwrap();
    let button = page.select(&buttons).next()?;
    let owner = button.value().attr("form")?;
    let forms = scraper::Selector::parse(&format!("form[id=\"{owner}\"]")).unwrap();
    let form = page.select(&forms).next()?;
    let inputs = scraper::Selector::parse("input, button, select, textarea").unwrap();
    let mut pairs: Pairs = form
        .select(&inputs)
        .map(|input| {
            (
                input.value().attr("name").unwrap_or_default().to_owned(),
                input.value().attr("value").unwrap_or_default().to_owned(),
            )
        })
        .collect();
    pairs.push((
        button.value().attr("name")?.to_owned(),
        button.value().attr("value")?.to_owned(),
    ));
    Some((form.value().attr("action")?.to_owned(), pairs))
}

/// The ids of every switch and Undo button on the page, in page order.
fn offered(html: &str) -> Vec<String> {
    let page = scraper::Html::parse_document(html);
    let buttons = scraper::Selector::parse("button[form]").unwrap();
    page.select(&buttons)
        .filter_map(|button| button.value().attr("id").map(str::to_owned))
        .collect()
}

/// The text of the first element matching `selector`.
fn text(html: &str, selector: &str) -> Option<String> {
    let page = scraper::Html::parse_document(html);
    page.select(&scraper::Selector::parse(selector).unwrap())
        .next()
        .map(|element| element.text().collect())
}

/// Clicks the button with the id `id` and returns the answer.
async fn click(setup: &Setup, html: &str, id: &str) -> (StatusCode, String) {
    let (path, pairs) = action(html, id).unwrap_or_else(|| panic!("no button {id:?} in {html}"));
    setup.post(&path, pairs).await
}

/// The `target` column of the note at `revision` and `address`.
async fn target(setup: &Setup, revision: i64, address: &'static str) -> Option<i64> {
    setup
        .store
        .run(move |db| {
            Ok(db.query_row(
                "SELECT target FROM private_sheet_notes WHERE sheet_id=1 AND revision=?1 AND address=?2",
                rusqlite::params![revision, address],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap()
}

async fn edit(setup: &Setup, revision: i64, changes: &[(&str, &str)], rows: &[Row<'_>]) {
    let (status, html) = setup
        .post("/private-sheets/1", form(revision, changes, rows))
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{changes:?}: {html}");
}

/// D-X4: the offer of cell `key` is non-blocking and can be hidden without script: an
/// unnamed checkbox (never submitted) labelled Hide.
fn dismissible(html: &str, key: &str) {
    let page = scraper::Html::parse_document(html);
    let dismiss = page
        .select(&scraper::Selector::parse(&format!("#dismiss-{key}")).unwrap())
        .next()
        .unwrap();
    assert_eq!(dismiss.value().attr("type"), Some("checkbox"));
    assert_eq!(dismiss.value().attr("name"), None);
    // Named on its own: it keeps focus, and its name, once the offer is hidden.
    assert_eq!(
        dismiss.value().attr("aria-label"),
        Some("Hide the suggestion")
    );
    assert!(html.contains(&format!("<label for=\"dismiss-{key}\">Hide</label>")));
}

const HP_SWITCH: &str = "switch-0";

/// D-Q: "-15" on HP 84 stores −15 literally and offers "Did you mean 84 − 15 = 69?";
/// "-1" on Armour 5 stores −1. Re-entering the literal is a no-op (D-U).
#[tokio::test]
async fn sign_led_input_is_applied_literally_with_the_offer() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[("value-armour", "5")]).await;
    let rope = [row("r1-1", "1")];
    let signed = [
        ("value-armour", "-1"),
        ("value-hp", "-15"),
        ("value-max_hp", "-5+3"),
    ];
    edit(&setup, 1, &signed, &rope).await;
    let (revision, data) = saved(&setup, 1).await;
    assert_eq!(revision, 2);
    assert_eq!(
        (&data["hp"], &data["armour"], &data["max_hp"]),
        (&json!(-15), &json!(-1), &json!(-2))
    );
    assert_eq!(
        notes_at(&setup, 1, 2).await,
        vec![
            note(
                2,
                "field:armour",
                "sign_literal",
                "-1",
                Some(r#"{"int":"5"}"#),
                r#"{"int":"-1"}"#
            ),
            note(
                2,
                "field:hp",
                "sign_literal",
                "-15",
                Some(r#"{"int":"84"}"#),
                r#"{"int":"-15"}"#
            ),
            note(
                2,
                "field:max_hp",
                "sign_literal",
                "-5+3",
                Some(r#"{"int":"84"}"#),
                r#"{"int":"-2"}"#
            ),
        ]
    );
    let html = page(&setup).await;
    assert_eq!(value(&html, "value-hp").as_deref(), Some("-15"));
    assert_eq!(
        text(&html, "#offer-0").as_deref(),
        Some("Did you mean 84 \u{2212} 15 = 69? YesHide")
    );
    assert_eq!(
        inner(&html, "#offer-0 bdi").as_deref(),
        Some("84 \u{2212} 15")
    );
    assert!(html.contains("aria-describedby=\"offer-0\""), "{html}");
    // A sign-led sum is shown like a sum, and offered relative to the value before.
    assert_eq!(
        inner(&html, "#note-1").as_deref(),
        Some("<bdi>\u{2212}5 + 3</bdi> = -2")
    );
    assert!(html.contains("aria-describedby=\"note-1 offer-1\""));
    assert_eq!(
        offered(&html),
        [
            HP_SWITCH, "undo-0", "switch-1", "undo-1", "switch-2", "undo-2",
        ]
    );
    assert_eq!(
        text(&html, "#offer-1").as_deref(),
        Some("Did you mean 84 \u{2212} 5 + 3 = 82? YesHide")
    );
    assert_eq!(
        text(&html, "#offer-2").as_deref(),
        Some("Did you mean 5 \u{2212} 1 = 4? YesHide")
    );
    assert_eq!(
        text(&html, "#was-0").as_deref(),
        Some("Last change: was 84")
    );
    assert_eq!(text(&html, "#was-2").as_deref(), Some("Last change: was 5"));
    dismissible(&html, "0");
    // D-U wins: the same literal again writes nothing and keeps the same offer.
    let notes_before = notes(&setup, 1).await;
    let (status, again) = setup
        .post("/private-sheets/1", form(2, &signed, &rope))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(again.contains("No change"), "{again}");
    assert_eq!(history_len(&setup, 1).await, 2);
    assert_eq!(notes(&setup, 1).await, notes_before);
    assert_eq!(offered(&again), offered(&html));
}

/// The switch writes b ± n with op `switched` once; a second submit is refused (409) and
/// writes nothing. After it, the cell shows "84 − 15 = 69" and offers Undo.
#[tokio::test]
async fn the_switch_writes_the_relative_reading_once() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    edit(&setup, 1, &[("value-hp", "-15")], &[row("r1-1", "1")]).await;
    let html = page(&setup).await;
    let (path, pairs) = action(&html, HP_SWITCH).unwrap();
    assert_eq!(path, "/private-sheets/1/switch");
    let keys: Vec<&str> = pairs.iter().map(|(key, _)| key.as_str()).collect();
    assert_eq!(keys, ["csrf", "base", "note"]);
    assert_eq!(pairs[1].1, "2");
    assert_eq!(pairs[2].1, "2:field:hp");
    let (status, answer) = setup.post(&path, pairs.clone()).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{answer}");
    let (revision, data) = saved(&setup, 1).await;
    assert_eq!((revision, &data["hp"]), (3, &json!(69)));
    assert_eq!(
        notes_at(&setup, 1, 3).await,
        vec![note(
            3,
            "field:hp",
            "switched",
            "-15",
            Some(r#"{"int":"-15"}"#),
            r#"{"int":"69"}"#
        )]
    );
    let html = page(&setup).await;
    assert_eq!(
        inner(&html, "#note-0").as_deref(),
        Some("<bdi>84 \u{2212} 15</bdi> = 69")
    );
    assert_eq!(offered(&html), ["undo-0"]);
    assert_eq!(
        text(&html, "#was-0").as_deref(),
        Some("Last change: was -15")
    );
    // Double submit: the note is no longer current, so nothing is written.
    let (status, refused) = setup.post(&path, pairs).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        refused.contains("Not switched: Current HP changed since, or it was already switched."),
        "{refused}"
    );
    assert_eq!(saved(&setup, 1).await, (3, data));
    assert_eq!(history_len(&setup, 1).await, 3);
}

/// The switch is bound to the note, not the page revision: an unrelated save keeps it valid,
/// a newer value of the cell refuses it. Malformed or forged requests get 400 or 403.
#[tokio::test]
async fn the_switch_is_bound_to_the_displayed_note() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let rope = [row("r1-1", "1")];
    edit(&setup, 1, &[("value-hp", "-15")], &rope).await;
    let shown = page(&setup).await;
    // Another tab changes Max HP: the shown offer still holds (the cell is unchanged).
    edit(
        &setup,
        2,
        &[("value-hp", "-15"), ("value-max_hp", "90")],
        &rope,
    )
    .await;
    let (status, _) = click(&setup, &shown, HP_SWITCH).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(saved(&setup, 1).await.1["hp"], json!(69));
    // A new literal, then another tab sets HP 50: the shown offer is refused.
    let fields = [("value-max_hp", "90")];
    edit(&setup, 4, &[fields[0], ("value-hp", "-10")], &rope).await;
    let shown = page(&setup).await;
    edit(&setup, 5, &[fields[0], ("value-hp", "50")], &rope).await;
    let (status, refused) = click(&setup, &shown, HP_SWITCH).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(refused.contains("Not switched"), "{refused}");
    assert_eq!(saved(&setup, 1).await.0, 6, "nothing written");
    assert_eq!(saved(&setup, 1).await.1["hp"], json!(50));
}

/// Forged switch and Undo requests against sheet 1 at live revision 6 (HP's current note at
/// 6, an older one at 5) and what each must answer.
fn forged() -> Vec<(&'static str, Pairs, StatusCode)> {
    let pairs = |base: &str, note: &str| {
        vec![
            ("csrf".to_owned(), "a".repeat(64)),
            ("base".to_owned(), base.to_owned()),
            ("note".to_owned(), note.to_owned()),
        ]
    };
    vec![
        (
            "base not a number",
            pairs("x", "5:field:hp"),
            StatusCode::BAD_REQUEST,
        ),
        ("base 0", pairs("0", "5:field:hp"), StatusCode::BAD_REQUEST),
        (
            "base padded",
            pairs("06", "5:field:hp"),
            StatusCode::BAD_REQUEST,
        ),
        (
            "base above live",
            pairs("99", "5:field:hp"),
            StatusCode::CONFLICT,
        ),
        (
            "note newer than base",
            pairs("4", "5:field:hp"),
            StatusCode::BAD_REQUEST,
        ),
        (
            "malformed note",
            pairs("6", "garbage"),
            StatusCode::BAD_REQUEST,
        ),
        (
            "unknown field",
            pairs("6", "5:field:nope"),
            StatusCode::BAD_REQUEST,
        ),
        (
            "derived field",
            pairs("6", "5:field:load"),
            StatusCode::BAD_REQUEST,
        ),
        (
            "text cell",
            pairs("6", "1:cell:equipment/r1-1/name"),
            StatusCode::BAD_REQUEST,
        ),
        (
            "row not on the sheet (stale, not malformed)",
            pairs("6", "1:cell:equipment/r9-9/quantity"),
            StatusCode::CONFLICT,
        ),
        (
            "metadata",
            pairs("6", "1:meta:name"),
            StatusCode::BAD_REQUEST,
        ),
        (
            "not the current note",
            pairs("6", "2:field:hp"),
            StatusCode::CONFLICT,
        ),
        (
            "extra key",
            [
                pairs("6", "5:field:hp"),
                vec![("value-hp".into(), "1".into())],
            ]
            .concat(),
            StatusCode::BAD_REQUEST,
        ),
        (
            "missing key",
            pairs("6", "5:field:hp")[..2].to_vec(),
            StatusCode::BAD_REQUEST,
        ),
        (
            "wrong CSRF",
            [
                vec![("csrf".to_owned(), "b".repeat(64))],
                pairs("6", "5:field:hp")[1..].to_vec(),
            ]
            .concat(),
            StatusCode::FORBIDDEN,
        ),
        (
            "no CSRF",
            pairs("6", "5:field:hp")[1..].to_vec(),
            StatusCode::FORBIDDEN,
        ),
    ]
}

/// Malformed and forged switch or Undo requests write nothing: 400 for bad keys, values and
/// addresses, 409 for a base above the live revision or a note that is not current, 403
/// without the CSRF token.
#[tokio::test]
async fn malformed_and_forged_requests_write_nothing() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let rope = [row("r1-1", "1")];
    for (revision, hp) in [(1, "-15"), (2, "69"), (3, "70"), (4, "-10"), (5, "50")] {
        edit(&setup, revision, &[("value-hp", hp)], &rope).await;
    }
    for (label, form, want) in forged() {
        for route in ["/private-sheets/1/switch", "/private-sheets/1/undo"] {
            let (status, _) = setup.post(route, form.clone()).await;
            assert_eq!(status, want, "{label} {route}");
        }
    }
    assert_eq!(saved(&setup, 1).await.0, 6, "nothing written");
}

/// No offer without a known value before, when b ± n overflows, or over 0 (where the
/// relative reading is the literal); a forged switch writes nothing.
#[tokio::test]
async fn no_offer_without_a_known_before_value_or_on_overflow() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[("value-hp", "-5")]).await;
    assert!(
        offered(&page(&setup).await).is_empty(),
        "create has no before"
    );
    let min = i64::MIN.to_string();
    let rows = [row("r1-1", "1"), row("", "-2")];
    edit(
        &setup,
        1,
        &[
            ("value-hp", "-5"),
            ("value-armour", &min),
            ("value-max_hp", "0"),
        ],
        &rows,
    )
    .await;
    let rows = [row("r1-1", "1"), row("r2-1", "-2")];
    let changes = [
        ("value-hp", "-5"),
        ("value-armour", "-1"),
        ("value-max_hp", "-7"),
        ("value-luck", "-3"),
        ("null-luck", "known"),
    ];
    edit(&setup, 2, &changes, &rows).await;
    let (_, data) = saved(&setup, 1).await;
    assert_eq!(
        (&data["armour"], &data["max_hp"], &data["luck"]),
        (&json!(-1), &json!(-7), &json!(-3))
    );
    let html = page(&setup).await;
    // Undo is offered (the values before are known); no switch anywhere: the new row has no
    // before value, Luck was unknown, b − 1 overflows for Armour, Max HP was 0.
    assert_eq!(offered(&html), ["undo-1", "undo-2", "undo-3"]);
    assert_eq!(text(&html, "#was-1").as_deref(), Some("Last change: was 0"));
    assert_eq!(
        text(&html, "#was-2"),
        Some(format!("Last change: was {min}"))
    );
    assert_eq!(
        text(&html, "#was-3").as_deref(),
        Some("Last change: was unknown")
    );
    assert_eq!(inner(&html, "#offer-2"), None);
    let forged = vec![
        ("csrf".to_owned(), "a".repeat(64)),
        ("base".to_owned(), "3".to_owned()),
        ("note".to_owned(), "3:field:armour".to_owned()),
    ];
    let (status, refused) = setup.post("/private-sheets/1/switch", forged).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        refused.contains("Not switched: Armour modifier"),
        "{refused}"
    );
    assert_eq!(saved(&setup, 1).await, (3, data));
}

/// Undo of a literal, a sum, an enum, a boolean, Not set → Set and a switch, each back to
/// its value before with a `reverted` note naming the undone revision; Undo of an Undo; a
/// double submit is refused.
#[tokio::test]
async fn undo_restores_the_last_change_of_a_cell() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&choices()).await;
    let choice = [("value-stance", "guard"), ("value-inspired", "false")];
    created(&setup, &choice).await;
    let (_, original) = saved(&setup, 1).await;
    let long = "a".repeat(64);
    let lamp = Row {
        broken: "true",
        set: "present",
        ..row("r1-1", "1")
    };
    let changes = [
        ("value-stance", long.as_str()),
        ("value-inspired", "true"),
        ("value-hp", "84 -15"),
        ("value-armour", "3"),
    ];
    edit(&setup, 1, &changes, &[lamp]).await;
    let html = page(&setup).await;
    assert_eq!(
        offered(&html),
        ["undo-0", "undo-2", "undo-6", "undo-7", "undo-4-0-3"]
    );
    for (id, was) in [
        ("0", "84"),
        ("2", "-1"),
        ("6", "guard"),
        ("7", "false"),
        ("4-0-3", "not set"),
    ] {
        let shown = text(&html, &format!("#was-{id}"));
        assert_eq!(shown, Some(format!("Last change: was {was}")), "{id}");
    }
    let undo_hp = action(&html, "undo-0").unwrap();
    let mut revision = 2;
    for label in offered(&html) {
        let (status, answer) = click(&setup, &page(&setup).await, &label).await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{label}: {answer}");
        revision += 1;
        assert_eq!(notes_at(&setup, 1, revision).await.len(), 1, "{label}");
        assert_eq!(
            notes_at(&setup, 1, revision).await[0].2,
            "reverted",
            "{label}"
        );
    }
    assert_eq!(saved(&setup, 1).await, (7, original.clone()));
    assert_eq!(target(&setup, 3, "field:hp").await, Some(2));
    assert_eq!(
        notes_at(&setup, 1, 3).await,
        vec![note(
            3,
            "field:hp",
            "reverted",
            "",
            Some(r#"{"int":"69"}"#),
            r#"{"int":"84"}"#
        )]
    );
    let (_, history) = setup.get("/private-sheets/1/history/3").await;
    assert!(history.contains("undid revision 2"), "{history}");
    // Double submit: the HP note is no longer current.
    let (status, refused) = setup.post(&undo_hp.0, undo_hp.1).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        refused.contains(
            "Nothing undone: Current HP changed since, or this change was already undone."
        ),
        "{refused}"
    );
    assert_eq!(saved(&setup, 1).await.0, 7);
    // Undo of an Undo puts the change back.
    let html = page(&setup).await;
    assert_eq!(
        text(&html, "#was-0").as_deref(),
        Some("Last change: was 69")
    );
    let (status, _) = click(&setup, &html, "undo-0").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(saved(&setup, 1).await.1["hp"], json!(69));
    // Set → Not set is undone back to the value (present ↔ absent).
    let all: Vec<(&str, &str)> = [choice.as_slice(), &[("value-hp", "69")]].concat();
    edit(&setup, 8, &all, &[lamp]).await;
    edit(&setup, 9, &all, &[row("r1-1", "1")]).await;
    assert!(
        saved(&setup, 1).await.1["equipment"][0]
            .get("broken")
            .is_none()
    );
    let (status, _) = click(&setup, &page(&setup).await, "undo-4-0-3").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        saved(&setup, 1).await.1["equipment"][0]["broken"],
        json!(true)
    );
}

/// A switch, then Undo of the switch, on a list cell.
#[tokio::test]
async fn switch_and_undo_work_on_list_cells() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    edit(&setup, 1, &[], &[row("r1-1", "5")]).await;
    edit(&setup, 2, &[], &[row("r1-1", "-2")]).await;
    let html = page(&setup).await;
    assert_eq!(
        inner(&html, "#offer-4-0-1 bdi").as_deref(),
        Some("5 \u{2212} 2")
    );
    assert!(html.contains("aria-describedby=\"offer-4-0-1\""), "{html}");
    // The buttons name the cell by reference: its label and its row legend.
    let document = scraper::Html::parse_document(&html);
    let switch = document
        .select(&scraper::Selector::parse("#switch-4-0-1").unwrap())
        .next()
        .unwrap();
    assert_eq!(
        switch.value().attr("aria-labelledby"),
        Some("switch-4-0-1 label-4-0-1 list-legend-4 legend-4-0")
    );
    assert_eq!(text(&html, "#label-4-0-1").as_deref(), Some("Quantity"));
    assert_eq!(text(&html, "#list-legend-4").as_deref(), Some("Equipment"));
    assert!(
        text(&html, "#legend-4-0")
            .unwrap()
            .starts_with("Row 1 of 1 · ID r1-1"),
        "{html}"
    );
    let (status, _) = click(&setup, &html, "switch-4-0-1").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        saved(&setup, 1).await.1["equipment"][0]["quantity"],
        json!(3)
    );
    let (status, _) = click(&setup, &page(&setup).await, "undo-4-0-1").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        saved(&setup, 1).await.1["equipment"][0]["quantity"],
        json!(-2)
    );
}

/// Nothing to undo on create, in a new row, for a scribble, for text, on history pages and
/// on frozen sheets (where a forged Undo gets 403).
#[tokio::test]
async fn undo_is_not_offered_where_nothing_can_be_undone() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    let player = player_sheet(&setup).await;
    let get = |path: &'static str| super::router_request(&setup.app, &player, path, None);
    let post = |path: &'static str, form: Pairs| {
        super::router_request(&setup.app, &player, path, Some(form))
    };
    assert!(
        offered(&body(get("/private-sheets/1").await).await).is_empty(),
        "create"
    );
    // Text edits, a new row and a scribble offer nothing.
    let rows = [
        row("r1-1", "1"),
        Row {
            name: "Lamp",
            ..row("", "3")
        },
    ];
    let edit = form(1, &[("name", "Renamed"), ("value-hp", "lots")], &rows);
    let response = post("/private-sheets/1", edit).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let html = body(get("/private-sheets/1").await).await;
    assert!(html.contains("not understood"), "{html}");
    assert!(offered(&html).is_empty(), "{:?}", offered(&html));
    // A change offers Undo; its history page does not.
    let rows = [
        row("r1-1", "1"),
        Row {
            name: "Lamp",
            ..row("r2-1", "3")
        },
    ];
    let edit = form(2, &[("name", "Renamed"), ("value-hp", "70")], &rows);
    assert_eq!(
        post("/private-sheets/1", edit).await.status(),
        StatusCode::SEE_OTHER
    );
    let html = body(get("/private-sheets/1").await).await;
    assert_eq!(offered(&html), ["undo-0"]);
    let undo = action(&html, "undo-0").unwrap();
    let history = body(get("/private-sheets/1/history/3").await).await;
    assert!(offered(&history).is_empty());
    assert!(!history.contains("/undo"), "{history}");
    // Frozen on departure: no Undo, and a forged one is 403 with nothing written.
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
    let frozen = body(get("/private-sheets/1").await).await;
    assert!(offered(&frozen).is_empty(), "frozen");
    let response = post("/private-sheets/1/undo", undo.1).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(saved(&setup, 1).await.0, 4, "frozen at 4, nothing written");
}

/// D-X1: an erased (unknown) value is undoable. Slice 1.4 writes `erased`; here the hook
/// stands in for it on a Known → Unknown change.
#[tokio::test]
async fn an_erased_value_is_undoable() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let rope = [row("r1-1", "1")];
    edit(
        &setup,
        1,
        &[("value-luck", "3"), ("null-luck", "known")],
        &rope,
    )
    .await;
    *super::super::notes::BEFORE_INSERT.lock().unwrap() = Some(Box::new(|notes| {
        notes[0].op = super::super::notes::Op::Erased;
        Ok(())
    }));
    edit(&setup, 2, &[], &rope).await;
    assert_eq!(notes_at(&setup, 1, 3).await[0].2, "erased");
    assert_eq!(saved(&setup, 1).await.1["luck"], Value::Null);
    let (status, _) = click(&setup, &page(&setup).await, "undo-3").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(saved(&setup, 1).await.1["luck"], json!(3));
}

/// The live revision of sheet 1, read without an account (its Master may be disabled).
async fn live_revision(setup: &Setup) -> i64 {
    setup
        .store
        .run(|db| {
            Ok(db.query_row(
                "SELECT revision FROM private_sheets WHERE id=1",
                [],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap()
}

/// Rights are checked before (403) and again inside the write transaction (403, nothing
/// written).
#[tokio::test]
async fn rights_are_rechecked_inside_the_transaction() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    let player = player_sheet(&setup).await;
    let edit = form(1, &[("value-hp", "-15")], &[row("r1-1", "1")]);
    let response =
        super::router_request(&setup.app, &player, "/private-sheets/1", Some(edit)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let html =
        body(super::router_request(&setup.app, &player, "/private-sheets/1", None).await).await;
    let (path, pairs) = action(&html, HP_SWITCH).unwrap();
    // The supervising Master is disabled inside the write's transaction.
    let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = ran.clone();
    *super::super::sheets::BEFORE_SAVE_RECHECK.lock().unwrap() = Some(Box::new(move |db| {
        seen.store(true, std::sync::atomic::Ordering::SeqCst);
        db.execute(
            "UPDATE accounts SET epoch=2,state='disabled' WHERE id=2",
            [],
        )?;
        Ok(())
    }));
    let response = super::router_request(&setup.app, &player, &path, Some(pairs.clone())).await;
    assert!(
        ran.load(std::sync::atomic::Ordering::SeqCst),
        "the recheck ran"
    );
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(live_revision(&setup).await, 2, "nothing written");
    // Rights lost before the request: 403 as well.
    setup
        .store
        .run(|db| {
            db.execute(
                "UPDATE accounts SET epoch=2,state='disabled' WHERE id=2",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let response = super::router_request(&setup.app, &player, &path, Some(pairs)).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(live_revision(&setup).await, 2, "nothing written");
}

/// Inside the write transaction the bound note must still be current and the revision
/// unchanged; otherwise 409 and nothing is written.
#[tokio::test]
async fn the_binding_is_rechecked_inside_the_transaction() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let rope = [row("r1-1", "1")];
    edit(&setup, 1, &[("value-hp", "-15")], &rope).await;
    edit(
        &setup,
        2,
        &[("value-hp", "-15"), ("value-max_hp", "90")],
        &rope,
    )
    .await;
    let html = page(&setup).await;
    // A newer note on HP at the live revision (possible for the revision actor, L1).
    *super::super::sheets::BEFORE_SAVE_RECHECK.lock().unwrap() = Some(Box::new(|db| {
        db.execute("INSERT INTO private_sheet_notes(sheet_id,revision,address,op,raw,before,after) VALUES(1,3,'field:hp','literal','-15','{\"int\":\"-15\"}','{\"int\":\"-15\"}')", [])?;
        Ok(())
    }));
    let (status, refused) = click(&setup, &html, HP_SWITCH).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(refused.contains("Not switched"), "{refused}");
    assert_eq!(saved(&setup, 1).await.0, 3, "nothing written");
    assert!(
        notes_at(&setup, 1, 3)
            .await
            .iter()
            .all(|note| note.1 != "field:hp"),
        "the hook's note rolled back"
    );
    // Another write lands inside the transaction first: the revision check refuses.
    let master = setup.auth[1].clone();
    let version = setup.version;
    *super::super::sheets::BEFORE_SAVE_RECHECK.lock().unwrap() = Some(Box::new(move |db| {
        let pin = crate::definitions::private_pin::validate(
            crate::definitions::private_pin::load(db, version)?,
        )?;
        let mut live = super::super::snapshots::load(db, 1)?;
        live.notes = "Another tab".into();
        super::super::snapshots::write(db, &master, live, "edit", Some(&pin))?;
        Ok(())
    }));
    let (status, refused) = click(&setup, &html, "undo-1").await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(saved(&setup, 1).await.0, 3, "nothing written");
    // Without interference both still work.
    let (status, _) = click(&setup, &html, HP_SWITCH).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let fail: super::super::notes::InsertHook = Box::new(|_| {
        Err(AppError::Invalid(
            "Synthetic failure after the snapshot write.",
        ))
    });
    *super::super::notes::BEFORE_INSERT.lock().unwrap() = Some(fail);
    let (status, _) = click(&setup, &html, "undo-1").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        saved(&setup, 1).await.0,
        4,
        "a failed note insert rolls the write back"
    );
}

/// Each button submits its own small form; Enter in the sheet still saves the sheet, and
/// the sheet form carries no switch or Undo key.
#[tokio::test]
async fn switch_and_undo_forms_stay_out_of_the_sheet_form() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    edit(&setup, 1, &[("value-hp", "-15")], &[row("r1-1", "1")]).await;
    let html = page(&setup).await;
    assert_eq!(
        default_submitter(&html),
        Some(("op".to_owned(), "save".to_owned()))
    );
    let sheet = submitted(&html);
    assert!(
        sheet.iter().all(|(key, _)| key != "note" && key != "base"),
        "{sheet:?}"
    );
    let document = scraper::Html::parse_document(&html);
    let buttons = scraper::Selector::parse("button[form]").unwrap();
    let sheet_form = scraper::Selector::parse("form[action=\"/private-sheets/1\"]").unwrap();
    let main = document.select(&sheet_form).next().unwrap();
    let inside: Vec<_> = main.select(&buttons).collect();
    assert_eq!(inside.len(), 2, "switch and Undo inside the sheet markup");
    for (button, owner, labelled) in [
        (inside[0], "assist-switch", "switch-0 label-0"),
        (inside[1], "assist-undo", "undo-0 label-0"),
    ] {
        // The button carries the note key; its form has exactly csrf and base.
        assert_eq!(button.value().attr("form"), Some(owner));
        assert_eq!(button.value().attr("name"), Some("note"));
        assert_eq!(button.value().attr("value"), Some("2:field:hp"));
        assert_eq!(button.value().attr("aria-labelledby"), Some(labelled));
        let own = scraper::Selector::parse(&format!("form[id=\"{owner}\"]")).unwrap();
        let form = document.select(&own).next().unwrap();
        assert_eq!(form.value().attr("method"), Some("post"));
        let inputs = scraper::Selector::parse("input").unwrap();
        let names: Vec<_> = form
            .select(&inputs)
            .filter_map(|input| input.value().attr("name"))
            .collect();
        assert_eq!(names, ["csrf", "base"]);
        let nested = scraper::Selector::parse(&format!("form form[id=\"{owner}\"]")).unwrap();
        assert!(document.select(&nested).next().is_none(), "not nested");
    }
    assert_eq!(text(&html, "#label-0").as_deref(), Some("Current HP"));
    // The sheet form re-posted unchanged is still a no-op save.
    let (status, again) = setup.post("/private-sheets/1", sheet).await;
    assert_eq!(status, StatusCode::OK);
    assert!(again.contains("No change"));
}

/// The new routes sit behind the auth layer's POST checks like every sheet POST: a foreign
/// Origin is 403 and a body that is not URL-encoded is 415, before anything is read.
#[tokio::test]
async fn foreign_origin_and_content_type_are_refused() {
    use tower::ServiceExt;
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    edit(&setup, 1, &[("value-hp", "-15")], &[row("r1-1", "1")]).await;
    let (path, pairs) = action(&page(&setup).await, HP_SWITCH).unwrap();
    let body = serde_urlencoded::to_string(&pairs).unwrap();
    for (origin, content, want) in [
        (
            "http://evil.example",
            "application/x-www-form-urlencoded",
            StatusCode::FORBIDDEN,
        ),
        (
            "http://127.0.0.1:3000",
            "text/plain",
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            "http://127.0.0.1:3000",
            "multipart/form-data; boundary=x",
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
    ] {
        let request = axum::http::Request::builder()
            .method("POST")
            .uri(&path)
            .header("host", "127.0.0.1:3000")
            .header("cookie", setup.auth[1].fixture_cookie())
            .header("origin", origin)
            .header("content-type", content)
            .body(axum::body::Body::from(body.clone()))
            .unwrap();
        let response = setup.app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), want, "{origin} {content}");
    }
    assert_eq!(saved(&setup, 1).await.0, 2, "nothing written");
    let (status, _) = setup.post(&path, pairs).await;
    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "the same request from the page"
    );
}

/// The accessible name `aria-labelledby` gives the element with id `id`: the text of each
/// referenced element, joined by spaces.
fn labelled_name(html: &str, id: &str) -> String {
    let page = scraper::Html::parse_document(html);
    let by_id = |id: &str| {
        page.select(&scraper::Selector::parse(&format!("[id=\"{id}\"]")).unwrap())
            .next()
            .map(|element| element.text().collect::<String>())
            .unwrap_or_default()
    };
    let element = page
        .select(&scraper::Selector::parse(&format!("[id=\"{id}\"]")).unwrap())
        .next()
        .unwrap();
    element
        .value()
        .attr("aria-labelledby")
        .unwrap()
        .split(' ')
        .map(by_id)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Review round 1 (Sonnet M1): a switch or Undo shown for a row that has since been removed
/// is stale, not malformed: 409 with the generic "The cell", nothing written.
#[tokio::test]
async fn a_stale_action_on_a_removed_row_is_409() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    edit(&setup, 1, &[], &[row("r1-1", "5")]).await;
    edit(&setup, 2, &[], &[row("r1-1", "-2")]).await;
    let shown = page(&setup).await;
    edit(&setup, 3, &[], &[]).await;
    for (id, refusal) in [
        (
            "switch-4-0-1",
            "Not switched: The cell changed since, or it was already switched.",
        ),
        (
            "undo-4-0-1",
            "Nothing undone: The cell changed since, or this change was already undone.",
        ),
    ] {
        let (status, html) = click(&setup, &shown, id).await;
        assert_eq!(status, StatusCode::CONFLICT, "{id}: {html}");
        assert!(html.contains(refusal), "{id}: {html}");
    }
    assert_eq!(saved(&setup, 1).await.0, 4, "nothing written");
}

/// Review round 1 (Sol Minor 3): two lists with the same property name and the same first
/// host ID give their row buttons distinct names, through the list legend.
#[tokio::test]
async fn row_actions_name_their_list() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let mut definition = assist();
    let mut spares = definition["fields"][4].clone();
    spares["id"] = json!("spares");
    spares["label"] = json!("Spares");
    definition["fields"].as_array_mut().unwrap().push(spares);
    definition["groups"][1]["fields"]
        .as_array_mut()
        .unwrap()
        .push(json!("spares"));
    let setup = setup(&definition).await;
    let spare = |quantity: &'static str, id: &'static str| {
        [
            ("id-6-0", id),
            ("row-6-0-0", "Rope"),
            ("row-6-0-1", quantity),
            ("row-6-0-2", "1"),
            ("row-6-0-3", ""),
            ("set-6-0-3", "absent"),
        ]
    };
    created(&setup, &spare("5", "")).await;
    edit(&setup, 1, &spare("-2", "r1-1"), &[row("r1-1", "-2")]).await;
    let html = page(&setup).await;
    let equipment = labelled_name(&html, "switch-4-0-1");
    let spares = labelled_name(&html, "switch-6-0-1");
    assert!(
        equipment.starts_with("Yes Quantity Equipment Row 1 of 1"),
        "{equipment}"
    );
    assert!(
        spares.starts_with("Yes Quantity Spares Row 1 of 1"),
        "{spares}"
    );
    assert_ne!(equipment, spares);
}

/// Review round 1 (Sol Major): a long expression is elided under the cell and in the offer
/// (the page stays within its markup bound); history keeps what was typed in full.
#[tokio::test]
async fn long_expressions_are_elided_on_the_sheet_and_kept_in_history() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let raw = "-11".repeat(64);
    edit(&setup, 1, &[("value-hp", &raw)], &[row("r1-1", "1")]).await;
    let html = page(&setup).await;
    // Cut at a term boundary with the count of terms left out; the result stays in full.
    let thirteen = format!("\u{2212}11{}", " \u{2212} 11".repeat(12));
    assert_eq!(
        inner(&html, "#note-0").unwrap(),
        format!("<bdi>{thirteen} \u{2026} (+51 more terms)</bdi> = -704")
    );
    let offer = format!("84{} \u{2026} (+52 more terms)", " \u{2212} 11".repeat(12));
    assert_eq!(inner(&html, "#offer-0 bdi").unwrap(), offer);
    assert!(
        text(&html, "#offer-0")
            .unwrap()
            .contains(&format!("{offer} = -620?")),
        "{html}"
    );
    let (_, history) = setup.get("/private-sheets/1/history/2").await;
    assert!(history.contains(&raw), "history keeps the full entry");
}
