//! Slice 1.5a "Per-field save: metadata and scalar fields" (assisted sheet plan §3a
//! "Per-cell save", D-C, D-X3; §5 slice 1.5a): Name, General notes and each scalar field
//! save on their own from their own form, against the unit's `base` (value-based
//! conflicts), with a bounded permit wait and a styled busy page. The full form POST is
//! still accepted, with its whole-sheet revision check.
use super::formula_tests::{Setup, body, setup};
use super::notes_tests::{assist, choices, created, form, note, notes, notes_at, row, saved};
use axum::http::StatusCode;
use serde_json::{Value, json};

type Pairs = Vec<(String, String)>;

/// The formula fixture (`formula_definition`): a text field called `name`, an integer, a
/// nullable integer, a multiline text and derived fields, with a mapped source.
fn named() -> Value {
    let mut definition = crate::definitions::source_fixture::formula_definition();
    definition["sources"] =
        json!([{"source_id": 1, "pdf_sha256": "0".repeat(64), "roles": ["sheet"]}]);
    definition["fields"][0]["mapping"] =
        json!({"state": "mapped", "regions": [{"source_id": 1, "page_id": 1, "unit_id": 1}]});
    definition
}

/// Sheet 1 of the `named` fixture at revision 1.
async fn named_sheet(setup: &Setup) {
    let create = vec![
        pair("csrf", &"a".repeat(64)),
        pair("revision", "0"),
        pair("name", "Sheet title"),
        pair("notes", ""),
        pair("value-name", "Field text"),
        pair("value-vitality", "4"),
        pair("value-hit-points", "7"),
        pair("null-hit-points", "known"),
        pair("value-story", "Once"),
    ];
    let path = format!(
        "/campaigns/{}/sheets/new?version={}",
        setup.campaign, setup.version
    );
    let (status, html) = setup.post(&path, create).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
}

fn pair(key: &str, value: &str) -> (String, String) {
    (key.into(), value.into())
}

/// What a browser submits from the form that owns the control with DOM id `id`, after the
/// player put `typed` in it (`None`: unchanged): the form's action and its named,
/// enabled controls, plus its default button's pair if it has a name. The form must have
/// exactly one submit button of its own (the switch and Undo buttons inside it belong to
/// other forms through `form`), so Enter and its Save button submit the same thing.
pub(super) fn unit_form(html: &str, id: &str, typed: Option<&str>) -> (String, Pairs) {
    let page = scraper::Html::parse_document(html);
    let control = page
        .select(&scraper::Selector::parse(&format!("[id=\"{id}\"]")).unwrap())
        .next()
        .unwrap_or_else(|| panic!("no control {id:?}"));
    let form = control
        .ancestors()
        .filter_map(scraper::ElementRef::wrap)
        .find(|element| element.value().name() == "form")
        .unwrap_or_else(|| panic!("{id:?} is in no form"));
    let controls = scraper::Selector::parse("input, textarea, select, button").unwrap();
    let option = scraper::Selector::parse("option").unwrap();
    let own = |element: &scraper::ElementRef<'_>| element.value().attr("form").is_none();
    let buttons: Vec<_> = form
        .select(&controls)
        .filter(|element| {
            own(element)
                && element.value().name() == "button"
                && element
                    .value()
                    .attr("type")
                    .is_none_or(|kind| kind == "submit")
        })
        .collect();
    assert_eq!(buttons.len(), 1, "one Save button in the form of {id:?}");
    let mut pairs = Pairs::new();
    for element in form.select(&controls).filter(own) {
        let value = element.value();
        let (Some(name), false) = (value.attr("name"), value.attr("disabled").is_some()) else {
            continue;
        };
        let current = match value.name() {
            "button" => continue,
            "textarea" => element.text().collect::<String>(),
            "select" => {
                let options: Vec<_> = element.select(&option).collect();
                let chosen = options
                    .iter()
                    .find(|option| option.value().attr("selected").is_some())
                    .or(options.first())
                    .expect("an option");
                chosen.value().attr("value").unwrap_or_default().to_owned()
            }
            _ if value.attr("type") == Some("checkbox") => continue,
            _ => value.attr("value").unwrap_or_default().to_owned(),
        };
        let typed = typed.filter(|_| value.attr("id") == Some(id));
        pairs.push((name.to_owned(), typed.map_or(current, str::to_owned)));
    }
    if let Some(name) = buttons[0].value().attr("name") {
        let value = buttons[0].value().attr("value").unwrap_or_default();
        pairs.push((name.to_owned(), value.to_owned()));
    }
    (form.value().attr("action").unwrap().to_owned(), pairs)
}

/// Types `typed` into the control `id` of `html` and presses Enter (or Save).
async fn enter(setup: &Setup, html: &str, id: &str, typed: &str) -> (StatusCode, String) {
    let (action, pairs) = unit_form(html, id, Some(typed));
    setup.post(&action, pairs).await
}

async fn page(setup: &Setup) -> String {
    let (status, html) = setup.get("/private-sheets/1").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    html
}

/// The saved Name, General notes and data of sheet 1.
async fn sheet(setup: &Setup) -> (i64, String, String, Value) {
    let (view, permit) = setup
        .store
        .private_read(
            setup.auth[1].clone(),
            1,
            None,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    drop(permit);
    let snapshot = view.snapshot;
    (
        snapshot.revision,
        snapshot.name,
        snapshot.notes,
        snapshot.data,
    )
}

/// The `base` the unit form of control `id` carries.
fn base_of(html: &str, id: &str) -> String {
    let (_, pairs) = unit_form(html, id, None);
    pairs
        .into_iter()
        .find(|(key, _)| key == "base")
        .map(|(_, value)| value)
        .expect("a base")
}

/// The `value` of the control with DOM id `id`, or its text for a textarea.
fn shown(html: &str, id: &str) -> String {
    let (_, pairs) = unit_form(html, id, None);
    pairs
        .into_iter()
        .find(|(key, _)| key == "value")
        .map(|(_, value)| value)
        .unwrap()
}

/// A unit form as posted by hand: `csrf`, `base`, `value` and the extra pairs.
fn unit(base: &str, value: &str, extra: &[(&str, &str)]) -> Pairs {
    let mut pairs = vec![
        pair("csrf", &"a".repeat(64)),
        pair("base", base),
        pair("value", value),
    ];
    pairs.extend(extra.iter().map(|(key, value)| pair(key, value)));
    pairs
}

/// D-C: Enter in Current HP posts HP's own form only (`csrf`, `base`, `value`) to
/// `fields/hp`, saves HP alone and redirects back to HP's label; every scalar, Name and
/// General notes has its own form with one Save button, and no full-sheet Save is shown.
#[tokio::test]
async fn enter_in_hp_saves_hp_alone() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&choices()).await;
    created(
        &setup,
        &[("value-stance", "guard"), ("value-inspired", "false")],
    )
    .await;
    let html = page(&setup).await;
    assert!(!html.contains("Save full private sheet"), "{html}");
    let (action, pairs) = unit_form(&html, "v0", Some("84 -15"));
    assert_eq!(action, "/private-sheets/1/fields/hp");
    let keys: Vec<&str> = pairs.iter().map(|(key, _)| key.as_str()).collect();
    assert_eq!(keys, ["csrf", "base", "value"]);
    let before = sheet(&setup).await;
    let response = super::router_request(&setup.app, &setup.auth[1], &action, Some(pairs)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers()["location"],
        "/private-sheets/1#label-0",
        "back to HP"
    );
    let after = sheet(&setup).await;
    assert_eq!(after.0, 2);
    let mut expected = before.3.clone();
    expected["hp"] = json!(69);
    assert_eq!(after.3, expected, "only HP changed");
    assert_eq!((after.1, after.2), (before.1, before.2));
    assert_eq!(
        notes_at(&setup, 1, 2).await,
        vec![note(
            2,
            "field:hp",
            "sum",
            "84 -15",
            Some(r#"{"int":"84"}"#),
            r#"{"int":"69"}"#
        )]
    );
    // Each unit has its own form: the nullable Luck adds its state, the select units (enum,
    // boolean) save with their button, Name and General notes post to `meta/…`.
    let html = page(&setup).await;
    assert_eq!(shown(&html, "v0"), "69");
    let (action, pairs) = unit_form(&html, "v3", None);
    assert_eq!(action, "/private-sheets/1/fields/luck");
    let keys: Vec<&str> = pairs.iter().map(|(key, _)| key.as_str()).collect();
    assert_eq!(keys, ["csrf", "base", "null", "value"]);
    assert_eq!(
        unit_form(&html, "v6", None).0,
        "/private-sheets/1/fields/stance"
    );
    assert_eq!(
        unit_form(&html, "v7", None).0,
        "/private-sheets/1/fields/inspired"
    );
    assert_eq!(
        unit_form(&html, "meta-name", None).0,
        "/private-sheets/1/meta/name"
    );
    assert_eq!(
        unit_form(&html, "meta-notes", None).0,
        "/private-sheets/1/meta/notes"
    );
    // The Save buttons are named with their unit's label ("Save Current HP").
    assert!(
        html.contains("<button id=\"save-0\" aria-labelledby=\"save-0 label-0\">Save</button>")
    );
    // The select units and the yes/no unit save with their button.
    assert_eq!(
        enter(&setup, &html, "v7", "true").await.0,
        StatusCode::SEE_OTHER
    );
    let html = page(&setup).await;
    assert_eq!(
        enter(&setup, &html, "v6", &"a".repeat(64)).await.0,
        StatusCode::SEE_OTHER
    );
    let (revision, _, _, data) = sheet(&setup).await;
    assert_eq!(revision, 4);
    assert_eq!(
        (&data["inspired"], &data["stance"]),
        (&json!(true), &json!("a".repeat(64)))
    );
    assert_eq!(data["hp"], json!(69));
}

/// Sol rev-2 M1: a definition field called `name` and the sheet's Name are distinct units
/// (`fields/name`, `meta/name`) and save separately; the multiline field and General notes
/// save with their buttons, keeping newlines.
#[tokio::test]
async fn the_name_field_and_the_metadata_name_save_separately() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&named()).await;
    named_sheet(&setup).await;
    let html = page(&setup).await;
    assert_eq!(
        unit_form(&html, "v0", None).0,
        "/private-sheets/1/fields/name"
    );
    assert_eq!(
        enter(&setup, &html, "v0", "New field text").await.0,
        StatusCode::SEE_OTHER
    );
    let (revision, name, _, data) = sheet(&setup).await;
    assert_eq!((revision, name.as_str()), (2, "Sheet title"));
    assert_eq!(data["name"], json!("New field text"));
    let html = page(&setup).await;
    let response = super::router_request(
        &setup.app,
        &setup.auth[1],
        "/private-sheets/1/meta/name",
        Some(unit_form(&html, "meta-name", Some("  New title  ")).1),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers()["location"],
        "/private-sheets/1#meta-name-label"
    );
    let (revision, name, _, data) = sheet(&setup).await;
    assert_eq!((revision, name.as_str()), (3, "New title"), "trimmed");
    assert_eq!(data["name"], json!("New field text"));
    // Multiline units: the story field and General notes.
    let html = page(&setup).await;
    assert_eq!(
        enter(&setup, &html, "v3", "line one\nline two").await.0,
        StatusCode::SEE_OTHER
    );
    let html = page(&setup).await;
    assert_eq!(
        enter(&setup, &html, "meta-notes", "Camp\nnotes").await.0,
        StatusCode::SEE_OTHER
    );
    let (revision, name, notes_text, data) = sheet(&setup).await;
    assert_eq!(revision, 5);
    assert_eq!(name, "New title");
    assert_eq!(notes_text, "Camp\nnotes");
    assert_eq!(data["story"], json!("line one\nline two"));
    // Text units write no change notes (D-T scope).
    assert_eq!(notes(&setup, 1).await, vec![]);
    // An empty Name is refused with the draft kept (422).
    let html = page(&setup).await;
    let (status, refused) = enter(&setup, &html, "meta-name", "   ").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(refused.contains("Name not saved"), "{refused}");
    assert_eq!(shown(&refused, "meta-name"), "   ");
    assert_eq!(sheet(&setup).await.0, 5);
}

/// Two fields saved from one page revision both succeed: each unit's `base` is its own.
#[tokio::test]
async fn two_fields_saved_from_one_page_both_succeed() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let html = page(&setup).await;
    assert_eq!(
        enter(&setup, &html, "v0", "70").await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        enter(&setup, &html, "v2", "3").await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        enter(&setup, &html, "meta-name", "Renamed").await.0,
        StatusCode::SEE_OTHER
    );
    let (revision, name, _, data) = sheet(&setup).await;
    assert_eq!(revision, 4);
    assert_eq!(name, "Renamed");
    assert_eq!((&data["hp"], &data["armour"]), (&json!(70), &json!(3)));
}

/// A stale base whose value differs is 409: the page is the live revision with the draft in
/// the unit's control (focused) and every `base` at L; a second Enter saves it. A stale
/// no-op answers "No change"; A → B → A is no conflict (values, not history, by design).
#[tokio::test]
async fn a_stale_unit_conflicts_with_its_draft_and_base_l() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let old = page(&setup).await;
    assert_eq!(base_of(&old, "v0"), "1");
    let fresh = page(&setup).await;
    assert_eq!(
        enter(&setup, &fresh, "v0", "70").await.0,
        StatusCode::SEE_OTHER
    );
    let (status, html) = enter(&setup, &old, "v0", "60").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        html.contains(
            "Current HP not saved: it changed since this page was loaded (now revision 2)"
        ),
        "{html}"
    );
    assert_eq!(shown(&html, "v0"), "60", "the draft");
    assert!(
        html.contains("id=\"v0\" name=\"value\" autofocus"),
        "focused"
    );
    for id in ["v0", "v1", "v2", "meta-name", "meta-notes"] {
        assert_eq!(base_of(&html, id), "2", "{id}");
    }
    assert_eq!(shown(&html, "v1"), "84", "other units show the saved value");
    assert_eq!(sheet(&setup).await.3["hp"], json!(70), "nothing written");
    // Pressing Enter again is a conscious overwrite.
    assert_eq!(
        enter(&setup, &html, "v0", "60").await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(sheet(&setup).await.3["hp"], json!(60));
    // A stale page submitting the value already saved: "No change", no revision.
    let (status, html) = enter(&setup, &old, "v0", "60").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("No change: Current HP already holds this value"),
        "{html}"
    );
    assert_eq!(sheet(&setup).await.0, 3);
    // ABA: HP back to 84 (its value at revision 1); the revision-1 page saves HP again.
    let html = page(&setup).await;
    assert_eq!(
        enter(&setup, &html, "v0", "84").await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        enter(&setup, &old, "v0", "50").await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(sheet(&setup).await.3["hp"], json!(50));
    // A stale Name conflicts the same way; a stale General notes too.
    let fresh = page(&setup).await;
    assert_eq!(
        enter(&setup, &fresh, "meta-notes", "Fresh notes").await.0,
        StatusCode::SEE_OTHER
    );
    let (status, html) = enter(&setup, &old, "meta-notes", "Old draft").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(shown(&html, "meta-notes"), "Old draft");
    assert!(
        html.contains("id=\"meta-notes\" name=\"value\" maxlength=\"20000\" rows=\"6\" autofocus")
    );
    assert_eq!(sheet(&setup).await.2, "Fresh notes");
}

/// `base` bounds: malformed or 0 is 400, above L is 409 (forged or from a lost future),
/// each with nothing written.
#[tokio::test]
async fn base_bounds() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    for base in ["0", "-1", "x", "", "01", "9223372036854775808"] {
        let (status, _) = setup
            .post("/private-sheets/1/fields/hp", unit(base, "70", &[]))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "base {base:?}");
    }
    let (status, html) = setup
        .post("/private-sheets/1/fields/hp", unit("2", "70", &[]))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(base_of(&html, "v0"), "1");
    assert_eq!(shown(&html, "v0"), "70");
    let (status, _) = setup
        .post(
            "/private-sheets/1/meta/name",
            unit(&i64::MAX.to_string(), "N", &[]),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(sheet(&setup).await.0, 1, "nothing written");
}

/// Route refusals (400, before any write): derived, list, unknown and non-slug field IDs,
/// unknown metadata, and inexact key sets (a missing or extra key, `null` on a non-nullable
/// field, no `null` on a nullable one).
#[tokio::test]
async fn unit_routes_refuse_what_they_do_not_name() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let refused = [
        ("/private-sheets/1/fields/load", unit("1", "3", &[])),
        ("/private-sheets/1/fields/equipment", unit("1", "3", &[])),
        ("/private-sheets/1/fields/missing", unit("1", "3", &[])),
        ("/private-sheets/1/fields/HP", unit("1", "3", &[])),
        ("/private-sheets/1/fields/hp%20", unit("1", "3", &[])),
        ("/private-sheets/1/meta/title", unit("1", "3", &[])),
        (
            "/private-sheets/1/fields/hp",
            unit("1", "3", &[("null", "known")]),
        ),
        (
            "/private-sheets/1/fields/hp",
            unit("1", "3", &[("revision", "1")]),
        ),
        ("/private-sheets/1/fields/luck", unit("1", "3", &[])),
        (
            "/private-sheets/1/meta/name",
            unit("1", "N", &[("null", "known")]),
        ),
        (
            "/private-sheets/1/fields/hp",
            vec![pair("csrf", &"a".repeat(64)), pair("value", "3")],
        ),
    ];
    for (path, pairs) in refused {
        let (status, _) = setup.post(path, pairs.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path} {pairs:?}");
    }
    assert_eq!(sheet(&setup).await.0, 1, "nothing written");
    // The nullable field with its state is admitted.
    let (status, _) = setup
        .post(
            "/private-sheets/1/fields/luck",
            unit("1", "3", &[("null", "known")]),
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(sheet(&setup).await.3["luck"], json!(3));
}

/// Rights come before keys: a caller without edit rights (an account outside the campaign,
/// or the custodian of a frozen sheet) and a wrong CSRF token get 403, even with a bad key
/// set; nothing is written. History and frozen pages show no unit forms.
#[tokio::test]
async fn unit_routes_recheck_rights_before_keys() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    let player = super::notes_tests::player_sheet(&setup).await;
    let outsider = &setup.auth[0];
    for pairs in [unit("1", "70", &[]), unit("1", "70", &[("extra", "1")])] {
        let response = super::router_request(
            &setup.app,
            outsider,
            "/private-sheets/1/fields/hp",
            Some(pairs),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let history =
        body(super::router_request(&setup.app, &player, "/private-sheets/1/history/1", None).await)
            .await;
    assert!(!history.contains("/fields/"), "history: no unit forms");
    let mut forged = unit("1", "70", &[]);
    forged[0].1 = "b".repeat(64);
    let response = super::router_request(
        &setup.app,
        &player,
        "/private-sheets/1/fields/hp",
        Some(forged),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    // The player may save; then a departure freezes the sheet: 403.
    let response = super::router_request(
        &setup.app,
        &player,
        "/private-sheets/1/fields/hp",
        Some(unit("1", "70", &[])),
    )
    .await;
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
    let response = super::router_request(
        &setup.app,
        &player,
        "/private-sheets/1/meta/name",
        Some(unit("3", "Thawed", &[])),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let frozen =
        body(super::router_request(&setup.app, &player, "/private-sheets/1", None).await).await;
    assert!(!frozen.contains("/meta/name"), "frozen: no unit forms");
    assert_eq!(saved(&setup, 1).await.0, 3, "the edit and the freeze only");
}

/// R1 (plan §3a): tab A at revision 1 has an unsaved Armour edit; tab B saves Armour (2);
/// tab A saves HP (3), which succeeds; tab A's Armour form still has `base` 1, so its save
/// is 409 with the stored value and the draft both kept.
#[tokio::test]
async fn two_tabs_without_script_keep_each_units_base() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let tab_a = page(&setup).await;
    let tab_b = page(&setup).await;
    assert_eq!(
        enter(&setup, &tab_b, "v2", "4").await.0,
        StatusCode::SEE_OTHER
    );
    // Tab A's HP form (base 1) saves: HP did not change since revision 1.
    assert_eq!(
        enter(&setup, &tab_a, "v0", "70").await.0,
        StatusCode::SEE_OTHER
    );
    let (status, html) = enter(&setup, &tab_a, "v2", "-3").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(shown(&html, "v2"), "-3", "the draft is kept");
    assert_eq!(base_of(&html, "v2"), "3");
    let (revision, _, _, data) = sheet(&setup).await;
    assert_eq!(revision, 3);
    assert_eq!(
        (&data["armour"], &data["hp"]),
        (&json!(4), &json!(70)),
        "stored kept"
    );
}

/// The full form POST is kept with its whole-sheet revision check (409) and D-U.
#[tokio::test]
async fn the_full_form_keeps_its_whole_sheet_conflict() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let html = page(&setup).await;
    assert_eq!(
        enter(&setup, &html, "v2", "4").await.0,
        StatusCode::SEE_OTHER
    );
    // A full form of revision 1 changing only HP is refused: the sheet moved on.
    let rope = [row("r1-1", "1")];
    let (status, _) = setup
        .post(
            "/private-sheets/1",
            form(1, &[("value-hp", "70"), ("value-armour", "4")], &rope),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, html) = setup
        .post(
            "/private-sheets/1",
            form(2, &[("value-armour", "4")], &rope),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("No change"), "{html}");
    let (status, _) = setup
        .post(
            "/private-sheets/1",
            form(2, &[("value-hp", "70"), ("value-armour", "4")], &rope),
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(sheet(&setup).await.0, 3);
}

/// D-X3: a unit save waits up to 2 s for the runtime permit. Held for 1 s by another
/// request, the save succeeds; held for 3 s, the answer is the styled 503 page with the
/// player's entry in a form that posts the same unit again (and succeeds once free).
#[tokio::test]
async fn a_unit_save_waits_briefly_for_the_permit() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let html = page(&setup).await;
    let hold = |seconds: u64| {
        let permit = crate::definitions::admit().unwrap();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
            drop(permit);
        })
    };
    let holder = hold(1);
    let started = std::time::Instant::now();
    assert_eq!(
        enter(&setup, &html, "v0", "70").await.0,
        StatusCode::SEE_OTHER
    );
    assert!(started.elapsed() >= std::time::Duration::from_millis(900));
    holder.await.unwrap();
    let html = page(&setup).await;
    let holder = hold(3);
    let started = std::time::Instant::now();
    let response = super::router_request(
        &setup.app,
        &setup.auth[1],
        "/private-sheets/1/fields/luck",
        Some(unit("2", "5 + <b>1</b>", &[("null", "known")])),
    )
    .await;
    let elapsed = started.elapsed();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["retry-after"], "1");
    let busy = body(response).await;
    assert!(
        elapsed >= std::time::Duration::from_millis(1900)
            && elapsed < std::time::Duration::from_secs(3),
        "{elapsed:?}"
    );
    assert!(busy.contains("Server busy: nothing was saved."), "{busy}");
    assert!(
        busy.contains("value=\"5 + &#60;b&#62;1&#60;/b&#62;\""),
        "escaped: {busy}"
    );
    let (action, pairs) = unit_form(&busy, "busy-value", Some("5"));
    assert_eq!(action, "/private-sheets/1/fields/luck");
    assert_eq!(
        pairs,
        vec![
            pair("csrf", &"a".repeat(64)),
            pair("base", "2"),
            pair("null", "known"),
            pair("value", "5"),
        ]
    );
    holder.await.unwrap();
    assert_eq!(sheet(&setup).await.0, 2, "nothing written");
    assert_eq!(setup.post(&action, pairs).await.0, StatusCode::SEE_OTHER);
    assert_eq!(sheet(&setup).await.3["luck"], json!(5));
    // A multiline entry is kept in a textarea.
    let _ = html;
    let holder = hold(3);
    let response = super::router_request(
        &setup.app,
        &setup.auth[1],
        "/private-sheets/1/meta/notes",
        Some(unit("3", "two\nlines", &[])),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let busy = body(response).await;
    assert!(busy.contains("<textarea id=\"busy-value\" name=\"value\" rows=\"6\" autofocus>\ntwo\nlines</textarea>"), "{busy}");
    holder.await.unwrap();
}

/// A slice-1.5a list form (posted by a page loaded before slice 1.5b) still saves its list
/// only: Save list posts that list's rows, so scalar fields keep their saved values and
/// scribbles; its draft operations re-render in the full form, which can save them.
#[tokio::test]
async fn a_list_form_saves_its_list_only() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let html = page(&setup).await;
    assert_eq!(
        enter(&setup, &html, "v0", "lots").await.0,
        StatusCode::SEE_OTHER
    );
    let html = page(&setup).await;
    let (action, mut pairs) = unit_form_list(&html, "row-4-0-1");
    assert_eq!(action, "/private-sheets/1");
    let keys: Vec<&str> = pairs.iter().map(|(key, _)| key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "csrf",
            "revision",
            "list",
            "op",
            "id-4-0",
            "row-4-0-0",
            "row-4-0-1",
            "row-4-0-2",
            "set-4-0-3",
            "row-4-0-3"
        ]
    );
    pairs
        .iter_mut()
        .find(|(key, _)| key == "row-4-0-1")
        .unwrap()
        .1 = "2+1".into();
    assert_eq!(
        setup.post(&action, pairs.clone()).await.0,
        StatusCode::SEE_OTHER
    );
    let (revision, _, _, data) = sheet(&setup).await;
    assert_eq!(revision, 3);
    assert_eq!(data["equipment"][0]["quantity"], json!(3));
    assert_eq!(data["hp"], json!(84));
    // The HP scribble stays current (the list form wrote no note for it).
    let html = page(&setup).await;
    assert_eq!(shown(&html, "v0"), "lots");
    assert!(html.contains("not understood"));
    assert_eq!(
        notes_at(&setup, 1, 3).await,
        vec![note(
            3,
            "cell:equipment/r1-1/quantity",
            "sum",
            "2+1",
            Some(r#"{"int":"1"}"#),
            r#"{"int":"3"}"#
        )]
    );
    // A draft operation re-renders the list draft and leaves the units saved.
    let (_, mut add) = unit_form_list(&html, "row-4-0-1");
    add.iter_mut().find(|(key, _)| key == "op").unwrap().1 = "add-4".into();
    let (status, drafted) = setup.post("/private-sheets/1", add).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        drafted.contains("Row added to Equipment. Not saved yet."),
        "{drafted}"
    );
    assert_eq!(
        super::notes_tests::value(&drafted, "value-hp").as_deref(),
        Some("lots"),
        "the saved scribble, not a draft"
    );
    assert!(drafted.contains("id=\"row-4-1-0\""));
    // Stale list forms keep the whole-sheet 409; forged scopes are 400.
    assert_eq!(
        setup.post("/private-sheets/1", pairs.clone()).await.0,
        StatusCode::CONFLICT
    );
    let mut other = pairs.clone();
    other.iter_mut().find(|(key, _)| key == "list").unwrap().1 = "0".into();
    assert_eq!(
        setup.post("/private-sheets/1", other).await.0,
        StatusCode::BAD_REQUEST
    );
    let mut extra = pairs.clone();
    extra.push(pair("value-hp", "1"));
    assert_eq!(
        setup.post("/private-sheets/1", extra).await.0,
        StatusCode::BAD_REQUEST
    );
    // A list form on the create page is 400 (the create page has only the full form).
    let create = format!(
        "/campaigns/{}/sheets/new?version={}",
        setup.campaign, setup.version
    );
    let mut fresh = pairs.clone();
    fresh
        .iter_mut()
        .find(|(key, _)| key == "revision")
        .unwrap()
        .1 = "0".into();
    fresh.iter_mut().find(|(key, _)| key == "id-4-0").unwrap().1 = String::new();
    assert_eq!(setup.post(&create, fresh).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(sheet(&setup).await.0, 3);
}

/// The slice-1.5a list form of the list owning control `row-<f>-…`, as a page loaded before
/// slice 1.5b posts it with its first button (Save list): `csrf`, `revision`, `list`, `op`
/// and that list's rows. Slice 1.5b pages have a form per cell instead; the route still
/// accepts this form from such pages.
fn unit_form_list(html: &str, id: &str) -> (String, Pairs) {
    let f = id
        .strip_prefix("row-")
        .and_then(|rest| rest.split('-').next())
        .expect("a row control");
    let legacy = super::formula_tests::legacy_form(html);
    let get = |key: &str| {
        legacy
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, value)| value.clone())
            .unwrap()
    };
    let mut pairs = vec![
        pair("csrf", &get("csrf")),
        pair("revision", &get("revision")),
        pair("list", f),
        pair("op", "save"),
    ];
    let prefixes = [format!("id-{f}-"), format!("row-{f}-"), format!("set-{f}-")];
    pairs.extend(legacy.into_iter().filter(|(key, _)| {
        prefixes
            .iter()
            .any(|prefix| key.starts_with(prefix.as_str()))
    }));
    ("/private-sheets/1".into(), pairs)
}

/// The unit page stays without script: unit forms post and redirect (PRG), and a scribble,
/// a sum and a sign-led literal are read and noted as on the full form.
#[tokio::test]
async fn unit_saves_note_numbers_as_the_full_form_does() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let html = page(&setup).await;
    assert_eq!(
        enter(&setup, &html, "v0", "lots").await.0,
        StatusCode::SEE_OTHER
    );
    let html = page(&setup).await;
    assert_eq!(shown(&html, "v0"), "lots");
    // Re-entering the same scribble is no change; the kept value resolves it.
    let (status, again) = enter(&setup, &html, "v0", "lots").await;
    assert_eq!(status, StatusCode::OK);
    assert!(again.contains("No change"));
    assert_eq!(
        enter(&setup, &html, "v0", "84").await.0,
        StatusCode::SEE_OTHER
    );
    let html = page(&setup).await;
    assert_eq!(
        enter(&setup, &html, "v1", "-5").await.0,
        StatusCode::SEE_OTHER
    );
    let html = page(&setup).await;
    assert_eq!(
        super::assist_tests::text(&html, "#offer-1").as_deref(),
        Some("Did you mean 84 \u{2212} 5 = 79? YesHide")
    );
    assert_eq!(
        notes(&setup, 1).await,
        vec![
            note(
                2,
                "field:hp",
                "unresolved",
                "lots",
                Some(r#"{"int":"84"}"#),
                r#"{"int":"84"}"#
            ),
            note(
                3,
                "field:hp",
                "literal",
                "84",
                Some(r#"{"int":"84"}"#),
                r#"{"int":"84"}"#
            ),
            note(
                4,
                "field:max_hp",
                "sign_literal",
                "-5",
                Some(r#"{"int":"84"}"#),
                r#"{"int":"-5"}"#
            ),
        ]
    );
    // Undo and the switch keep working from the unit page.
    let (status, _) = super::assist_tests::click(&setup, &html, "switch-1").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(sheet(&setup).await.3["max_hp"], json!(79));
}

/// P9 (page bound, plan D7): a scribble is quoted under its cell to 32 characters; its
/// control, right above, echoes it in full. A short scribble is quoted whole.
#[tokio::test]
async fn a_long_scribble_is_quoted_to_32_characters_under_its_cell() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let long = format!("{}{}", "a".repeat(32), "b".repeat(40));
    let html = page(&setup).await;
    assert_eq!(
        enter(&setup, &html, "v0", &long).await.0,
        StatusCode::SEE_OTHER
    );
    let html = page(&setup).await;
    assert_eq!(shown(&html, "v0"), long, "the control echoes it in full");
    assert_eq!(
        super::notes_tests::inner(&html, "#note-0").as_deref(),
        Some(format!("kept 84 · ‘<bdi>{}…</bdi>’ not understood", "a".repeat(32)).as_str())
    );
    let exact = "c".repeat(32);
    assert_eq!(
        enter(&setup, &html, "v1", &exact).await.0,
        StatusCode::SEE_OTHER
    );
    let html = page(&setup).await;
    assert_eq!(
        super::notes_tests::inner(&html, "#note-1").as_deref(),
        Some(format!("kept 84 · ‘<bdi>{exact}</bdi>’ not understood").as_str())
    );
}

/// A list form acts on its own list: rows or a draft operation of another list are 400.
#[tokio::test]
async fn a_list_form_cannot_reach_another_list() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&super::list_tests::list_definition()).await;
    let create = vec![
        pair("csrf", &"a".repeat(64)),
        pair("revision", "0"),
        pair("name", "Lists"),
        pair("notes", ""),
        pair("value-label", "x"),
    ];
    let path = format!(
        "/campaigns/{}/sheets/new?version={}",
        setup.campaign, setup.version
    );
    assert_eq!(setup.post(&path, create).await.0, StatusCode::SEE_OTHER);
    // The empty lists' slice-1.5a list forms, as a page loaded before slice 1.5b posts them.
    let list_form = |f: &str, op: &str| {
        vec![
            pair("csrf", &"a".repeat(64)),
            pair("revision", "1"),
            pair("list", f),
            pair("op", op),
        ]
    };
    let (status, added) = setup
        .post("/private-sheets/1", list_form("2", "add-2"))
        .await;
    assert_eq!(status, StatusCode::OK, "Add on its own list");
    assert!(added.contains("id=\"row-2-0-0\""));
    let equipment = list_form("1", "add-1");
    let mut other_op: Pairs = equipment
        .iter()
        .filter(|(key, _)| key != "op")
        .cloned()
        .collect();
    other_op.push(pair("op", "add-2"));
    assert_eq!(
        setup.post("/private-sheets/1", other_op).await.0,
        StatusCode::BAD_REQUEST
    );
    let mut other_rows: Pairs = equipment
        .iter()
        .filter(|(key, _)| key != "op")
        .cloned()
        .collect();
    other_rows.extend([pair("id-2-0", ""), pair("row-2-0-0", "t")]);
    assert_eq!(
        setup.post("/private-sheets/1", other_rows).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(sheet(&setup).await.0, 1, "nothing written");
}

// ---- Review round 1 ----

/// A POST as a browser navigation sends it: `Accept: text/html` and document fetch
/// metadata, so the auth layer's document presentation applies to error pages.
async fn document_post(setup: &Setup, path: &str, pairs: Pairs) -> axum::response::Response {
    use tower::ServiceExt;
    let request = axum::http::Request::builder()
        .uri(path)
        .method("POST")
        .header("host", "127.0.0.1:3000")
        .header("origin", "http://127.0.0.1:3000")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "text/html")
        .header("sec-fetch-mode", "navigate")
        .header("sec-fetch-dest", "document")
        .header("cookie", setup.auth[1].fixture_cookie())
        .body(axum::body::Body::from(
            serde_urlencoded::to_string(pairs).unwrap(),
        ))
        .unwrap();
    setup.app.clone().oneshot(request).await.unwrap()
}

/// Holds the runtime permit for `seconds`, as another request would.
fn hold(seconds: u64) -> tokio::task::JoinHandle<()> {
    let permit = crate::definitions::admit().unwrap();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
        drop(permit);
    })
}

/// Sol Major 1: the busy page reaches a real browser. A document navigation used to get the
/// auth layer's generic busy document, which drops the entry; the styled page is exempt.
/// It names the unit and keeps the escaped entry, the action, `base` and `null`, a
/// multiline entry in a textarea, and posting it again succeeds once the permit is free.
#[tokio::test]
async fn the_busy_page_survives_a_browser_navigation() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let holder = hold(3);
    let response = document_post(
        &setup,
        "/private-sheets/1/fields/luck",
        unit("1", "5 + <b>1</b>", &[("null", "known")]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["retry-after"], "1");
    let busy = body(response).await;
    assert!(busy.contains("Server busy: nothing was saved."), "{busy}");
    assert!(busy.contains("Your entry for the field luck"), "{busy}");
    assert!(
        busy.contains("value=\"5 + &#60;b&#62;1&#60;/b&#62;\""),
        "{busy}"
    );
    let (action, pairs) = unit_form(&busy, "busy-value", Some("5"));
    assert_eq!(action, "/private-sheets/1/fields/luck");
    assert_eq!(
        pairs,
        vec![
            pair("csrf", &"a".repeat(64)),
            pair("base", "1"),
            pair("null", "known"),
            pair("value", "5"),
        ]
    );
    holder.await.unwrap();
    let response = document_post(&setup, &action, pairs).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(sheet(&setup).await.3["luck"], json!(5));
    // A multiline General notes entry, kept in a textarea and named.
    let holder = hold(3);
    let response = document_post(
        &setup,
        "/private-sheets/1/meta/notes",
        unit("2", "two\n<i>lines</i>", &[]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let busy = body(response).await;
    assert!(busy.contains("Your entry for General notes"), "{busy}");
    assert!(
        busy.contains(
            "<textarea id=\"busy-value\" name=\"value\" rows=\"6\" autofocus>\ntwo\n&#60;i&#62;lines&#60;/i&#62;</textarea>"
        ),
        "{busy}"
    );
    holder.await.unwrap();
    let (action, pairs) = unit_form(&busy, "busy-value", None);
    assert_eq!(action, "/private-sheets/1/meta/notes");
    assert_eq!(
        document_post(&setup, &action, pairs).await.status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(sheet(&setup).await.2, "two\n<i>lines</i>");
}

/// The other answers that keep the draft reach a browser too: the 409 and 422 re-renders
/// (the trusted editor page) and the stale "No change" (200).
#[tokio::test]
async fn refused_unit_saves_keep_their_draft_in_a_browser() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let response = document_post(&setup, "/private-sheets/1/fields/hp", unit("9", "60", &[])).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let html = body(response).await;
    assert!(html.contains("Current HP not saved"), "{html}");
    assert_eq!(shown(&html, "v0"), "60");
    let response = document_post(
        &setup,
        "/private-sheets/1/fields/hp",
        unit("1", "6\u{7}", &[]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = body(response).await;
    assert!(html.contains("Current HP not saved"), "{html}");
    let response = document_post(&setup, "/private-sheets/1/fields/hp", unit("1", "84", &[])).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body(response).await.contains("No change"));
}

/// Sonnet M1: a list form (the full-form route) waits for the permit like a unit save. Held
/// for 1 s, the save succeeds; held for 3 s, the styled busy page keeps every submitted
/// pair and posting it again saves the list.
#[tokio::test]
async fn a_list_form_save_waits_briefly_for_the_permit() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let html = page(&setup).await;
    let (action, mut pairs) = unit_form_list(&html, "row-4-0-1");
    pairs
        .iter_mut()
        .find(|(key, _)| key == "row-4-0-1")
        .unwrap()
        .1 = "2".into();
    let holder = hold(1);
    let response = document_post(&setup, &action, pairs.clone()).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    holder.await.unwrap();
    assert_eq!(sheet(&setup).await.3["equipment"][0]["quantity"], json!(2));
    let html = page(&setup).await;
    let (action, mut pairs) = unit_form_list(&html, "row-4-0-1");
    pairs
        .iter_mut()
        .find(|(key, _)| key == "row-4-0-0")
        .unwrap()
        .1 = "Rope \"long\"\nend".into();
    let holder = hold(3);
    let response = document_post(&setup, &action, pairs.clone()).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["retry-after"], "1");
    let busy = body(response).await;
    assert!(busy.contains("Server busy: nothing was saved."), "{busy}");
    assert!(busy.contains("Your changes are kept"), "{busy}");
    let (again_action, again) = super::formula_tests::form_submission(&busy);
    assert_eq!(again_action, "/private-sheets/1");
    let mut sent = pairs.clone();
    sent.sort();
    let mut kept = again.clone();
    kept.sort();
    assert_eq!(kept, sent, "every pair kept");
    holder.await.unwrap();
    assert_eq!(sheet(&setup).await.0, 2, "nothing written");
    assert_eq!(
        document_post(&setup, &again_action, again).await.status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        sheet(&setup).await.3["equipment"][0]["name"],
        json!("Rope \"long\"\nend")
    );
}

/// Sonnet m2: every Save button has a real accessible name, its own text and its unit's
/// label ("Save Current HP"), through `aria-labelledby`; so do the metadata and the row
/// cells (slice 1.5b: "Save Quantity Equipment Row 1").
#[tokio::test]
async fn save_buttons_are_named_by_their_unit() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let html = page(&setup).await;
    for (button, labelled) in [
        ("save-0", "save-0 label-0"),
        ("save-3", "save-3 label-3"),
        ("save-meta-name", "save-meta-name meta-name-label"),
        ("save-meta-notes", "save-meta-notes meta-notes-label"),
        (
            "save-4-0-1",
            "save-4-0-1 label-4-0-1 list-legend-4 legend-4-0",
        ),
    ] {
        let page = scraper::Html::parse_document(&html);
        let element = page
            .select(&scraper::Selector::parse(&format!("button#{button}")).unwrap())
            .next()
            .unwrap_or_else(|| panic!("{button}"));
        assert_eq!(element.value().attr("aria-labelledby"), Some(labelled));
        assert_eq!(element.value().attr("aria-describedby"), None);
    }
    // Sonnet m4: Enter in HP activates HP's own Save, its form's default submitter.
    let page = scraper::Html::parse_document(&html);
    let form = page
        .select(&scraper::Selector::parse("form[action$=\"/fields/hp\"]").unwrap())
        .next()
        .unwrap();
    let default = form
        .select(&scraper::Selector::parse("button:not([form])").unwrap())
        .next()
        .unwrap();
    assert_eq!(default.value().attr("id"), Some("save-0"));
}

// ---- Review round 2 ----

/// Sol Minor 1 (round 2): the edit save parses and checks CSRF and the session before it
/// waits for the permit, so a forged token is refused at once while the server is busy.
#[tokio::test]
async fn a_forged_edit_is_refused_at_once_while_busy() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let html = page(&setup).await;
    let (action, mut pairs) = unit_form_list(&html, "row-4-0-1");
    pairs[0].1 = "b".repeat(64);
    let holder = hold(3);
    let started = std::time::Instant::now();
    let response = document_post(&setup, &action, pairs).await;
    let elapsed = started.elapsed();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(elapsed < std::time::Duration::from_secs(1), "{elapsed:?}");
    holder.await.unwrap();
}

/// Sonnet F3 (round 2): the busy page's echo bound counts keys and per-pair markup too. A
/// form of 24,000 unique 70-byte quote keys is not kept (the page says so and stays small);
/// a form within the bound is kept, and the page stays within the fixed chrome plus five
/// times the echo bound (plan D7).
#[tokio::test]
async fn the_busy_page_of_a_form_is_bounded() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let mut wide = vec![pair("csrf", &"a".repeat(64))];
    wide.extend((0..24_000).map(|k| pair(&format!("{}{k:06}", "\"".repeat(64)), "")));
    let holder = hold(3);
    let response = document_post(&setup, "/private-sheets/1", wide).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let busy = body(response).await;
    holder.await.unwrap();
    assert!(
        busy.contains("larger than this page can keep"),
        "{}",
        &busy[..2000]
    );
    assert!(busy.len() < 4096, "{} bytes", busy.len());
    // Many short pairs: 180,000 bytes of keys, within the bound alone, but their markup takes
    // the page over it, so the per-pair charge drops the draft (review round 3, Sol).
    let mut short = vec![pair("csrf", &"a".repeat(64))];
    short.extend((0..30_000).map(|k| pair(&format!("k{k:05}"), "")));
    let holder = hold(3);
    let response = document_post(&setup, "/private-sheets/1", short).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let busy = body(response).await;
    holder.await.unwrap();
    assert!(busy.contains("larger than this page can keep"));
    assert!(busy.len() < 4096, "{} bytes", busy.len());
    // Within the bound: one value of 1,000,000 quotes is kept, escaped once.
    let quotes = "\"".repeat(1_000_000);
    let kept = vec![
        pair("csrf", &"a".repeat(64)),
        pair("revision", "1"),
        pair("notes", &quotes),
    ];
    let holder = hold(3);
    let response = document_post(&setup, "/private-sheets/1", kept).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let busy = String::from_utf8(
        axum::body::to_bytes(response.into_body(), 16 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    holder.await.unwrap();
    assert!(busy.contains("Your changes are kept"));
    assert_eq!(busy.matches(&"&#34;".repeat(1_000_000)).count(), 1);
    let bound = super::list_tests::FIXED_PAGE_BYTES + 5 * super::super::web::ECHO_BYTES;
    assert!(busy.len() <= bound, "{} > {bound}", busy.len());
}
