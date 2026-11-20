//! Assisted sheet step 4, items 1, 2 and 6: the warnings of linking rules 3 and 4 on the page
//! and in the JSON answer, and the "Which one?" route, through the real router.
use super::{
    catalogue_member_tests::{entry, gear, page, published, stored, world},
    formula_tests::Setup,
    interpret_route_tests::send,
    list_tests::{legacy_create, pair},
};
use axum::http::StatusCode;
use scraper::{Html, Selector};
use serde_json::{Value, json};

fn select<'a>(page: &'a Html, css: &str) -> Vec<scraper::ElementRef<'a>> {
    page.select(&Selector::parse(css).unwrap()).collect()
}

/// The warnings of the page: the cell key (`1-<r>-0`), its text and hrefs.
fn warnings(page: &Html) -> Vec<(String, String, Vec<String>)> {
    select(page, "p.entry-match")
        .into_iter()
        .map(|warning| {
            let hrefs = warning
                .select(&Selector::parse("a").unwrap())
                .map(|anchor| anchor.value().attr("href").unwrap().to_owned())
                .collect();
            (
                warning
                    .value()
                    .attr("id")
                    .unwrap()
                    .trim_start_matches("match-")
                    .to_owned(),
                warning
                    .text()
                    .collect::<String>()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
                hrefs,
            )
        })
        .collect()
}

#[tokio::test]
async fn rules_three_and_four_warn_without_echoing_the_name() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(
        None,
        &[
            ("Sword", None),            // several
            ("Zzyzx unknown", None),    // none
            ("Old Rope", None),         // only a retired entry
            ("Sword", Some("sword-b")), // rule 1: linked, no warning
            ("Shield", None),           // rule 2: linked, no warning
            ("  ", None),               // blank: nothing
        ],
    )
    .await;
    let page = page(&setup).await;
    assert_eq!(
        warnings(&page),
        [
            (
                "1-0-0".to_owned(),
                "Several catalogue entries have this name. Which one?".to_owned(),
                vec!["/private-sheets/1/cells/1/0/0/which?base=1".to_owned()]
            ),
            (
                "1-1-0".to_owned(),
                "Not in the catalogue. Browse".to_owned(),
                vec!["/private-sheets/1/catalogue/gear".to_owned()]
            ),
            (
                "1-2-0".to_owned(),
                "No current entry of this type has this name. Browse".to_owned(),
                vec!["/private-sheets/1/catalogue/gear".to_owned()]
            ),
        ]
    );
    // The warning names nobody's text, and the control is described by it.
    let html = page.html();
    assert!(!html.contains("class=\"entry-match\" id=\"match-1-1-0\">Zzyzx"));
    let described = select(&page, "input#row-1-0-0")[0]
        .value()
        .attr("aria-describedby")
        .unwrap()
        .to_owned();
    assert!(
        described.split(' ').any(|id| id == "match-1-0-0"),
        "{described}"
    );
    // Browse and the which link: a new tab for Browse only, `rel=noopener`.
    let browse = select(&page, "p#match-1-1-0 a")[0].value().to_owned();
    assert_eq!(browse.attr("target"), Some("_blank"));
    assert_eq!(browse.attr("rel"), Some("noopener"));
    assert!(
        select(&page, "p#match-1-0-0 a.which")[0]
            .value()
            .attr("target")
            .is_none()
    );
    // The suggest route sits on each name input and the new-row name, never on the others.
    let suggest: Vec<_> = select(&page, "input[data-suggest]")
        .into_iter()
        .map(|input| input.value().attr("data-suggest").unwrap().to_owned())
        .collect();
    assert_eq!(suggest.len(), 7, "six rows and the new row");
    assert!(
        suggest
            .iter()
            .all(|path| path == "/private-sheets/1/suggest/1/0")
    );
}

#[tokio::test]
async fn the_entry_type_decides_the_wording_and_the_browse_filter() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(
        Some("weapon"),
        &[("Shield", None), ("Nothing", None), ("Sword", None)],
    )
    .await;
    let found = warnings(&page(&setup).await);
    assert_eq!(
        found,
        [
            (
                "1-0-0".to_owned(),
                "No current entry of this type has this name. Browse".to_owned(),
                vec!["/private-sheets/1/catalogue/gear?type=weapon".to_owned()]
            ),
            (
                "1-1-0".to_owned(),
                "Not in the catalogue. Browse".to_owned(),
                vec!["/private-sheets/1/catalogue/gear?type=weapon".to_owned()]
            ),
        ],
        "the weapon Sword links (rule 2)"
    );
}

#[tokio::test]
async fn a_failed_lookup_shows_no_warning_either() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Zzyzx unknown", None), ("Sword", None)]).await;
    assert_eq!(warnings(&page(&setup).await).len(), 2);
    setup
        .store
        .run(|db| Ok(db.execute_batch("DROP TABLE catalogue_entry_names")?))
        .await
        .unwrap();
    let after = page(&setup).await;
    assert!(
        warnings(&after).is_empty(),
        "never \"Not in the catalogue\" after a failure"
    );
    assert!(after.html().contains("Zzyzx unknown"));
}

async fn post_json(setup: &Setup, path: &str, base: i64, value: &str) -> Value {
    let (status, answer, _) = send(
        setup,
        "POST",
        path,
        Some(&vec![
            pair("csrf", "a".repeat(64)),
            pair("base", base.to_string()),
            pair("value", value),
        ]),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    answer
}

#[tokio::test]
async fn the_json_answer_carries_the_match_of_a_saved_name() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Shield", None)]).await;
    let path = "/private-sheets/1/cells/1/0/0";
    let answer = post_json(&setup, path, 1, "Sword").await;
    assert_eq!(
        answer["unit"]["match"],
        json!({"kind": "several", "which": "/private-sheets/1/cells/1/0/0/which?base=2"})
    );
    assert_eq!(answer["unit"]["link"], Value::Null);
    let answer = post_json(&setup, path, 2, "Nothing here").await;
    assert_eq!(
        answer["unit"]["match"],
        json!({"kind": "none", "browse": "/private-sheets/1/catalogue/gear"})
    );
    let answer = post_json(&setup, path, 3, "Old Rope").await;
    assert_eq!(
        answer["unit"]["match"],
        json!({"kind": "none", "browse": "/private-sheets/1/catalogue/gear", "current": false})
    );
    let answer = post_json(&setup, path, 4, "Rope").await;
    assert_eq!(answer["unit"]["match"], Value::Null);
    assert_ne!(answer["unit"]["link"], Value::Null);
}

fn which(row: usize, property: usize, base: i64) -> String {
    format!("/private-sheets/1/cells/1/{row}/{property}/which?base={base}")
}

#[tokio::test]
async fn which_one_lists_the_duplicates_with_what_tells_them_apart() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Shield", None), ("Sword", None)]).await;
    let (status, html) = setup.get(&which(1, 0, 1)).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    let page = Html::parse_document(&html);
    let forms = select(&page, "ul.which-list form");
    assert_eq!(forms.len(), 2);
    let fields = |form: &scraper::ElementRef<'_>| -> Vec<(String, String)> {
        form.select(&Selector::parse("input").unwrap())
            .map(|input| {
                (
                    input.value().attr("name").unwrap().to_owned(),
                    input.value().attr("value").unwrap().to_owned(),
                )
            })
            .collect()
    };
    for (form, key) in forms.iter().zip(["sword-a", "sword-b"]) {
        assert_eq!(
            form.value().attr("action"),
            Some("/private-sheets/1/cells/1/1/0")
        );
        assert_eq!(
            fields(form),
            [
                ("csrf".to_owned(), "a".repeat(64)),
                ("base".to_owned(), "1".to_owned()),
                ("value".to_owned(), "Sword".to_owned()),
                ("entry".to_owned(), key.to_owned()),
            ]
        );
        let text = form.text().collect::<String>();
        assert!(
            text.contains(key) && text.contains(&format!("Synthetic text of {key}")),
            "{text}"
        );
        let open = form
            .select(&Selector::parse("a").unwrap())
            .next()
            .unwrap()
            .value()
            .to_owned();
        assert_eq!(
            open.attr("href"),
            Some(format!("/private-sheets/1/catalogue/gear/{key}").as_str())
        );
        assert_eq!(open.attr("rel"), Some("noopener"));
    }
    assert!(html.contains("<bdi>Weapon</bdi>") && html.contains("<bdi>Tool</bdi>"));
    // Posting a candidate's form stores the pick and links the row, without script.
    let form = vec![
        pair("csrf", "a".repeat(64)),
        pair("base", "1"),
        pair("value", "Sword"),
        pair("entry", "sword-b"),
    ];
    let (status, _) = setup.post("/private-sheets/1/cells/1/1/0", form).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        stored(&setup).await["data"]["equipment"][1]["gear_key"],
        "sword-b"
    );
    // The JSON answer for the module.
    let (status, answer, _) = send(&setup, "GET", &which(1, 0, 2), None, true).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "linked by key now: no longer rule 3: {answer}"
    );
}

#[tokio::test]
async fn which_one_answers_json_for_the_module() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Sword", None)]).await;
    let (status, answer, headers) = send(&setup, "GET", &which(0, 0, 1), None, true).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(headers["content-type"], "application/json");
    assert_eq!(answer["status"], "ok");
    assert_eq!(answer["more"], false);
    assert_eq!(answer["browse"], Value::Null);
    let keys: Vec<&str> = answer["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|one| one["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["sword-a", "sword-b"]);
    assert_eq!(answer["candidates"][0]["type"], "Weapon");
    assert_eq!(
        answer["candidates"][0]["open"],
        "/private-sheets/1/catalogue/gear/sword-a"
    );
    assert!(
        answer["candidates"][1]["hint"]
            .as_str()
            .unwrap()
            .contains("Synthetic text of sword-b")
    );
}

#[tokio::test]
async fn a_changed_row_answers_404_never_another_rows_candidates() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Sword", None), ("Sword", None)]).await;
    // A Move in another tab: the row at base index 0 is found by its host ID at its new place,
    // and the forms post there.
    let (status, _) = setup
        .post(
            "/private-sheets/1/lists/1/rows/0/down",
            vec![pair("csrf", "a".repeat(64)), pair("base", "1")],
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (status, html) = setup.get(&which(0, 0, 1)).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains("action=\"/private-sheets/1/cells/1/1/0\""));
    assert!(
        html.contains("name=\"base\" value=\"2\""),
        "the live revision"
    );
    // The row at base index 0 is renamed meanwhile (host r1-1 now at index 1): 404.
    let (status, _) = setup
        .post(
            "/private-sheets/1/cells/1/1/0",
            vec![
                pair("csrf", "a".repeat(64)),
                pair("base", "2"),
                pair("value", "Swordfish"),
            ],
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (status, html) = setup.get(&which(0, 0, 1)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(html.contains("This row changed since the page was loaded"));
    assert!(
        html.contains("href=\"/private-sheets/1#label-1-0-0\""),
        "a link back"
    );
    // A removed row: 404 too; and a base past the live revision names no row.
    let (status, _) = setup
        .post(
            "/private-sheets/1/lists/1/rows/0/remove",
            vec![pair("csrf", "a".repeat(64)), pair("base", "3")],
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(setup.get(&which(1, 0, 3)).await.0, StatusCode::NOT_FOUND);
    assert_eq!(setup.get(&which(0, 0, 99)).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn which_one_refuses_malformed_addresses_and_uniformly_everything_else() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Sword", None)]).await;
    for path in [
        "/private-sheets/1/cells/x/0/0/which?base=1",
        "/private-sheets/1/cells/1/00/0/which?base=1",
        "/private-sheets/1/cells/1/0/0/which?base=0",
        "/private-sheets/1/cells/1/0/0/which?base=1&base=1",
        "/private-sheets/1/cells/1/0/0/which?base=1&x=1",
        "/private-sheets/1/cells/1/0/0/which",
    ] {
        assert_eq!(setup.get(path).await.0, StatusCode::BAD_REQUEST, "{path}");
    }
    // Quantity (no member), the key property, a scalar field, a forged list, an unknown sheet.
    for path in [
        which(0, 1, 1),
        which(0, 2, 1),
        "/private-sheets/1/cells/0/0/0/which?base=1".to_owned(),
        "/private-sheets/1/cells/7/0/0/which?base=1".to_owned(),
        "/private-sheets/99/cells/1/0/0/which?base=1".to_owned(),
    ] {
        let (status, html) = setup.get(&path).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        assert!(html.contains("can’t be listed from here"), "{path}");
    }
    // A reader who may not edit (the owner account reads no sheet; account 3 is no member).
    let stranger = Setup {
        auth: vec![setup.auth[0].clone(), setup.auth[2].clone()],
        dir: tempfile::tempdir().unwrap(),
        store: setup.store.clone(),
        app: setup.app.clone(),
        campaign: setup.campaign,
        version: setup.version,
    };
    assert_eq!(stranger.get(&which(0, 0, 1)).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn names_with_markup_are_escaped_in_the_warning_page_and_the_which_page() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let hostile = "Crow <i>\"x\"</i> & co";
    let mut catalogue = gear();
    let entries = catalogue["entries"].as_array_mut().unwrap();
    for key in ["crow-a", "crow-b"] {
        entries.push(entry(key, "tool", hostile, false));
    }
    let setup = published(&super::catalogue_member_tests::definition(None), &catalogue).await;
    legacy_create(
        &setup,
        1,
        json!({"label": "Sheet", "equipment": [{"id": "r1-1", "name": hostile, "quantity": 1}]}),
    )
    .await;
    let (_, sheet) = setup.get("/private-sheets/1").await;
    assert!(!sheet.contains("<i>\"x\"</i>"));
    let (status, html) = setup.get(&which(0, 0, 1)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!html.contains("<i>"), "{html}");
    assert!(html.contains("<bdi>Crow &#60;i&#62;&#34;x&#34;&#60;/i&#62; &#38; co</bdi>"));
}

#[tokio::test]
async fn more_than_fifty_candidates_are_capped_at_fifty_and_say_so() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let mut catalogue = gear();
    let entries = catalogue["entries"].as_array_mut().unwrap();
    for n in 0..60 {
        entries.push(entry(&format!("dup-{n:02}"), "tool", "Dagger", false));
    }
    let setup = published(&super::catalogue_member_tests::definition(None), &catalogue).await;
    legacy_create(
        &setup,
        1,
        json!({"label": "Sheet", "equipment": [{"id": "r1-1", "name": "Dagger", "quantity": 1}]}),
    )
    .await;
    let (status, html) = setup.get(&which(0, 0, 1)).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    let page = Html::parse_document(&html);
    let keys: Vec<String> = select(&page, "ul.which-list form input[name=entry]")
        .iter()
        .map(|input| input.value().attr("value").unwrap().to_owned())
        .collect();
    assert_eq!(keys.len(), 50, "capped at 50");
    assert_eq!(
        (keys[0].as_str(), keys[49].as_str()),
        ("dup-00", "dup-49"),
        "by (name, key)"
    );
    assert!(html.contains("More entries share this name than are listed."));
    assert!(html.contains("href=\"/private-sheets/1/catalogue/gear\""));
    let (_, answer, _) = send(&setup, "GET", &which(0, 0, 1), None, true).await;
    assert_eq!(answer["candidates"].as_array().unwrap().len(), 50);
    assert_eq!(answer["more"], true);
    assert_eq!(answer["browse"], "/private-sheets/1/catalogue/gear");
}
