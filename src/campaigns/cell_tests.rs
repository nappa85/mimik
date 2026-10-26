//! Slice 1.5b "Per-cell save: lists, new row, row actions" (`docs/assisted-entry.md`, "Saving
//! a saved sheet"; D-C, D-S, D-V, D-M, D-X3): each saved row cell
//! saves on its own (`cells/{f}/{r}/{p}`, the row named by its index at the form's `base`),
//! a new-row form adds one row (`lists/{f}/rows`), and each row has Remove, Move up and Move
//! down (`lists/{f}/rows/{r}/{remove|up|down}`). Remove protects the row's values and is
//! undone right after ("Row removed · Undo"); Move protects the order.
use super::formula_tests::{Setup, body, setup};
use super::notes_tests::{Row, assist, create_path, form, note, notes_at, saved};
use super::unit_tests::unit_form;
use axum::http::StatusCode;
use serde_json::{Value, json};

type Pairs = Vec<(String, String)>;

fn pair(key: &str, value: &str) -> (String, String) {
    (key.into(), value.into())
}

pub(super) fn item<'a>(id: &'a str, name: &'a str, quantity: &'a str) -> Row<'a> {
    Row {
        id,
        name,
        quantity,
        broken: "",
        set: "absent",
    }
}

/// Sheet 1 at revision 1 with the rows Axe (1), Bow (2) and Cup (3): `r1-1`, `r1-2`, `r1-3`.
pub(super) async fn three_rows(setup: &Setup) {
    let rows = [
        item("", "Axe", "1"),
        item("", "Bow", "2"),
        item("", "Cup", "3"),
    ];
    let (status, html) = setup.post(&create_path(setup), form(0, &[], &rows)).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
}

pub(super) async fn page(setup: &Setup) -> String {
    let (status, html) = setup.get("/private-sheets/1").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    html
}

/// The rows of the saved Equipment list as (ID, name, quantity).
pub(super) async fn rows(setup: &Setup) -> Vec<(String, String, Value)> {
    saved(setup, 1).await.1["equipment"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["id"].as_str().unwrap().to_owned(),
                row["name"].as_str().unwrap().to_owned(),
                row["quantity"].clone(),
            )
        })
        .collect()
}

fn keys(pairs: &Pairs) -> Vec<&str> {
    pairs.iter().map(|(key, _)| key.as_str()).collect()
}

/// Types `typed` into the cell control `id` and presses Enter.
async fn enter(setup: &Setup, html: &str, id: &str, typed: &str) -> (StatusCode, String) {
    let (action, pairs) = unit_form(html, id, Some(typed));
    setup.post(&action, pairs).await
}

/// A row action button (`remove-4-1`, `up-4-2`): it belongs to the page's shared row-action
/// form (`form`), which posts its `csrf` and `base` to the button's `formaction`.
pub(super) fn row_action(html: &str, id: &str) -> (String, Pairs) {
    let page = scraper::Html::parse_document(html);
    let button = page
        .select(&scraper::Selector::parse(&format!("button[id=\"{id}\"]")).unwrap())
        .next()
        .unwrap_or_else(|| panic!("no button {id:?}"));
    assert!(
        button.value().attr("disabled").is_none(),
        "{id} is disabled"
    );
    assert_eq!(button.value().attr("name"), None, "{id} adds no pair");
    let owner = button.value().attr("form").expect("a shared form");
    let shared = page
        .select(&scraper::Selector::parse(&format!("form[id=\"{owner}\"]")).unwrap())
        .next()
        .expect("the shared form");
    let inputs = scraper::Selector::parse("input").unwrap();
    let pairs = shared
        .select(&inputs)
        .map(|input| {
            (
                input.value().attr("name").unwrap().to_owned(),
                input.value().attr("value").unwrap_or_default().to_owned(),
            )
        })
        .collect();
    (button.value().attr("formaction").unwrap().to_owned(), pairs)
}

pub(super) async fn press(setup: &Setup, html: &str, id: &str) -> (StatusCode, String) {
    let (action, pairs) = row_action(html, id);
    setup.post(&action, pairs).await
}

/// The new-row form of list `f`: its action and what Enter in its first control submits,
/// with `typed` values put into its `row-<p>` / `set-<p>` controls.
pub(super) fn new_row(html: &str, f: usize, typed: &[(&str, &str)]) -> (String, Pairs) {
    let page = scraper::Html::parse_document(html);
    let form = page
        .select(&scraper::Selector::parse(&format!("form[action$=\"/lists/{f}/rows\"]")).unwrap())
        .next()
        .unwrap_or_else(|| panic!("no new-row form for list {f}"));
    let controls = scraper::Selector::parse("input, select, textarea, button").unwrap();
    let option = scraper::Selector::parse("option").unwrap();
    let mut buttons = 0;
    let mut pairs = Pairs::new();
    for control in form.select(&controls) {
        let element = control.value();
        if element.name() == "button" {
            buttons += 1;
            continue;
        }
        let name = element.attr("name").unwrap().to_owned();
        let current = match element.name() {
            "select" => control
                .select(&option)
                .find(|option| option.value().attr("selected").is_some())
                .or_else(|| control.select(&option).next())
                .and_then(|option| option.value().attr("value"))
                .unwrap_or_default()
                .to_owned(),
            "textarea" => control.text().collect(),
            _ => element.attr("value").unwrap_or_default().to_owned(),
        };
        let value = typed
            .iter()
            .find(|(key, _)| *key == name)
            .map_or(current, |(_, value)| (*value).to_owned());
        pairs.push((name, value));
    }
    assert_eq!(buttons, 1, "one Add button");
    (form.value().attr("action").unwrap().to_owned(), pairs)
}

fn text(html: &str, selector: &str) -> Option<String> {
    super::assist_tests::text(html, selector)
}

/// The accessible name `aria-labelledby` gives the button `id`: the texts of its ids.
pub(super) fn name(html: &str, id: &str) -> String {
    let page = scraper::Html::parse_document(html);
    let button = page
        .select(&scraper::Selector::parse(&format!("button[id=\"{id}\"]")).unwrap())
        .next()
        .unwrap_or_else(|| panic!("no button {id:?}"));
    button
        .value()
        .attr("aria-labelledby")
        .unwrap_or_else(|| panic!("{id} has no aria-labelledby"))
        .split(' ')
        .map(|part| text(html, &format!("[id=\"{part}\"]")).unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ")
}

/// D-C: Enter in the Quantity of the first, middle and last row posts that cell's own form
/// (`csrf`, `base`, `value`) to `cells/4/<r>/1`, saves that cell alone and never adds, moves
/// or removes a row; an optional cell adds its `set`.
#[tokio::test]
async fn enter_in_a_rows_quantity_saves_that_cell_alone() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    three_rows(&setup).await;
    let ids = ["r1-1", "r1-2", "r1-3"];
    for r in 0..3_usize {
        let html = page(&setup).await;
        assert!(!html.contains("Save list"), "no list form");
        let (action, pairs) =
            unit_form(&html, &format!("row-4-{r}-1"), Some(&format!("{}", 10 + r)));
        assert_eq!(action, format!("/private-sheets/1/cells/4/{r}/1"));
        assert_eq!(keys(&pairs), ["csrf", "base", "value"]);
        let before = rows(&setup).await;
        let response =
            super::router_request(&setup.app, &setup.auth[1], &action, Some(pairs)).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers()["location"],
            format!("/private-sheets/1#label-4-{r}-1").as_str()
        );
        let after = rows(&setup).await;
        let mut expected = before.clone();
        expected[r].2 = json!(10 + r);
        assert_eq!(after, expected, "row {r}: only its Quantity changed");
        let order: Vec<&str> = after.iter().map(|row| row.0.as_str()).collect();
        assert_eq!(order, ids, "no row added, moved or removed");
    }
    assert_eq!(
        notes_at(&setup, 1, 3).await,
        vec![note(
            3,
            "cell:equipment/r1-2/quantity",
            "literal",
            "11",
            Some(r#"{"int":"2"}"#),
            r#"{"int":"11"}"#
        )]
    );
    // The optional Broken cell posts its Set/Not set state with its value.
    let html = page(&setup).await;
    let (action, mut pairs) = unit_form(&html, "row-4-2-3", None);
    assert_eq!(action, "/private-sheets/1/cells/4/2/3");
    assert_eq!(keys(&pairs), ["csrf", "base", "set", "value"]);
    pairs[2].1 = "present".into();
    pairs[3].1 = "true".into();
    assert_eq!(setup.post(&action, pairs).await.0, StatusCode::SEE_OTHER);
    let data = saved(&setup, 1).await.1;
    assert_eq!(data["equipment"][2]["broken"], json!(true));
    assert!(data["equipment"][1].get("broken").is_none());
    // A sum in a cell is read as on the full form.
    let html = page(&setup).await;
    assert_eq!(
        enter(&setup, &html, "row-4-0-2", "2+1").await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(saved(&setup, 1).await.1["equipment"][0]["weight"], json!(3));
    let html = page(&setup).await;
    assert_eq!(text(&html, "#note-4-0-2").as_deref(), Some("2 + 1 = 3"));
}

/// Two cells of different rows saved from one page both succeed: each cell's `base` is
/// its own, and a cell of another row changed since is not a conflict.
#[tokio::test]
async fn two_cells_of_different_rows_saved_from_one_page_both_succeed() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    three_rows(&setup).await;
    let html = page(&setup).await;
    assert_eq!(
        enter(&setup, &html, "row-4-0-1", "5").await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        enter(&setup, &html, "row-4-2-0", "Cups").await.0,
        StatusCode::SEE_OTHER
    );
    let after = rows(&setup).await;
    assert_eq!(after[0].2, json!(5));
    assert_eq!(after[2].1, "Cups");
    assert_eq!(saved(&setup, 1).await.0, 3);
}

/// A stale cell (its value changed between `base` and L) is 409: the live page with the
/// draft in that cell's control (focused) and every form's `base` at L; Enter again saves.
/// A stale page submitting the saved value is "No change". A row moved since is found by
/// its host ID at `base`, so its cell still saves.
#[tokio::test]
async fn a_stale_cell_conflicts_with_its_draft_and_base_l() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    three_rows(&setup).await;
    let old = page(&setup).await;
    let fresh = page(&setup).await;
    assert_eq!(
        enter(&setup, &fresh, "row-4-1-1", "7").await.0,
        StatusCode::SEE_OTHER
    );
    let (status, html) = enter(&setup, &old, "row-4-1-1", "9").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        html.contains(
            "Quantity, row 2 of Equipment not saved: it changed since this page was loaded (now revision 2)"
        ),
        "{html}"
    );
    let (_, pairs) = unit_form(&html, "row-4-1-1", None);
    assert_eq!(pairs[2], pair("value", "9"), "the draft");
    assert!(html.contains("id=\"row-4-1-1\" name=\"value\" value=\"9\" autofocus"));
    for id in ["row-4-0-1", "row-4-1-1", "row-4-2-0", "v0", "meta-name"] {
        let (_, pairs) = unit_form(&html, id, None);
        assert_eq!(pairs[1], pair("base", "2"), "{id}");
    }
    assert_eq!(rows(&setup).await[1].2, json!(7), "nothing written");
    assert_eq!(
        enter(&setup, &html, "row-4-1-1", "9").await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(rows(&setup).await[1].2, json!(9));
    // A stale no-op: "No change", no revision.
    let (status, html) = enter(&setup, &old, "row-4-1-1", "9").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("No change: Quantity, row 2 of Equipment already holds this value"),
        "{html}"
    );
    assert_eq!(saved(&setup, 1).await.0, 3);
    // Another tab moves Axe down; the old page's Axe Quantity (row 1 at its base) still
    // names Axe, which now sits in row 2.
    let fresh = page(&setup).await;
    assert_eq!(
        press(&setup, &fresh, "down-4-0").await.0,
        StatusCode::SEE_OTHER
    );
    let (status, _) = enter(&setup, &old, "row-4-0-1", "4").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let after = rows(&setup).await;
    assert_eq!(
        (
            after[0].0.as_str(),
            &after[0].2,
            after[1].0.as_str(),
            &after[1].2
        ),
        ("r1-2", &json!(9), "r1-1", &json!(4))
    );
}

/// Remove protects the row's values (plan §3a): a stale Remove after another tab edited
/// that row is 409 and removes nothing; a stale Remove of an untouched row removes it; a
/// second Remove of a removed row is "No change".
#[tokio::test]
async fn a_stale_remove_after_another_tab_edited_that_row_conflicts() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    three_rows(&setup).await;
    let old = page(&setup).await;
    let fresh = page(&setup).await;
    assert_eq!(
        enter(&setup, &fresh, "row-4-1-0", "Longbow").await.0,
        StatusCode::SEE_OTHER
    );
    let (status, html) = press(&setup, &old, "remove-4-1").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        html.contains("Row 2 of Equipment not removed: the list changed since this page was loaded (now revision 2)"),
        "{html}"
    );
    assert_eq!(rows(&setup).await.len(), 3, "nothing removed");
    // Axe was not edited: the old page removes it.
    let (status, _) = press(&setup, &old, "remove-4-0").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let ids: Vec<String> = rows(&setup).await.into_iter().map(|row| row.0).collect();
    assert_eq!(ids, ["r1-2", "r1-3"]);
    // Pressing it again (a double submit) is "No change".
    let (status, html) = press(&setup, &old, "remove-4-0").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("No change: Row 1 of Equipment was already removed"),
        "{html}"
    );
    assert_eq!(saved(&setup, 1).await.0, 3);
}

/// Move protects the list's order: a stale Move after a reorder, an add or a remove is 409;
/// from a fresh page it swaps the rows and writes no note; an edge move is 400.
#[tokio::test]
async fn a_stale_move_after_a_reorder_add_or_remove_conflicts() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    three_rows(&setup).await;
    let old = page(&setup).await;
    // A move from a fresh page: Bow up (revision 2), no note.
    let (status, _) = press(&setup, &page(&setup).await, "up-4-1").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let ids: Vec<String> = rows(&setup).await.into_iter().map(|row| row.0).collect();
    assert_eq!(ids, ["r1-2", "r1-1", "r1-3"]);
    assert_eq!(notes_at(&setup, 1, 2).await, vec![]);
    // After a reorder: the old page's Move is stale.
    let (status, html) = press(&setup, &old, "down-4-1").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        html.contains("Row 2 of Equipment not moved: the list changed since this page was loaded (now revision 2)"),
        "{html}"
    );
    // After an add.
    let old = page(&setup).await;
    let (action, pairs) = new_row(&page(&setup).await, 4, &[("row-0", "Dart")]);
    assert_eq!(setup.post(&action, pairs).await.0, StatusCode::SEE_OTHER);
    assert_eq!(
        press(&setup, &old, "down-4-0").await.0,
        StatusCode::CONFLICT
    );
    // After a remove.
    let old = page(&setup).await;
    assert_eq!(
        press(&setup, &page(&setup).await, "remove-4-3").await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(press(&setup, &old, "up-4-2").await.0, StatusCode::CONFLICT);
    let ids: Vec<String> = rows(&setup).await.into_iter().map(|row| row.0).collect();
    assert_eq!(ids, ["r1-2", "r1-1", "r1-3"], "no stale move applied");
    // The first row's Move up and the last row's Move down are disabled; forged, 400.
    let html = page(&setup).await;
    for (id, path) in [
        ("up-4-0", "/private-sheets/1/lists/4/rows/0/up"),
        ("down-4-2", "/private-sheets/1/lists/4/rows/2/down"),
    ] {
        assert!(html.contains(&format!("id=\"{id}\"")));
        assert!(
            scraper::Html::parse_document(&html)
                .select(&scraper::Selector::parse(&format!("#{id}[disabled]")).unwrap())
                .next()
                .is_some(),
            "{id} disabled"
        );
        let base = saved(&setup, 1).await.0.to_string();
        let pairs = vec![pair("csrf", &"a".repeat(64)), pair("base", &base)];
        assert_eq!(
            setup.post(path, pairs).await.0,
            StatusCode::BAD_REQUEST,
            "{id}"
        );
    }
}

/// Routes refuse what they do not name, with 400 and nothing written: a row missing at
/// `base`, non-canonical or out-of-range indices, a list index that
/// is a scalar or derived field, and key sets other than the cell's.
#[tokio::test]
async fn list_routes_refuse_what_they_do_not_name() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    three_rows(&setup).await;
    let old = page(&setup).await;
    assert_eq!(
        press(&setup, &page(&setup).await, "remove-4-1").await.0,
        StatusCode::SEE_OTHER
    );
    // A removed row's cell is a conflict that keeps the draft (review round 1, Sonnet);
    // see `a_cell_whose_row_was_removed_keeps_its_draft`.
    assert_eq!(
        enter(&setup, &old, "row-4-1-1", "5").await.0,
        StatusCode::CONFLICT
    );
    let cell = |base: &str, value: &str| {
        vec![
            pair("csrf", &"a".repeat(64)),
            pair("base", base),
            pair("value", value),
        ]
    };
    for path in [
        "/private-sheets/1/cells/4/5/1",
        "/private-sheets/1/cells/4/01/1",
        "/private-sheets/1/cells/4/0/9",
        "/private-sheets/1/cells/0/0/0",
        "/private-sheets/1/cells/5/0/0",
        "/private-sheets/1/cells/9/0/0",
        "/private-sheets/1/cells/x/0/0",
        "/private-sheets/1/lists/0/rows/0/remove",
        "/private-sheets/1/lists/4/rows/0/delete",
        "/private-sheets/1/lists/4/rows/9/remove",
    ] {
        assert_eq!(
            setup.post(path, cell("2", "5")).await.0,
            StatusCode::BAD_REQUEST,
            "{path}"
        );
    }
    let mut extra = cell("2", "5");
    extra.push(pair("set", "present"));
    let mut missing = cell("2", "5");
    missing.pop();
    for pairs in [extra, missing] {
        assert_eq!(
            setup.post("/private-sheets/1/cells/4/0/1", pairs).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    // A row action takes exactly `csrf` and `base`; a new row its own row keys.
    assert_eq!(
        setup
            .post("/private-sheets/1/lists/4/rows/0/remove", cell("2", "5"))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let mut row = vec![pair("csrf", &"a".repeat(64)), pair("base", "2")];
    row.extend(["row-0", "row-1", "row-2"].map(|key| pair(key, "1")));
    assert_eq!(
        setup.post("/private-sheets/1/lists/4/rows", row).await.0,
        StatusCode::BAD_REQUEST,
        "set-3 missing"
    );
    // `base` bounds: malformed or 0 is 400, above L is 409.
    for base in ["0", "x", "01"] {
        assert_eq!(
            setup
                .post("/private-sheets/1/cells/4/0/1", cell(base, "5"))
                .await
                .0,
            StatusCode::BAD_REQUEST,
            "{base}"
        );
    }
    assert_eq!(
        setup
            .post("/private-sheets/1/cells/4/0/1", cell("9", "5"))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(saved(&setup, 1).await.0, 2, "only the removal");
}

/// D-V: Remove happens at once and the page shows "Row removed · Undo" right after; Undo
/// re-inserts the row from the revision before the removal, at its old index, with a new
/// host ID (removed IDs never return), and the line goes away. A second Undo is 409.
#[tokio::test]
async fn remove_then_undo_restores_the_row_with_a_new_host_id() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    three_rows(&setup).await;
    let html = page(&setup).await;
    assert_eq!(text(&html, "#removed-4"), None, "nothing to undo yet");
    let response = {
        let (action, pairs) = row_action(&html, "remove-4-1");
        assert_eq!(action, "/private-sheets/1/lists/4/rows/1/remove");
        assert_eq!(keys(&pairs), ["csrf", "base"]);
        super::router_request(&setup.app, &setup.auth[1], &action, Some(pairs)).await
    };
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers()["location"],
        "/private-sheets/1#removed-4"
    );
    let ids: Vec<String> = rows(&setup).await.into_iter().map(|row| row.0).collect();
    assert_eq!(ids, ["r1-1", "r1-3"]);
    assert_eq!(
        notes_at(&setup, 1, 2).await,
        vec![note(
            2,
            "row:equipment/r1-2",
            "literal",
            "",
            Some(r#"{"row":"present"}"#),
            r#"{"row":"absent"}"#
        )]
    );
    let html = page(&setup).await;
    assert_eq!(
        text(&html, "#removed-4").as_deref(),
        Some("Row removed · Undo")
    );
    assert_eq!(name(&html, "undo-row-4"), "Undo Row removed Equipment");
    let (status, _) = super::assist_tests::click(&setup, &html, "undo-row-4").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let after = rows(&setup).await;
    assert_eq!(
        after,
        [
            ("r1-1".into(), "Axe".into(), json!(1)),
            ("r3-1".into(), "Bow".into(), json!(2)),
            ("r1-3".into(), "Cup".into(), json!(3)),
        ]
    );
    let undo = notes_at(&setup, 1, 3).await;
    assert_eq!(
        undo,
        vec![note(
            3,
            "row:equipment/r1-2",
            "reverted",
            "",
            Some(r#"{"row":"absent"}"#),
            r#"{"row":"present"}"#
        )]
    );
    let target: Option<i64> = setup
        .store
        .run(|db| {
            Ok(db.query_row(
                "SELECT target FROM private_sheet_notes WHERE sheet_id=1 AND revision=3",
                [],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(target, Some(2));
    let fresh = page(&setup).await;
    assert_eq!(text(&fresh, "#removed-4"), None, "undone");
    // The restored row is a new row for notes: its cells offer no Undo.
    assert!(!fresh.contains("id=\"undo-4-1-1\""));
    // A second Undo from the page that offered it: nothing to undo.
    let (status, again) = super::assist_tests::click(&setup, &html, "undo-row-4").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        again.contains("Nothing undone: the row removed from Equipment changed since, or this change was already undone."),
        "{again}"
    );
    assert_eq!(saved(&setup, 1).await.0, 3);
}

/// Undo of a removal re-inserts the row at its old index clamped to the current length:
/// the last row removed, then the list shortened (by the full form, which writes no row
/// note), comes back at the end.
#[tokio::test]
async fn undo_of_remove_clamps_the_old_index() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    three_rows(&setup).await;
    assert_eq!(
        press(&setup, &page(&setup).await, "remove-4-2").await.0,
        StatusCode::SEE_OTHER
    );
    let (status, html) = setup
        .post(
            "/private-sheets/1",
            form(2, &[], &[item("r1-2", "Bow", "2")]),
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
    let html = page(&setup).await;
    let (status, _) = super::assist_tests::click(&setup, &html, "undo-row-4").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let after = rows(&setup).await;
    assert_eq!(
        after,
        [
            ("r1-2".into(), "Bow".into(), json!(2)),
            ("r4-1".into(), "Cup".into(), json!(3)),
        ]
    );
}

/// The legacy full form of `rows` saved rows (create), for lists of any length.
pub(super) fn many(rows: usize) -> Vec<Row<'static>> {
    (0..rows).map(|_| item("", "Item", "1")).collect()
}

/// Undo of a removal into a full list (256 rows) is refused with a message; the full list's
/// page offers no new-row form, and a forged new row is refused too.
#[tokio::test]
async fn undo_of_remove_into_a_full_list_is_refused_with_a_message() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    let (status, html) = setup
        .post(&create_path(&setup), form(0, &[], &many(256)))
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
    let full = page(&setup).await;
    assert!(full.contains("This list holds 256 rows, the most a list can hold."));
    assert!(!full.contains("/lists/4/rows\""), "no new-row form");
    assert_eq!(
        press(&setup, &full, "remove-4-0").await.0,
        StatusCode::SEE_OTHER
    );
    let (action, pairs) = new_row(&page(&setup).await, 4, &[("row-0", "Last")]);
    assert_eq!(
        setup.post(&action, pairs.clone()).await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(rows(&setup).await.len(), 256);
    let html = page(&setup).await;
    let (status, refused) = super::assist_tests::click(&setup, &html, "undo-row-4").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        refused.contains("Nothing changed: This list already holds 256 rows, the most a list can hold; remove a row first, then Undo."),
        "{refused}"
    );
    assert_eq!(saved(&setup, 1).await.0, 3, "nothing written");
    let mut forged = pairs;
    forged[1].1 = "3".into();
    let (status, refused) = setup.post(&action, forged).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        refused.contains("This list already holds 256 rows, the most a list can hold."),
        "{refused}"
    );
}

/// A list with a required enum and yes/no, an optional yes/no and a number, as the new-row
/// tests need.
pub(super) fn gear() -> Value {
    let unresolved = json!({"state": "unresolved", "reason": "Synthetic fixture"});
    let property = |id: &str, label: &str, control: &str, optional: bool| json!({"id": id, "label": label, "control": control, "optional": optional});
    let mut kind = property("kind", "Kind", "enum", false);
    kind["options"] = json!([{"id": "tool", "label": "Tool"}, {"id": "arm", "label": "Arm"}]);
    json!({
        "contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
        "definition_id": "gear", "title": "Gear",
        "sources": [{"source_id": 1, "pdf_sha256": "0".repeat(64), "roles": ["sheet"]}],
        "fields": [
            {"id": "title", "label": "Title", "control": "text", "nullable": false,
             "mapping": {"state": "mapped", "regions": [{"source_id": 1, "page_id": 1, "unit_id": 1}]}},
            {"id": "gear", "label": "Gear", "control": "list", "nullable": false,
             "mapping": unresolved, "items": {"properties": [
                property("name", "Name", "text", false),
                property("count", "Count", "integer", false),
                kind,
                property("ready", "Ready", "boolean", false),
                property("worn", "Worn", "boolean", true),
             ]}},
        ],
        "formula_profile": 1,
        "groups": [{"id": "main", "label": "Main", "fields": ["title", "gear"], "column_hint": "main"}],
    })
}

pub(super) async fn gear_sheet(setup: &Setup) {
    let create = vec![
        pair("csrf", &"a".repeat(64)),
        pair("revision", "0"),
        pair("name", "Gear sheet"),
        pair("notes", ""),
        pair("value-title", "t"),
    ];
    let (status, html) = setup.post(&create_path(setup), create).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
}

/// The new-row form: one row's controls, Enter adds it with the host ID `r<L+1>-1` and
/// returns to the new row. A required enum or yes/no left unset is refused with the draft
/// kept; a blank number is unknown and an unreadable one unknown with its scribble (D-M).
#[tokio::test]
async fn the_new_row_form_adds_one_row() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&gear()).await;
    gear_sheet(&setup).await;
    let html = page(&setup).await;
    assert!(html.contains("No rows yet."));
    let (action, pairs) = new_row(&html, 1, &[("row-0", "Rope"), ("row-2", "tool")]);
    assert_eq!(action, "/private-sheets/1/lists/1/rows");
    assert_eq!(
        keys(&pairs),
        [
            "csrf", "base", "row-0", "row-1", "row-2", "row-3", "set-4", "row-4"
        ]
    );
    assert_eq!(name(&html, "add-1"), "Add row Gear");
    // Ready (a required yes/no) unset: refused, the draft kept and focused.
    let (status, refused) = setup.post(&action, pairs.clone()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        refused.contains("The new Gear row was not added:"),
        "{refused}"
    );
    let (_, kept) = new_row(&refused, 1, &[]);
    assert_eq!(kept[2], pair("row-0", "Rope"));
    assert_eq!(kept[4], pair("row-2", "tool"));
    assert!(
        refused.contains(
            "id=\"row-new-1-0\" name=\"row-0\" value=\"Rope\" maxlength=\"8000\" autofocus"
        )
    );
    // A required enum unset is refused too.
    let (_, unset) = new_row(&html, 1, &[("row-0", "Rope"), ("row-3", "true")]);
    assert_eq!(
        setup.post(&action, unset).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(saved(&setup, 1).await.0, 1, "nothing written");
    // A blank Count is unknown; the row gets the host ID r2-1.
    let (_, complete) = new_row(
        &html,
        1,
        &[("row-0", "Rope"), ("row-2", "tool"), ("row-3", "true")],
    );
    let response = super::router_request(&setup.app, &setup.auth[1], &action, Some(complete)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()["location"], "/private-sheets/1#row-1-0");
    let data = saved(&setup, 1).await.1;
    assert_eq!(
        data["gear"],
        json!([{"id": "r2-1", "name": "Rope", "count": null, "kind": "tool", "ready": true}])
    );
    assert_eq!(
        notes_at(&setup, 1, 2).await,
        vec![],
        "a blank number writes no note"
    );
    // "lots" is unknown with its scribble; an optional Worn set.
    let html = page(&setup).await;
    let (_, scribbled) = new_row(
        &html,
        1,
        &[
            ("row-0", "Torch"),
            ("row-1", "lots"),
            ("row-2", "arm"),
            ("row-3", "false"),
            ("set-4", "present"),
            ("row-4", "true"),
        ],
    );
    assert_eq!(
        setup.post(&action, scribbled).await.0,
        StatusCode::SEE_OTHER
    );
    let data = saved(&setup, 1).await.1;
    assert_eq!(
        data["gear"][1],
        json!({"id": "r3-1", "name": "Torch", "count": null, "kind": "arm", "ready": false, "worn": true})
    );
    assert_eq!(
        notes_at(&setup, 1, 3).await,
        vec![note(
            3,
            "cell:gear/r3-1/count",
            "unresolved",
            "lots",
            None,
            r#"{"unknown":true}"#
        )]
    );
    let html = page(&setup).await;
    assert_eq!(
        text(&html, "#note-1-1-1").as_deref(),
        Some("? (unknown) · ‘lots’ not understood")
    );
}

/// The new-row form writes at most one note per number, choice or yes/no property: 32 for a
/// row of 32 integers, each read as a sum.
#[tokio::test]
async fn the_new_row_form_writes_at_most_32_notes() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let mut definition = gear();
    definition["fields"][1]["items"]["properties"] = Value::Array(
        (0..32)
            .map(|p| json!({"id": format!("n{p:02}"), "label": format!("N{p}"), "control": "integer", "optional": false}))
            .collect(),
    );
    let setup = setup(&definition).await;
    gear_sheet(&setup).await;
    let typed: Vec<(String, &str)> = (0..32).map(|p| (format!("row-{p}"), "1+1")).collect();
    let typed: Vec<(&str, &str)> = typed
        .iter()
        .map(|(key, value)| (key.as_str(), *value))
        .collect();
    let (action, pairs) = new_row(&page(&setup).await, 1, &typed);
    assert_eq!(setup.post(&action, pairs).await.0, StatusCode::SEE_OTHER);
    let written = notes_at(&setup, 1, 2).await;
    assert_eq!(written.len(), 32);
    assert!(
        written
            .iter()
            .all(|note| note.2 == "sum" && note.5 == r#"{"int":"2"}"#)
    );
    assert_eq!(saved(&setup, 1).await.1["gear"][0]["n31"], json!(2));
}

/// Every button is named through `aria-labelledby`: "Save Quantity Equipment Row 1",
/// "Remove Row 2 Equipment", "Move up Row 2 Equipment"; each cell form's default submitter
/// is its Save, and the row actions belong to the page's shared form.
#[tokio::test]
async fn list_buttons_are_named_by_their_cell_and_row() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    three_rows(&setup).await;
    let html = page(&setup).await;
    for (button, expected) in [
        ("save-4-0-1", "Save Quantity Equipment Row 1"),
        ("save-4-2-3", "Save Broken Equipment Row 3"),
        ("remove-4-1", "Remove Row 2 Equipment"),
        ("up-4-1", "Move up Row 2 Equipment"),
        ("down-4-1", "Move down Row 2 Equipment"),
        ("add-4", "Add row Equipment"),
    ] {
        assert_eq!(name(&html, button), expected, "{button}");
    }
    let page = scraper::Html::parse_document(&html);
    let cell = page
        .select(&scraper::Selector::parse("form[action$=\"/cells/4/1/1\"]").unwrap())
        .next()
        .unwrap();
    let default = cell
        .select(&scraper::Selector::parse("button:not([form])").unwrap())
        .next()
        .unwrap();
    assert_eq!(default.value().attr("id"), Some("save-4-1-1"));
    let shared = page
        .select(&scraper::Selector::parse("button[form=\"row-actions\"]").unwrap())
        .count();
    assert_eq!(shared, 9, "Move up, Move down and Remove of each row");
}

/// A POST as a browser navigation sends it: `Accept: text/html` and document fetch metadata.
pub(super) async fn document_post(
    setup: &Setup,
    path: &str,
    pairs: Pairs,
) -> axum::response::Response {
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
pub(super) fn hold(seconds: u64) -> tokio::task::JoinHandle<()> {
    let permit = crate::definitions::admit().unwrap();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
        drop(permit);
    })
}

/// D-X3 in a real browser: the cell, new-row and row-action saves wait up to 2 s for the
/// permit, then answer the styled busy page (kept by the auth layer under document
/// headers) that keeps the entry, the new row's pairs or the action; posting it again
/// succeeds once the permit is free.
#[tokio::test]
async fn list_busy_pages_survive_a_browser_navigation() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    three_rows(&setup).await;
    let html = page(&setup).await;
    // A cell: the entry, `base` and `set` are kept.
    let (action, mut pairs) = unit_form(&html, "row-4-1-3", None);
    pairs[2].1 = "present".into();
    pairs[3].1 = "true".into();
    let holder = hold(3);
    let started = std::time::Instant::now();
    let response = document_post(&setup, &action, pairs).await;
    let elapsed = started.elapsed();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["retry-after"], "1");
    assert!(
        elapsed >= std::time::Duration::from_millis(1900),
        "{elapsed:?}"
    );
    let busy = body(response).await;
    assert!(busy.contains("Server busy: nothing was saved."), "{busy}");
    assert!(
        busy.contains("Your entry for column 4 of row 2 of list field 4"),
        "{busy}"
    );
    let (again, kept) = unit_form(&busy, "busy-value", None);
    assert_eq!(again, "/private-sheets/1/cells/4/1/3");
    assert_eq!(
        kept,
        vec![
            pair("csrf", &"a".repeat(64)),
            pair("base", "1"),
            pair("set", "present"),
            pair("value", "true"),
        ]
    );
    holder.await.unwrap();
    assert_eq!(
        document_post(&setup, &again, kept).await.status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        saved(&setup, 1).await.1["equipment"][1]["broken"],
        json!(true)
    );
    // A new row: every pair kept, escaped.
    let html = page(&setup).await;
    let (action, pairs) = new_row(&html, 4, &[("row-0", "Rope \"<b>\""), ("row-1", "2")]);
    let holder = hold(3);
    let response = document_post(&setup, &action, pairs.clone()).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let busy = body(response).await;
    assert!(busy.contains("Your changes are kept"), "{busy}");
    assert!(
        busy.contains("value=\"Rope &#34;&#60;b&#62;&#34;\""),
        "{busy}"
    );
    let (again, kept) = super::formula_tests::form_submission(&busy);
    assert_eq!(again, "/private-sheets/1/lists/4/rows");
    let (mut sent, mut kept_sorted) = (pairs.clone(), kept.clone());
    sent.sort();
    kept_sorted.sort();
    assert_eq!(kept_sorted, sent, "every pair kept");
    holder.await.unwrap();
    assert_eq!(
        document_post(&setup, &again, kept).await.status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(rows(&setup).await[3].1, "Rope \"<b>\"");
    // A row action: its `base`, and a retry button.
    let html = page(&setup).await;
    let (action, pairs) = row_action(&html, "remove-4-0");
    let holder = hold(3);
    let response = document_post(&setup, &action, pairs).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let busy = body(response).await;
    assert!(busy.contains("Server busy: nothing was changed."), "{busy}");
    let (again, kept) = super::formula_tests::form_submission(&busy);
    assert_eq!(again, "/private-sheets/1/lists/4/rows/0/remove");
    assert_eq!(keys(&kept), ["csrf", "base"]);
    assert!(
        busy.contains("<button autofocus>Remove row 1 again</button>"),
        "{busy}"
    );
    holder.await.unwrap();
    assert_eq!(
        document_post(&setup, &again, kept).await.status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(rows(&setup).await.len(), 3, "Axe removed, Rope added");
}

/// The other answers that keep the draft reach a browser too: a stale cell (409), a refused
/// cell (422) and a refused new row (422) re-render the trusted editor page.
#[tokio::test]
async fn refused_list_saves_keep_their_draft_in_a_browser() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&gear()).await;
    gear_sheet(&setup).await;
    let html = page(&setup).await;
    let (action, pairs) = new_row(
        &html,
        1,
        &[("row-0", "Rope"), ("row-1", "5"), ("row-2", "tool")],
    );
    let response = document_post(&setup, &action, pairs).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let refused = body(response).await;
    assert!(
        refused.contains("The new Gear row was not added"),
        "{refused}"
    );
    assert_eq!(new_row(&refused, 1, &[]).1[3], pair("row-1", "5"));
    let (_, complete) = new_row(
        &html,
        1,
        &[
            ("row-0", "Rope"),
            ("row-1", "5"),
            ("row-2", "tool"),
            ("row-3", "true"),
        ],
    );
    assert_eq!(setup.post(&action, complete).await.0, StatusCode::SEE_OTHER);
    let old = page(&setup).await;
    assert_eq!(
        enter(&setup, &page(&setup).await, "row-1-0-1", "6").await.0,
        StatusCode::SEE_OTHER
    );
    let (action, pairs) = unit_form(&old, "row-1-0-1", Some("7"));
    let response = document_post(&setup, &action, pairs).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let html = body(response).await;
    assert!(html.contains("Count, row 1 of Gear not saved"), "{html}");
    assert_eq!(unit_form(&html, "row-1-0-1", None).1[2], pair("value", "7"));
    let (action, pairs) = unit_form(&html, "row-1-0-1", Some("7\u{7}"));
    let response = document_post(&setup, &action, pairs).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = body(response).await;
    assert!(html.contains("Count, row 1 of Gear not saved"), "{html}");
    assert_eq!(saved(&setup, 1).await.1["gear"][0]["count"], json!(6));
}

/// Rights before keys: an outsider's list saves are 403 whatever their keys; a frozen
/// sheet's are 403 and its page has no cell, row-action or new-row forms.
#[tokio::test]
async fn list_routes_recheck_rights_before_keys() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    let player = super::notes_tests::player_sheet(&setup).await;
    let outsider = &setup.auth[0];
    let cell = vec![
        pair("csrf", &"a".repeat(64)),
        pair("base", "1"),
        pair("value", "5"),
    ];
    for path in [
        "/private-sheets/1/cells/4/0/1",
        "/private-sheets/1/cells/9/9/9",
        "/private-sheets/1/lists/4/rows",
        "/private-sheets/1/lists/4/rows/0/remove",
    ] {
        let response = super::router_request(&setup.app, outsider, path, Some(cell.clone())).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
    }
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
        "/private-sheets/1/cells/4/0/1",
        Some(cell),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let frozen =
        body(super::router_request(&setup.app, &player, "/private-sheets/1", None).await).await;
    assert!(!frozen.contains("/cells/"), "{frozen}");
    assert!(!frozen.contains("/lists/4/rows"));
    assert!(!frozen.contains("row-actions"));
}

/// Migration 021 (slice 1.5b): the note table admits row addresses (`row:<list>/<host>`)
/// and the 6-byte `meta:x` (slice-1.2 review row 3); other addresses stay refused.
#[tokio::test]
async fn row_notes_and_six_byte_meta_addresses_are_admitted() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    three_rows(&setup).await;
    let insert = |address: &'static str| {
        // Inside a request's auth, as the note trigger requires.
        setup.store.run_campaign(setup.auth[1].clone(), move |db| {
            db.execute(
                "INSERT INTO private_sheet_notes(sheet_id,revision,address,op,raw,after) VALUES(1,1,?1,'literal','','{\"row\":\"absent\"}')",
                [address],
            )?;
            Ok(())
        })
    };
    assert!(insert("row:equipment/r1-1").await.is_ok());
    assert!(insert("meta:x").await.is_ok());
    for refused in [
        "row:",
        "row:equipment",
        "meta:",
        "list:equipment/r1-1",
        "cell:a/b",
    ] {
        assert!(insert(refused).await.is_err(), "{refused}");
    }
}

/// Migration 021 upgrades a schema-20 database: every note is kept byte for byte, with the
/// three triggers, the current-note index and the primary key, and a row removal then
/// writes its note.
#[tokio::test]
async fn schema_20_upgrades_to_21_keeping_every_note() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    three_rows(&setup).await;
    assert_eq!(
        enter(&setup, &page(&setup).await, "row-4-0-1", "1+1")
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    let all = |db: &rusqlite::Connection| -> Result<Vec<Vec<String>>, crate::AppError> {
        let mut query = db.prepare("SELECT sheet_id,revision,address,op,raw,coalesce(before,'-'),after,coalesce(bundle,0),coalesce(target,0) FROM private_sheet_notes ORDER BY sheet_id,revision,address")?;
        let rows = query
            .query_map([], |r| {
                (0..9)
                    .map(|i| r.get_ref(i).map(|value| format!("{value:?}")))
                    .collect()
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    };
    let before = setup
        .store
        .run(move |db| {
            let before = all(db)?;
            // Rewind to a genuine schema-20 shape: migration 020's table, triggers and index,
            // holding the same notes (the insert trigger is set aside while they are copied).
            db.execute_batch("CREATE TEMP TABLE kept AS SELECT * FROM private_sheet_notes; DROP TABLE private_sheet_notes;")?;
            db.execute_batch(include_str!("../../migrations/020_sheet_notes.sql"))?;
            let trigger: String = db.query_row(
                "SELECT sql FROM sqlite_master WHERE name='private_sheet_notes_insert'",
                [],
                |r| r.get(0),
            )?;
            db.execute_batch("DROP TRIGGER private_sheet_notes_insert; INSERT INTO private_sheet_notes SELECT * FROM kept; DROP TABLE kept;")?;
            db.execute_batch(crate::assistant::records::DROP_SCHEMA)?;
            db.execute_batch(&format!("{trigger};"))?;
            assert_eq!(
                db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
                20
            );
            Ok(before)
        })
        .await
        .unwrap();
    assert_eq!(before.len(), 1, "the sum note");
    let reopened = crate::Store::open(&setup.dir.path().join("private.sqlite3")).unwrap();
    reopened
        .run(|db| {
            crate::store::register_auth_clock(db, std::sync::Arc::new(|| Ok(100)))?;
            Ok(())
        })
        .await
        .unwrap();
    let (version, objects, after) = reopened
        .run(move |db| {
            Ok((
                db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
                db.query_row("SELECT count(*) FROM sqlite_master WHERE tbl_name='private_sheet_notes' AND type IN ('trigger','index')", [], |r| r.get::<_, i64>(0))?,
                all(db)?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(version, crate::SCHEMA_VERSION);
    assert_eq!(version, 22);
    assert_eq!(after, before, "notes kept byte for byte");
    assert_eq!(objects, 5, "three triggers, the index and the primary key");
    let app = crate::auth::app(reopened.clone()).await.unwrap();
    let html =
        body(super::router_request(&app, &setup.auth[1], "/private-sheets/1", None).await).await;
    let (action, pairs) = row_action(&html, "remove-4-1");
    let response = super::router_request(&app, &setup.auth[1], &action, Some(pairs)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let written = reopened
        .run(|db| super::super::notes::of_revision(db, 1, 3))
        .await
        .unwrap();
    assert_eq!(written.len(), 1);
    assert_eq!(written[0].note.address, "row:equipment/r1-2");
    // Notes stay insert-only after the rebuild.
    let update = reopened
        .run(|db| Ok(db.execute("UPDATE private_sheet_notes SET raw='x'", [])?))
        .await;
    assert!(update.is_err());
}

// ---- Review round 1 ----

/// The kept-draft block of list `f`: the text of its message, labels, states and values.
pub(super) fn kept(html: &str, f: usize) -> Option<String> {
    text(html, &format!("#kept-{f}"))
}

/// Sol Major 1: a new row refused because the list filled up meanwhile (255 rows at load,
/// another tab adds the 256th) keeps every submitted value and presence state on the 422
/// page, read-only and copyable, with no form that could add it; nothing is written.
#[tokio::test]
async fn a_new_row_refused_by_a_full_list_keeps_every_value() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    let (status, html) = setup
        .post(&create_path(&setup), form(0, &[], &many(255)))
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
    let old = page(&setup).await;
    let (action, other) = new_row(&page(&setup).await, 4, &[("row-0", "Other")]);
    assert_eq!(setup.post(&action, other).await.0, StatusCode::SEE_OTHER);
    let before = saved(&setup, 1).await;
    let (_, mine) = new_row(
        &old,
        4,
        &[
            ("row-0", "Late \"lamp\" <b>"),
            ("row-1", "7"),
            ("row-2", "2+1"),
            ("set-3", "present"),
            ("row-3", "true"),
        ],
    );
    let response = document_post(&setup, &action, mine).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = body(response).await;
    assert_eq!(saved(&setup, 1).await, before, "nothing written");
    assert!(!html.contains("/lists/4/rows\""), "no form adds it");
    let kept = kept(&html, 4).expect("the kept row");
    for shown in [
        "Late \"lamp\" <b>",
        "7",
        "2+1",
        "Set",
        "true",
        "Item name",
        "Broken",
    ] {
        assert!(kept.contains(shown), "{shown:?} in {kept:?}");
    }
    assert!(kept.contains("full"), "{kept}");
    assert!(!html.contains("<b>"), "escaped");
}

/// Sonnet Minor (round 1): a cell save whose row was removed since its page was loaded is a
/// 409 that shows the draft, read-only, under "its row was removed"; it is never put into
/// the row now at the same index, and nothing is written.
#[tokio::test]
async fn a_cell_whose_row_was_removed_keeps_its_draft() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    three_rows(&setup).await;
    let old = page(&setup).await;
    assert_eq!(
        press(&setup, &page(&setup).await, "remove-4-1").await.0,
        StatusCode::SEE_OTHER
    );
    let before = saved(&setup, 1).await;
    let (action, pairs) = unit_form(&old, "row-4-1-1", Some("9 \"x\""));
    let response = document_post(&setup, &action, pairs).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let html = body(response).await;
    assert_eq!(saved(&setup, 1).await, before, "nothing written");
    assert!(html.contains("its row was removed"), "{html}");
    let kept = kept(&html, 4).expect("the kept entry");
    assert!(
        kept.contains("9 \"x\"") && kept.contains("Quantity"),
        "{kept}"
    );
    // Cup now sits at index 1: its Quantity is unchanged, not the draft.
    assert_eq!(unit_form(&html, "row-4-1-1", None).1[2], pair("value", "3"));
    assert!(
        !html.contains("autofocus"),
        "no row control takes the draft"
    );
}

/// Sol Major 2: a cell whose row is removed between planning and the writer's IMMEDIATE
/// transaction gets the writer's 409 with the same orphaned-draft recovery.
#[tokio::test]
async fn a_cell_whose_row_goes_before_the_write_keeps_its_draft() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    three_rows(&setup).await;
    let html = page(&setup).await;
    let (action, pairs) = unit_form(&html, "row-4-1-1", Some("9"));
    let (planned, planned_rx) = tokio::sync::oneshot::channel();
    let (resume, resume_rx) = tokio::sync::oneshot::channel();
    *super::super::web::AFTER_UNIT_PLAN.lock().unwrap() = Some((planned, resume_rx));
    let app = setup.app.clone();
    let cookie = setup.auth[1].fixture_cookie();
    let post = tokio::spawn(async move {
        use tower::ServiceExt;
        let request = axum::http::Request::builder()
            .uri(&action)
            .method("POST")
            .header("host", "127.0.0.1:3000")
            .header("origin", "http://127.0.0.1:3000")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("accept", "text/html")
            .header("sec-fetch-mode", "navigate")
            .header("sec-fetch-dest", "document")
            .header("cookie", cookie)
            .body(axum::body::Body::from(
                serde_urlencoded::to_string(pairs).unwrap(),
            ))
            .unwrap();
        app.oneshot(request).await.unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(20), planned_rx)
        .await
        .expect("planned within 20 s")
        .unwrap();
    // Another writer removes Bow while the save is planned (outside the runtime permit).
    let dummy = || async {
        std::sync::Arc::new(tokio::sync::Semaphore::new(1))
            .acquire_owned()
            .await
            .unwrap()
    };
    let (mut view, permit) = setup
        .store
        .private_read(setup.auth[1].clone(), 1, None, dummy().await)
        .await
        .unwrap();
    view.snapshot.data["equipment"]
        .as_array_mut()
        .unwrap()
        .remove(1);
    setup
        .store
        .private_save(setup.auth[1].clone(), view, permit)
        .await
        .unwrap();
    resume.send(()).unwrap();
    let response = tokio::time::timeout(std::time::Duration::from_secs(20), post)
        .await
        .expect("answered within 20 s")
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let html = body(response).await;
    assert!(html.contains("its row was removed"), "{html}");
    assert!(kept(&html, 4).expect("the kept entry").contains('9'));
    assert_eq!(unit_form(&html, "row-4-1-1", None).1[2], pair("value", "3"));
    let ids: Vec<String> = rows(&setup).await.into_iter().map(|row| row.0).collect();
    assert_eq!(ids, ["r1-1", "r1-3"], "only the other writer's removal");
}

/// Both reviewers (round 1): after a Move the page comes back with the moved row's Move
/// button in the same direction focused, or the other direction's when it reached an edge
/// (the full form's focus continuation): `?focus=` and one `autofocus`.
#[tokio::test]
async fn a_move_returns_to_the_moved_rows_move_button() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    three_rows(&setup).await;
    for (button, focus) in [
        ("up-4-2", "up-4-1"),
        ("up-4-1", "down-4-0"),
        ("down-4-0", "down-4-1"),
        ("down-4-1", "up-4-2"),
    ] {
        let (action, pairs) = row_action(&page(&setup).await, button);
        let response =
            super::router_request(&setup.app, &setup.auth[1], &action, Some(pairs)).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{button}");
        let location = response.headers()["location"].to_str().unwrap().to_owned();
        assert_eq!(
            location,
            format!("/private-sheets/1?focus={focus}"),
            "{button}"
        );
        let (status, html) = setup.get(&location).await;
        assert_eq!(status, StatusCode::OK);
        let page = scraper::Html::parse_document(&html);
        let focused: Vec<_> = page
            .select(&scraper::Selector::parse("[autofocus]").unwrap())
            .filter_map(|element| element.value().attr("id"))
            .collect();
        assert_eq!(focused, [focus], "{button}");
    }
    // A malformed or unknown focus is ignored.
    for query in ["?focus=x", "?focus=up-4-9", "?focus=up-04-1"] {
        let (status, html) = setup.get(&format!("/private-sheets/1{query}")).await;
        assert_eq!(status, StatusCode::OK, "{query}");
        assert!(!html.contains("autofocus"), "{query}");
    }
}

/// R1 with a dirty row cell (plan §3a): tab A has an unsaved Quantity of row 2; tab B saves
/// that Quantity; tab A saves row 1's Weight, which succeeds; tab A's row-2 form still has
/// its old `base`, so its save is 409 with the stored value and the draft both kept.
#[tokio::test]
async fn two_tabs_without_script_keep_each_cells_base() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    three_rows(&setup).await;
    let tab_a = page(&setup).await;
    let tab_b = page(&setup).await;
    assert_eq!(
        enter(&setup, &tab_b, "row-4-1-1", "5").await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        enter(&setup, &tab_a, "row-4-0-2", "4").await.0,
        StatusCode::SEE_OTHER
    );
    let (status, html) = enter(&setup, &tab_a, "row-4-1-1", "8").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(unit_form(&html, "row-4-1-1", None).1[2], pair("value", "8"));
    assert_eq!(unit_form(&html, "row-4-1-1", None).1[1], pair("base", "3"));
    let data = saved(&setup, 1).await.1;
    assert_eq!(
        (
            &data["equipment"][1]["quantity"],
            &data["equipment"][0]["weight"]
        ),
        (&json!(5), &json!(4)),
        "stored kept"
    );
}
