//! Assisted sheet step 4, items 3 to 5: the key written with a name (a pick among duplicates)
//! and cleared when the name's norm changes, on the cell and new-row routes and the full form.
use super::{
    catalogue_member_tests::{
        definition, full_form, gear, href, links, new_row_form, page, published, stored, world,
    },
    list_tests::{legacy_create, pair},
};
use axum::http::StatusCode;
use serde_json::{Value, json};

type Form = Vec<(String, String)>;

fn cell(base: i64, value: &str, extra: &[(&str, &str)]) -> Form {
    let mut form = vec![
        pair("csrf", "a".repeat(64)),
        pair("base", base.to_string()),
        pair("value", value),
    ];
    form.extend(extra.iter().map(|(key, value)| pair(*key, *value)));
    form
}

async fn revision(setup: &super::formula_tests::Setup) -> i64 {
    stored(setup).await["revision"].as_i64().unwrap()
}

async fn notes(setup: &super::formula_tests::Setup) -> i64 {
    setup
        .store
        .run(|db| Ok(db.query_row("SELECT count(*) FROM private_sheet_notes", [], |r| r.get(0))?))
        .await
        .unwrap()
}

#[tokio::test]
async fn picking_one_of_several_duplicates_stores_the_key_in_one_revision_without_a_note() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Sword", None), ("Rope", None)]).await;
    assert_eq!(
        links(&page(&setup).await),
        [("entry-1-1-0".to_owned(), href("rope"))]
    );
    let notes_before = notes(&setup).await;
    let (status, _) = setup
        .post(
            "/private-sheets/1/cells/1/0/0",
            cell(1, "Sword", &[("entry", "sword-b")]),
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let saved = stored(&setup).await;
    assert_eq!(saved["revision"], 2, "one revision");
    assert_eq!(saved["data"]["equipment"][0]["gear_key"], "sword-b");
    assert_eq!(notes(&setup).await, notes_before, "no note");
    assert_eq!(
        links(&page(&setup).await),
        [
            ("entry-1-0-0".to_owned(), href("sword-b")),
            ("entry-1-1-0".to_owned(), href("rope")),
        ]
    );
    // The same name and key again is "No change" (D-U): no revision.
    let (status, body) = setup
        .post(
            "/private-sheets/1/cells/1/0/0",
            cell(2, "Sword", &[("entry", "sword-b")]),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("No change"));
    assert_eq!(revision(&setup).await, 2);
}

#[tokio::test]
async fn a_pick_that_no_longer_matches_is_422_and_writes_nothing() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Sword", None), ("Old Rope", None)]).await;
    for (value, entry) in [
        ("Sword", "gone"),        // no such entry
        ("Sword", "shield"),      // another name
        ("Old Rope", "old-rope"), // retired
    ] {
        let (status, body) = setup
            .post(
                "/private-sheets/1/cells/1/0/0",
                cell(1, value, &[("entry", entry)]),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{entry}");
        assert!(
            body.contains("That entry no longer has this name in the catalogue."),
            "{entry}: {body}"
        );
    }
    assert_eq!(revision(&setup).await, 1);
    // The entry type of the member filters: a tool is no pick for a weapon list.
    let setup = world(Some("weapon"), &[("Sword", None)]).await;
    let (status, _) = setup
        .post(
            "/private-sheets/1/cells/1/0/0",
            cell(1, "Sword", &[("entry", "sword-b")]),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = setup
        .post(
            "/private-sheets/1/cells/1/0/0",
            cell(1, "Sword", &[("entry", "sword-a")]),
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn entry_is_admitted_only_on_a_catalogue_backed_name_and_only_as_a_slug() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Sword", None)]).await;
    // Another property (quantity), the key property and a malformed key: 400.
    for (path, form) in [
        (
            "/private-sheets/1/cells/1/0/1",
            cell(1, "3", &[("entry", "sword-a")]),
        ),
        (
            "/private-sheets/1/cells/1/0/2",
            cell(1, "x", &[("entry", "sword-a"), ("set", "present")]),
        ),
        (
            "/private-sheets/1/cells/1/0/0",
            cell(1, "Sword", &[("entry", "SWORD A")]),
        ),
        (
            "/private-sheets/1/cells/1/0/0",
            cell(1, "Sword", &[("entry", "")]),
        ),
        (
            "/private-sheets/1/meta/name",
            cell(1, "Sheet", &[("entry", "sword-a")]),
        ),
        (
            "/private-sheets/1/fields/label",
            cell(1, "Sheet", &[("entry", "sword-a")]),
        ),
    ] {
        let (status, _) = setup.post(path, form).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}");
    }
    assert_eq!(revision(&setup).await, 1);
}

#[tokio::test]
async fn an_optional_name_posts_set_present_and_entry_with_set_absent_is_400() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let mut optional = definition(None);
    optional["fields"][1]["items"]["properties"][0]["optional"] = true.into();
    optional["fields"][1]["items"]["quick_add"] = Value::Null;
    optional["fields"][1]["items"]
        .as_object_mut()
        .unwrap()
        .remove("quick_add");
    let setup = published(&optional, &gear()).await;
    legacy_create(
        &setup,
        1,
        json!({"label": "Sheet", "equipment": [{"id": "r1-1", "name": "Sword", "quantity": 1}]}),
    )
    .await;
    let (status, _) = setup
        .post(
            "/private-sheets/1/cells/1/0/0",
            cell(1, "Sword", &[("entry", "sword-a"), ("set", "absent")]),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = setup
        .post(
            "/private-sheets/1/cells/1/0/0",
            cell(1, "Sword", &[("entry", "sword-a")]),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an optional name needs its set pair"
    );
    let (status, body) = setup
        .post(
            "/private-sheets/1/cells/1/0/0",
            cell(1, "Sword", &[("entry", "sword-a"), ("set", "present")]),
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert_eq!(
        stored(&setup).await["data"]["equipment"][0]["gear_key"],
        "sword-a"
    );
}

#[tokio::test]
async fn a_rename_clears_the_key_in_the_same_revision_and_a_recase_keeps_it() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Sword", Some("sword-b")), ("Rope", Some("rope"))]).await;
    let (status, _) = setup
        .post("/private-sheets/1/cells/1/0/0", cell(1, "SWORD", &[]))
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let saved = stored(&setup).await;
    assert_eq!(saved["data"]["equipment"][0]["name"], "SWORD");
    assert_eq!(
        saved["data"]["equipment"][0]["gear_key"], "sword-b",
        "recasing keeps"
    );
    let (status, _) = setup
        .post("/private-sheets/1/cells/1/0/0", cell(2, "Swordfish", &[]))
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let saved = stored(&setup).await;
    assert_eq!(saved["revision"], 3, "name and key in one revision");
    assert!(saved["data"]["equipment"][0].get("gear_key").is_none());
}

#[tokio::test]
async fn the_full_form_clears_the_key_by_the_same_rule() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Sword", Some("sword-b")), ("Rope", Some("rope"))]).await;
    // `full_form` renames row 1 to "Shield" and keeps row 2's "Rope".
    let (status, body) = setup.post("/private-sheets/1", full_form(vec![])).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let rows = stored(&setup).await["data"]["equipment"].clone();
    assert_eq!(rows[0]["name"], "Shield");
    assert!(rows[0].get("gear_key").is_none());
    assert_eq!(rows[1]["gear_key"], "rope");
}

#[tokio::test]
async fn a_new_row_takes_a_pick_among_duplicates_with_its_key_in_one_revision() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Shield", None)]).await;
    let mut form = new_row_form(vec![pair("entry", "sword-b")]);
    for (key, value) in &mut form {
        if key == "row-0" {
            *value = "Sword".into();
        }
    }
    let (status, body) = setup.post("/private-sheets/1/lists/1/rows", form).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let saved = stored(&setup).await;
    assert_eq!(saved["revision"], 2);
    assert_eq!(saved["data"]["equipment"][1]["name"], "Sword");
    assert_eq!(saved["data"]["equipment"][1]["gear_key"], "sword-b");
    // A stale pick is refused with the row kept below; a name that differs is stale too.
    for (name, entry) in [("Sword", "gone"), ("Lamp", "sword-b")] {
        let mut form = new_row_form(vec![pair("entry", entry)]);
        for (key, value) in &mut form {
            if key == "row-0" {
                *value = name.into();
            }
        }
        let (status, body) = setup
            .post("/private-sheets/1/lists/1/rows", form.clone())
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{name}/{entry}");
        assert!(body.contains("That entry no longer has this name in the catalogue."));
    }
    assert_eq!(revision(&setup).await, 2);
    // `entry` only as the slug the key is.
    let form = new_row_form(vec![pair("entry", "NOT A SLUG")]);
    let (status, _) = setup.post("/private-sheets/1/lists/1/rows", form).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_busy_answer_keeps_the_pick_among_duplicates() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Sword", None)]).await;
    let jobs = crate::definitions::admit().unwrap();
    // The cell route's busy page holds `entry` beside `base`, so a retry posts the pick.
    let (status, body) = setup
        .post(
            "/private-sheets/1/cells/1/0/0",
            cell(1, "Sword", &[("entry", "sword-b")]),
        )
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body.contains("name=\"entry\" value=\"sword-b\""), "{body}");
    // The new-row route keeps every pair, `entry` included.
    let mut form = new_row_form(vec![pair("entry", "sword-a")]);
    for (key, value) in &mut form {
        if key == "row-0" {
            *value = "Sword".into();
        }
    }
    let (status, body) = setup.post("/private-sheets/1/lists/1/rows", form).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body.contains("name=\"entry\" value=\"sword-a\""), "{body}");
    drop(jobs);
    assert_eq!(revision(&setup).await, 1);
}

#[tokio::test]
async fn a_stored_pick_survives_move_and_the_undo_of_a_removal() {
    use super::assist_tests::click;
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Sword", None), ("Rope", None)]).await;
    let (status, _) = setup
        .post(
            "/private-sheets/1/cells/1/0/0",
            cell(1, "Sword", &[("entry", "sword-b")]),
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let row_action = |action: &'static str, row: usize| {
        let setup = &setup;
        async move {
            let base = revision(setup).await.to_string();
            setup
                .post(
                    &format!("/private-sheets/1/lists/1/rows/{row}/{action}"),
                    vec![pair("csrf", "a".repeat(64)), pair("base", base)],
                )
                .await
                .0
        }
    };
    let picked = || async {
        let rows = stored(&setup).await["data"]["equipment"].clone();
        rows.as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == "Sword")
            .map(|row| row["gear_key"].clone())
    };
    // Move down and up: the pick stays on its row and the page still links it.
    assert_eq!(row_action("down", 0).await, StatusCode::SEE_OTHER);
    assert_eq!(picked().await, Some(json!("sword-b")));
    assert!(links(&page(&setup).await).contains(&("entry-1-1-0".to_owned(), href("sword-b"))));
    assert_eq!(row_action("up", 1).await, StatusCode::SEE_OTHER);
    assert_eq!(picked().await, Some(json!("sword-b")));
    // Remove the Sword row and undo the removal: the key comes back with it.
    assert_eq!(row_action("remove", 0).await, StatusCode::SEE_OTHER);
    assert_eq!(picked().await, None);
    let (_, html) = setup.get("/private-sheets/1").await;
    assert_eq!(
        click(&setup, &html, "undo-row-1").await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(picked().await, Some(json!("sword-b")));
    assert!(links(&page(&setup).await).contains(&("entry-1-0-0".to_owned(), href("sword-b"))));
}
