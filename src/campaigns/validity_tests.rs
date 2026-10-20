//! Slice 1.7 validity flag (RR-3, ASSIST-4): a saved revision shows "Not validated" in the
//! sheet header when a check is violated, not evaluated or in error, or when no checks cover
//! its pin. Computed on view from the saved revision, never stored; it never blocks a save
//! and never hides or changes a value.
use super::formula_tests::{Setup, form, formula_definition, setup};
use axum::http::StatusCode;
use serde_json::{Value, json};

const FLAG: &str = "<strong>Not validated</strong>";

/// The flag paragraph, or `None` when the page shows no flag.
fn flag(html: &str) -> Option<&str> {
    let start = html.find("<p id=\"validity\"")?;
    let end = start + html[start..].find("</p>").unwrap();
    Some(&html[start..end])
}

async fn saved(setup: &Setup) -> (i64, Value) {
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
    (view.snapshot.revision, view.snapshot.data)
}

#[tokio::test]
async fn all_checks_passed_with_coverage_shows_no_flag() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&formula_definition()).await;
    setup.create().await;
    for path in ["/private-sheets/1", "/private-sheets/1/history/1"] {
        let (status, html) = setup.get(path).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            html.contains("0 violated, 0 not evaluated, 2 passed."),
            "{path}"
        );
        assert_eq!(flag(&html), None, "{path}");
        assert!(!html.contains("Not validated"), "{path}");
    }
}

#[tokio::test]
async fn violated_check_flags_and_a_flagged_sheet_still_saves() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&formula_definition()).await;
    setup.create().await;
    let (status, _) = setup
        .post("/private-sheets/1", form(1, "11", Some("2")))
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (_, html) = setup.get("/private-sheets/1").await;
    let shown = flag(&html).expect("a violated check flags the sheet");
    assert!(
        shown.contains(&format!("{FLAG} (saved revision 2): 1 check violated.")),
        "{shown}"
    );
    assert!(shown.contains("<a href=\"#advisory-checks\">Why it is not validated</a>"));
    assert!(shown.contains("This flag never blocks saving."));
    // In the header: before any form, so it reads first without script or styles.
    assert!(html.find("id=\"validity\"").unwrap() < html.find("<form").unwrap());
    assert!(
        !html.contains("<script"),
        "the sheet page works without script"
    );
    // The flag hides and changes nothing: the violating value is shown and kept.
    assert!(html.contains("value=\"11\""));
    assert_eq!(saved(&setup).await.1["vitality"], 11);
    // A save of a flagged sheet succeeds.
    let (status, _) = setup
        .post("/private-sheets/1", form(2, "12", Some("2")))
        .await;
    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "the flag never blocks a save"
    );
    let (revision, data) = saved(&setup).await;
    assert_eq!((revision, data["vitality"].clone()), (3, json!(12)));
    let (_, html) = setup.get("/private-sheets/1").await;
    assert!(
        flag(&html)
            .unwrap()
            .contains("(saved revision 3): 1 check violated.")
    );
}

#[tokio::test]
async fn unknown_input_flags_the_check_not_evaluated() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&formula_definition()).await;
    let (status, _) = setup
        .post(
            &format!(
                "/campaigns/{}/sheets/new?version={}",
                setup.campaign, setup.version
            ),
            form(0, "4", None),
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (_, html) = setup.get("/private-sheets/1").await;
    let shown = flag(&html).expect("an unknown input flags the sheet");
    assert!(
        shown.contains(&format!(
            "{FLAG} (saved revision 1): 1 check not evaluated; 2 derived values not computed."
        )),
        "{shown}"
    );
}

#[tokio::test]
async fn evaluation_error_flags_the_sheet() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let mut definition = formula_definition();
    definition["constraints"]
        .as_array_mut()
        .unwrap()
        .push(json!({
        "id": "ratio", "expr": "10 / vitality >= 0", "rule_ids": [],
        "message": "Ratio is not negative.",
        "evidence": {"state": "unresolved", "reason": "Synthetic"}}));
    let setup = setup(&definition).await;
    setup.create().await;
    let (_, html) = setup.get("/private-sheets/1").await;
    assert_eq!(flag(&html), None, "vitality 4: every check passes");
    let (status, _) = setup
        .post("/private-sheets/1", form(1, "0", Some("2")))
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (_, html) = setup.get("/private-sheets/1").await;
    assert!(html.contains("<code>division_by_zero</code>"));
    let shown = flag(&html).expect("an evaluation error flags the sheet");
    assert!(
        shown.contains(&format!(
            "{FLAG} (saved revision 2): 1 check not evaluated."
        )),
        "{shown}"
    );
}

/// A formula-free runtime definition (no checks) and one with derived values but no
/// checks: rule coverage is unavailable, so the sheet is never shown as validated.
#[tokio::test]
async fn pins_without_checks_flag_the_sheet() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let mut derived_only = formula_definition();
    derived_only["constraints"] = json!([]);
    let mut formula_free = formula_definition();
    let object = formula_free.as_object_mut().unwrap();
    object.remove("formula_profile");
    object.remove("constraints");
    let fields: Vec<Value> = formula_free["fields"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|field| field.get("formula").is_none())
        .cloned()
        .collect();
    let ids: Vec<Value> = fields.iter().map(|field| field["id"].clone()).collect();
    formula_free["fields"] = fields.into();
    formula_free["groups"][0]["fields"] = ids.into();
    for (definition, anchor) in [
        (derived_only, Some("<a href=\"#advisory-checks\">")),
        (formula_free, None),
    ] {
        let setup = setup(&definition).await;
        setup.create().await;
        let (status, html) = setup.get("/private-sheets/1").await;
        assert_eq!(status, StatusCode::OK);
        let shown = flag(&html).expect("no checks: coverage unavailable");
        assert!(
            shown.contains(&format!(
                "{FLAG} (saved revision 1): its definition declares no checks."
            )),
            "{shown}"
        );
        match anchor {
            Some(link) => assert!(shown.contains(link), "{shown}"),
            None => assert!(!shown.contains("<a "), "{shown}"),
        }
    }
}

#[tokio::test]
async fn history_shows_the_flag_per_revision() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&formula_definition()).await;
    setup.create().await;
    let (status, _) = setup
        .post("/private-sheets/1", form(1, "11", Some("2")))
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (status, _) = setup
        .post("/private-sheets/1", form(2, "4", Some("2")))
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let flags = [None, Some("(saved revision 2): 1 check violated."), None];
    for (revision, expected) in (1..=3).zip(flags) {
        let (status, html) = setup
            .get(&format!("/private-sheets/1/history/{revision}"))
            .await;
        assert_eq!(status, StatusCode::OK);
        match expected {
            Some(text) => assert!(flag(&html).unwrap().contains(text), "revision {revision}"),
            None => assert_eq!(flag(&html), None, "revision {revision}"),
        }
    }
    let (_, current) = setup.get("/private-sheets/1").await;
    assert_eq!(flag(&current), None, "the current revision passes again");
}

/// A refused save re-renders the draft beside the saved revision's flag, never a flag of
/// the draft; the create page has no saved revision and shows none.
#[tokio::test]
async fn refused_draft_shows_the_saved_revision_flag_and_create_shows_none() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&formula_definition()).await;
    let create = format!(
        "/campaigns/{}/sheets/new?version={}",
        setup.campaign, setup.version
    );
    let (_, html) = setup.get(&create).await;
    assert_eq!(flag(&html), None);
    setup.create().await;
    let (status, html) = setup
        .post("/private-sheets/1", form(1, &"9".repeat(201), Some("2")))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(flag(&html), None, "the refused draft is not evaluated");
    let (status, _) = setup
        .post("/private-sheets/1", form(1, "11", Some("2")))
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (status, html) = setup
        .post("/private-sheets/1", form(2, &"4".repeat(201), Some("2")))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        flag(&html)
            .unwrap()
            .contains("(saved revision 2): 1 check violated.")
    );
}
