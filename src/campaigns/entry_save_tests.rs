//! Slice 1.1 connected saves (assisted sheet plan §5): lenient number entry through the real
//! router on the existing full-form save, with native sheets kept strict.
use super::formula_tests::{Setup, setup, submitted};
use axum::http::StatusCode;
use serde_json::{Value, json};

/// The demo definition (`assist-definition.json`), with a mapped source as publication needs.
fn assist() -> Value {
    let mut definition = crate::definitions::source_fixture::assist_definition();
    definition["sources"] =
        json!([{"source_id": 1, "pdf_sha256": "0".repeat(64), "roles": ["sheet"]}]);
    definition["fields"][0]["mapping"] =
        json!({"state": "mapped", "regions": [{"source_id": 1, "page_id": 1, "unit_id": 1}]});
    definition
}

fn pair(key: &str, value: &str) -> (String, String) {
    (key.into(), value.into())
}

/// A full assisted-sheet form. `equipment` rows are (host ID, name, quantity, weight); the
/// list is field 4 and `broken` (property 3) is left Not set.
fn form(
    revision: i64,
    hp: &str,
    armour: &str,
    notes: &str,
    equipment: &[(&str, &str, &str, &str)],
) -> Vec<(String, String)> {
    let mut form = vec![
        pair("csrf", &"a".repeat(64)),
        pair("revision", &revision.to_string()),
        pair("name", "Assisted sheet"),
        pair("notes", notes),
        pair("value-hp", hp),
        pair("value-max_hp", "84"),
        pair("value-armour", armour),
        pair("value-luck", ""),
        pair("null-luck", "unknown"),
    ];
    for (r, (id, name, quantity, weight)) in equipment.iter().enumerate() {
        form.extend([
            pair(&format!("id-4-{r}"), id),
            pair(&format!("row-4-{r}-0"), name),
            pair(&format!("row-4-{r}-1"), quantity),
            pair(&format!("row-4-{r}-2"), weight),
            pair(&format!("row-4-{r}-3"), ""),
            pair(&format!("set-4-{r}-3"), "absent"),
        ]);
    }
    form
}

fn create_path(setup: &Setup) -> String {
    format!(
        "/campaigns/{}/sheets/new?version={}",
        setup.campaign, setup.version
    )
}

/// Sheet 1 at revision 1: HP 84, Armour −1, one Rope row (quantity 1, weight 1).
async fn created(setup: &Setup) {
    let create = form(0, "84", "-1", "", &[("", "Rope", "1", "1")]);
    let (status, _) = setup.post(&create_path(setup), create).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "create");
}

/// The saved revision and data of sheet `id`.
async fn saved(setup: &Setup, id: i64) -> (i64, Value) {
    let (view, permit) = setup
        .store
        .private_read(
            setup.auth[1].clone(),
            id,
            None,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    drop(permit);
    (view.snapshot.revision, view.snapshot.data)
}

#[tokio::test]
async fn scalar_sum_saves_its_result() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup).await;
    let edit = form(1, "84 -15", "-1", "", &[("r1-1", "Rope", "1", "1")]);
    let (status, _) = setup.post("/private-sheets/1", edit).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "sum saved");
    let (revision, data) = saved(&setup, 1).await;
    assert_eq!((revision, &data["hp"]), (2, &json!(69)));
    assert_eq!(data["armour"], json!(-1));
    // A sum also works on the create form.
    let create = form(0, "80+4", "0 - 1", "", &[]);
    assert_eq!(
        setup.post(&create_path(&setup), create).await.0,
        StatusCode::SEE_OTHER
    );
    let (_, data) = saved(&setup, 2).await;
    assert_eq!((&data["hp"], &data["armour"]), (&json!(84), &json!(-1)));
}

#[tokio::test]
async fn list_row_sum_saves_its_result() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup).await;
    let edit = form(
        1,
        "84",
        "-1",
        "",
        &[("r1-1", "Rope", "2+1", "1"), ("", "Torch", "1", "4 - 1")],
    );
    let (status, _) = setup.post("/private-sheets/1", edit).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "row sums saved");
    let (_, data) = saved(&setup, 1).await;
    let rows = data["equipment"].as_array().unwrap();
    assert_eq!(
        (&rows[0]["quantity"], &rows[0]["weight"]),
        (&json!(3), &json!(1))
    );
    assert_eq!(
        (&rows[1]["quantity"], &rows[1]["weight"]),
        (&json!(1), &json!(3))
    );
    let (_, page) = setup.get("/private-sheets/1").await;
    assert!(
        page.contains("<dd class=\"evidence-text\">6</dd>"),
        "load 3*1 + 1*3"
    );
}

/// B1 regression (plan §8): re-posting the rendered forms keeps saved negatives, because a
/// sign-led input is literal (D-Q). Covers a scalar and a list-row integer. Since slice
/// 1.5a each unit and each list posts its own form: Armour's form and the list form,
/// re-posted unchanged, write nothing (D-U); General notes then save on their own.
#[tokio::test]
async fn canonical_negatives_survive_a_re_post() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    let create = form(0, "84", "-1", "", &[("", "Cursed rope", "1", "-3")]);
    assert_eq!(
        setup.post(&create_path(&setup), create).await.0,
        StatusCode::SEE_OTHER
    );
    let (_, before) = saved(&setup, 1).await;
    for round in 1..=2 {
        let (_, page) = setup.get("/private-sheets/1").await;
        let (action, armour) = super::unit_tests::unit_form(&page, "v2", None);
        assert!(armour.contains(&pair("value", "-1")), "{armour:?}");
        let (status, html) = setup.post(&action, armour).await;
        assert_eq!(status, StatusCode::OK);
        assert!(html.contains("No change"), "{html}");
        let rendered = submitted(&page);
        assert!(rendered.contains(&pair("row-4-0-2", "-3")), "{rendered:?}");
        assert_eq!(
            setup.post("/private-sheets/1", rendered).await.0,
            StatusCode::OK,
            "the unchanged list form writes nothing"
        );
        let (action, mut notes) = super::unit_tests::unit_form(&page, "meta-notes", None);
        for (key, value) in &mut notes {
            if key == "value" {
                *value = format!("General notes, round {round}");
            }
        }
        assert_eq!(setup.post(&action, notes).await.0, StatusCode::SEE_OTHER);
        let (revision, data) = saved(&setup, 1).await;
        assert_eq!(revision, round + 1);
        assert_eq!(data, before, "values unchanged after re-post {round}");
    }
}

/// Slice 1.2 narrowed this: an unreadable entry over a known saved integer is kept with a
/// scribble (`notes_tests`). Slice 1.4 narrowed it again: blank input is unknown, and an
/// unreadable entry in a new row or on create is unknown with a scribble (`unknown_tests`).
/// Control-character and over-long input still get 422 with the draft, in a saved cell and
/// in a new row.
#[tokio::test]
async fn unkeepable_entry_is_still_422_with_the_draft() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup).await;
    let (_, before) = saved(&setup, 1).await;
    let long = format!("1{}", " ".repeat(200));
    for (hp, quantity) in [
        ("84\u{7}", "1"),
        (long.as_str(), "1"),
        ("84", "2\u{7}"),
        ("84 -15", long.as_str()),
    ] {
        let edit = form(
            1,
            hp,
            "-1",
            "kept draft",
            &[("r1-1", "Rope", "1", "1"), ("", "Torch", quantity, "1")],
        );
        let (status, html) = setup.post("/private-sheets/1", edit).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{hp:?} {quantity:?}"
        );
        assert!(html.contains("No changes saved:"), "{hp:?} {quantity:?}");
        let page = scraper::Html::parse_document(&html);
        let value = |name: &str| {
            page.select(&scraper::Selector::parse(&format!("[name=\"{name}\"]")).unwrap())
                .next()
                .and_then(|element| element.value().attr("value").map(str::to_owned))
        };
        assert_eq!(value("value-hp").as_deref(), Some(hp), "draft kept");
        assert_eq!(value("row-4-1-1").as_deref(), Some(quantity), "draft kept");
        assert_eq!(saved(&setup, 1).await, (1, before.clone()), "nothing saved");
    }
}

/// Native sheets keep their strict path (`native_draft(...).validate()`), and protocol
/// integers keep the strict `values::integer`.
#[tokio::test]
async fn native_sheets_and_protocol_integers_stay_strict() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    let campaign = setup
        .store
        .create_campaign(
            setup.auth[1].clone(),
            "Native".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    let mut native = super::native(campaign, 2);
    native.sheet_id = 1;
    super::insert_native(&setup.store, setup.auth[1].clone(), native).await;
    let mut native_form = super::native_form();
    let (_, before) = saved(&setup, 1).await;
    for (key, value) in &mut native_form {
        match key.as_str() {
            "revision" => *value = "1".into(),
            "hp" => *value = "84 -15".into(),
            _ => {}
        }
    }
    assert_eq!(
        setup.post("/private-sheets/1", native_form).await.0,
        StatusCode::UNPROCESSABLE_ENTITY,
        "native HP sum refused"
    );
    assert_eq!(saved(&setup, 1).await, (1, before), "nothing saved");
    for raw in ["+1", "84 -15", "84-15", "01", " 1"] {
        assert!(super::values::integer(raw).is_err(), "{raw:?}");
    }
    // The submitted revision is a protocol integer: a sum there is a bad request.
    created(&setup).await;
    let mut edit = form(1, "84", "-1", "", &[("r1-1", "Rope", "1", "1")]);
    edit[1].1 = "0+1".into();
    let (status, _) = setup.post("/private-sheets/2", edit).await;
    assert_ne!(status, StatusCode::SEE_OTHER, "a revision sum is refused");
    assert_eq!(saved(&setup, 2).await.0, 1, "nothing saved");
}

/// Runtime integer inputs have no `inputmode` (the iOS numeric keypad hides minus and
/// space); native inputs keep `inputmode="numeric"`.
#[tokio::test]
async fn runtime_integer_inputs_have_no_numeric_inputmode() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup).await;
    // By name on the full forms (create, native); by DOM id in the unit forms (slice 1.5a).
    let input = |html: &str, name: &str| {
        let page = scraper::Html::parse_document(html);
        let selector = if name.starts_with('#') {
            scraper::Selector::parse(&format!("input{name}")).unwrap()
        } else {
            scraper::Selector::parse(&format!("input[name=\"{name}\"]")).unwrap()
        };
        let element = page
            .select(&selector)
            .next()
            .unwrap_or_else(|| panic!("{name}"));
        element.value().attr("inputmode").map(str::to_owned)
    };
    for path in ["/private-sheets/1".to_owned(), create_path(&setup)] {
        let (status, html) = setup.get(&path).await;
        assert_eq!(status, StatusCode::OK);
        let names = if path == "/private-sheets/1" {
            ["#v0", "#v1", "#v2", "#v3"]
        } else {
            ["value-hp", "value-max_hp", "value-armour", "value-luck"]
        };
        for name in names {
            assert_eq!(input(&html, name), None, "{path} {name}");
        }
        if path == "/private-sheets/1" {
            for name in ["row-4-0-1", "row-4-0-2"] {
                assert_eq!(input(&html, name), None, "{path} {name}");
            }
        }
    }
    let campaign = setup
        .store
        .create_campaign(
            setup.auth[1].clone(),
            "Native".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    let mut native = super::native(campaign, 2);
    native.sheet_id = 2;
    super::insert_native(&setup.store, setup.auth[1].clone(), native).await;
    let (status, html) = setup.get("/private-sheets/2").await;
    assert_eq!(status, StatusCode::OK);
    for name in ["hp", "max_hp", "strength", "omens", "silver"] {
        assert_eq!(
            input(&html, name).as_deref(),
            Some("numeric"),
            "native {name}"
        );
    }
}
