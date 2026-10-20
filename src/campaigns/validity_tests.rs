//! Slice 1.7 validity flag (RR-3, ASSIST-4): a saved revision shows "Not validated" in the
//! sheet header when a check is violated, not evaluated or in error, or when no checks cover
//! its pin. Computed on view from the saved revision, never stored; it never blocks a save
//! and never hides or changes a value.
use super::cell_tests::{document_post, hold};
use super::formula_tests::{Setup, body, form, formula_definition, setup};
use axum::http::StatusCode;
use serde_json::{Value, json};

const FLAG: &str = "Not validated";

/// The flag paragraph, or `None` when the page shows no flag.
fn flag(html: &str) -> Option<&str> {
    let start = html.find("<p id=\"validity\"")?;
    let end = start + html[start..].find("</p>").unwrap();
    Some(&html[start..end])
}

/// The flag's text without its tags, or `None` when the page shows no flag.
fn words(html: &str) -> Option<String> {
    let shown = flag(html)?;
    let mut text = String::new();
    let mut tag = false;
    for c in shown.chars() {
        match c {
            '<' => tag = true,
            '>' => tag = false,
            _ if !tag => text.push(c),
            _ => {}
        }
    }
    Some(text)
}

/// A unit form as posted by hand (`csrf`, `base`, `value`, plus `extra`).
fn unit(base: &str, value: &str, extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut pairs = vec![
        ("csrf".to_owned(), "a".repeat(64)),
        ("base".to_owned(), base.to_owned()),
        ("value".to_owned(), value.to_owned()),
    ];
    pairs.extend(
        extra
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
    );
    pairs
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
        words(&html)
            .unwrap()
            .starts_with(&format!("{FLAG} (saved revision 2): 1 check violated.")),
        "{shown}"
    );
    assert!(shown.contains("<strong>Not validated</strong>"), "in words");
    assert!(shown.contains("<a href=\"#advisory-checks\">1 check violated</a>"));
    assert!(shown.contains("This flag never blocks saving."));
    // In the header: before any form, so it reads first without script or styles.
    assert!(html.find("id=\"validity\"").unwrap() < html.find("<form").unwrap());
    assert!(
        !html
            .replace(super::script_tests::MODULE_TAG, "")
            .contains("<script"),
        "the sheet page loads only the sheet module, and the flag needs none"
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
        words(&html)
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
    assert_eq!(
        words(&html).unwrap(),
        format!(
            "{FLAG} (saved revision 1): 1 check not evaluated; 2 derived values not computed. \
             This flag never blocks saving."
        ),
    );
    // Each reason links to the section that explains it.
    assert!(shown.contains("<a href=\"#advisory-checks\">1 check not evaluated</a>"));
    assert!(shown.contains("<a href=\"#formula-results\">2 derived values not computed</a>"));
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
    assert!(flag(&html).is_some(), "an evaluation error flags the sheet");
    assert_eq!(
        words(&html).unwrap(),
        format!("{FLAG} (saved revision 2): 1 check not evaluated. This flag never blocks saving.")
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
        (
            derived_only,
            Some("<a href=\"#advisory-checks\">its definition declares no checks</a>"),
        ),
        (formula_free, None),
    ] {
        let setup = setup(&definition).await;
        setup.create().await;
        let (status, html) = setup.get("/private-sheets/1").await;
        assert_eq!(status, StatusCode::OK);
        let shown = flag(&html).expect("no checks: coverage unavailable");
        assert!(
            words(&html).unwrap().starts_with(&format!(
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
            Some(text) => assert!(words(&html).unwrap().contains(text), "revision {revision}"),
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
        words(&html)
            .unwrap()
            .contains("(saved revision 2): 1 check violated.")
    );
    // A stale full-form save (409) re-renders a passing draft beside the saved flag.
    let (status, html) = setup
        .post("/private-sheets/1", form(1, "4", Some("2")))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(html.contains("value=\"4\""), "the draft is kept");
    assert!(
        words(&html)
            .unwrap()
            .contains("(saved revision 2): 1 check violated."),
        "the 409 flag is the saved revision's"
    );
    // A refused unit save (422) of a passing value, the same.
    let (status, html) = setup
        .post(
            "/private-sheets/1/fields/vitality",
            unit("2", &"4".repeat(201), &[]),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(html.contains(&"4".repeat(201)), "the unit draft is kept");
    assert!(
        words(&html)
            .unwrap()
            .contains("(saved revision 2): 1 check violated."),
        "the unit 422 flag is the saved revision's"
    );
}

/// Sol review: every check passes but an unrelated derived value fails (`10 / vitality`
/// with vitality 0). The flag names only that reason and links to the derived values
/// section, which says where each failure is explained.
#[tokio::test]
async fn a_failed_derived_value_alone_flags_and_its_link_explains_it() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let mut definition = formula_definition();
    definition["fields"].as_array_mut().unwrap().push(json!({
        "id": "ratio", "label": "Ratio", "control": "integer", "nullable": false,
        "mapping": {"state": "mapped", "regions": [{"source_id": 1, "page_id": 1, "unit_id": 1}]},
        "formula": {"expr": "10 / vitality", "rule_ids": [],
                    "evidence": {"state": "unresolved", "reason": "Synthetic"}}}));
    definition["groups"][0]["fields"]
        .as_array_mut()
        .unwrap()
        .push("ratio".into());
    let setup = setup(&definition).await;
    setup.create().await;
    let (_, html) = setup.get("/private-sheets/1").await;
    assert_eq!(
        flag(&html),
        None,
        "vitality 4: everything computed and passed"
    );
    let (status, _) = setup
        .post("/private-sheets/1", form(1, "0", Some("2")))
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (_, html) = setup.get("/private-sheets/1").await;
    assert!(html.contains("0 violated, 0 not evaluated, 2 passed."));
    assert_eq!(
        words(&html).unwrap(),
        format!(
            "{FLAG} (saved revision 2): 1 derived value not computed. This flag never blocks saving."
        )
    );
    let shown = flag(&html).unwrap();
    assert!(
        shown.contains("<a href=\"#formula-results\">1 derived value not computed</a>"),
        "{shown}"
    );
    // The destination explains it: it names the failed value and links to its reason.
    let start = html.find("id=\"formula-results\"").unwrap();
    let results = &html[start..start + html[start..].find("</section>").unwrap()];
    assert!(
        results.contains(
            "1 derived value not computed: each shows \u{201c}Not computed\u{201d} and its \
             reason next to its field, starting with <a href=\"#label-ratio\">Ratio</a>."
        ),
        "{results}"
    );
    assert!(html.contains("<dt id=\"label-ratio\">"));
    assert!(html.contains("<code>division_by_zero</code>"));
}

/// The busy page (503) renders no sheet and so no validity flag.
#[tokio::test]
async fn the_busy_page_shows_no_validity_flag() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&formula_definition()).await;
    setup.create().await;
    let (status, _) = setup
        .post("/private-sheets/1", form(1, "11", Some("2")))
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let holder = hold(3);
    let response = document_post(
        &setup,
        "/private-sheets/1/fields/vitality",
        unit("2", "12", &[]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let busy = body(response).await;
    assert!(busy.contains("Server busy: nothing was saved."), "{busy}");
    assert_eq!(flag(&busy), None);
    assert!(!busy.contains("Not validated"));
    holder.await.unwrap();
}
