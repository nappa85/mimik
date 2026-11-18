//! Assisted sheet step 3a: linking on view (rules 1 and 2) through the real router, and the
//! catalogue key property that is never a control. Synthetic owner text only.
use super::{
    fixture,
    formula_tests::Setup,
    list_tests::{legacy_create, pair},
    published_fixture, router_request,
};
use crate::{Store, catalogues::PublishRequest};
use axum::http::StatusCode;
use serde_json::{Value, json};

fn mapped() -> Value {
    json!({"state": "mapped", "regions": [{"source_id": 1, "page_id": 1, "unit_id": 1}]})
}

fn entry(key: &str, entry_type: &str, name: &str, retired: bool) -> Value {
    let mut entry = json!({"key": key, "type": entry_type, "name": name, "properties": {},
                           "description": {"owner_text": format!("Synthetic text of {key}.")}});
    if retired {
        entry["retired"] = true.into();
    }
    entry
}

/// `gear`: two entries named Sword of different types, a retired rope, and a name with `ß`.
fn gear() -> Value {
    json!({"contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
           "catalogue_id": "gear", "title": "Synthetic gear", "sources": [],
           "entry_types": [{"id": "tool", "label": "Tool", "properties": []},
                           {"id": "weapon", "label": "Weapon", "properties": []}],
           "entries": [
               entry("shield", "tool", "Shield", false),
               entry("rope", "tool", "Rope", false),
               entry("old-rope", "tool", "Old Rope", true),
               entry("sword-a", "weapon", "Sword", false),
               entry("sword-b", "tool", "Sword", false),
               entry("strasse", "tool", "Stra\u{df}e", false)]})
}

/// A list `equipment` (field 1) whose name carries the member, keyed by `gear_key`.
pub(super) fn definition(entry_type: Option<&str>) -> Value {
    let mut member = json!({"id": "gear", "key": "gear_key"});
    if let Some(entry_type) = entry_type {
        member["entry_type"] = entry_type.into();
    }
    json!({
        "contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
        "definition_id": "gear-sheet", "title": "Gear sheet",
        "sources": [{"source_id": 1, "pdf_sha256": "0".repeat(64), "roles": ["sheet"]}],
        "fields": [
            {"id": "label", "label": "Label", "control": "text", "nullable": false,
             "mapping": mapped()},
            {"id": "equipment", "label": "Equipment", "control": "list", "nullable": false,
             "mapping": mapped(), "items": {"properties": [
                {"id": "name", "label": "Name", "control": "text", "optional": false,
                 "catalogue": member},
                {"id": "quantity", "label": "Quantity", "control": "integer", "optional": false},
                {"id": "gear_key", "label": "Gear key", "control": "text", "optional": true}],
              "quick_add": {"name": "name", "quantity": "quantity"}}}],
        "formula_profile": 1,
        "groups": [{"id": "main", "label": "Main", "fields": ["label", "equipment"],
                    "column_hint": "main"}],
        "catalogues": [{"id": "gear"}]
    })
}

async fn publish_catalogue(store: &Store, value: &Value) {
    let draft = store
        .save_catalogue_draft(serde_json::to_vec(value).unwrap(), 0)
        .await
        .unwrap();
    let comparison = store.compare_catalogue(draft.id).await.unwrap();
    store
        .publish_catalogue(
            draft.id,
            PublishRequest {
                comparison_hash: comparison.comparison_hash,
                actor: "Synthetic operator".into(),
                rationale: "Synthetic catalogue".into(),
            },
        )
        .await
        .unwrap();
}

/// A store with the synthetic source, `catalogue` published, `definition` published against it
/// and a campaign (no sheet yet).
async fn published(definition: &Value, catalogue: &Value) -> Setup {
    let (dir, store, auth, _clock) = fixture().await;
    published_fixture(&store).await;
    publish_catalogue(&store, catalogue).await;
    let draft = store
        .save_definition_draft(serde_json::to_vec(definition).unwrap(), 0)
        .await
        .unwrap();
    let comparison = store.compare_definition(draft.id, false).await.unwrap();
    let version = store
        .approve_definition(
            draft.id,
            false,
            crate::definitions::ApprovalRequest {
                comparison_hash: comparison.comparison_hash,
                actor: "Synthetic test".into(),
                rationale: "Synthetic original compared".into(),
                originals_compared: true,
            },
        )
        .await
        .unwrap()
        .id;
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Gear".into(),
            "synthetic-game".into(),
            "first".into(),
        )
        .await
        .unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    Setup {
        dir,
        store,
        auth,
        app,
        campaign,
        version,
    }
}

/// `published` with the demo definition, and the sheet 1 holding `rows` (name, key) with
/// quantity 1.
async fn world(entry_type: Option<&str>, rows: &[(&str, Option<&str>)]) -> Setup {
    let setup = published(&definition(entry_type), &gear()).await;
    let items: Vec<Value> = rows
        .iter()
        .enumerate()
        .map(|(index, (name, key))| {
            let mut item = json!({"id": format!("r1-{}", index + 1), "name": name, "quantity": 1});
            if let Some(key) = key {
                item["gear_key"] = (*key).into();
            }
            item
        })
        .collect();
    legacy_create(&setup, 1, json!({"label": "Sheet", "equipment": items})).await;
    setup
}

async fn page(setup: &Setup) -> scraper::Html {
    let (status, html) = setup.get("/private-sheets/1").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    scraper::Html::parse_document(&html)
}

/// The entry links of the page: the row they sit on (cell key `1-<r>-0`) and their href.
fn links(page: &scraper::Html) -> Vec<(String, String)> {
    let anchors = scraper::Selector::parse("a.entry-link").unwrap();
    page.select(&anchors)
        .map(|anchor| {
            let element = anchor.value();
            assert_eq!(element.attr("target"), Some("_blank"));
            assert_eq!(element.attr("rel"), Some("noopener"));
            let id = element.attr("id").unwrap().to_owned();
            assert_eq!(
                element.attr("aria-labelledby").unwrap(),
                format!("{id} row-{} catalogue-0", id.trim_start_matches("entry-"))
            );
            (id, element.attr("href").unwrap().to_owned())
        })
        .collect()
}

fn href(key: &str) -> String {
    format!("/private-sheets/1/catalogue/gear/{key}")
}

async fn stored(setup: &Setup) -> Value {
    let snapshot: String = setup
        .store
        .run(|db| {
            Ok(db.query_row(
                "SELECT snapshot FROM private_sheets WHERE id=1",
                [],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    serde_json::from_str(&snapshot).unwrap()
}

#[tokio::test]
async fn rules_one_and_two_link_saved_rows_and_nothing_else() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(
        None,
        &[
            ("SHIELD", None),           // rule 2: the single entry with that name
            ("Sword", Some("sword-b")), // rule 1: the key picks one of two duplicates
            ("sword", None),            // duplicates without a key: no link
            ("Rope", Some("sword-a")),  // the key resolves but its name differs: rule 2
            ("rope", Some("gone")),     // a dangling key: rule 2
            ("Old Rope", None),         // retired: never linked
            ("  ", None),               // blank
            ("STRASSE", None),          // full fold
            ("Rope", Some("old-rope")), // a key naming a retired entry: rule 2
            ("Unknown", None),
        ],
    )
    .await;
    let page = page(&setup).await;
    assert_eq!(
        links(&page),
        [
            ("entry-1-0-0".to_owned(), href("shield")),
            ("entry-1-1-0".to_owned(), href("sword-b")),
            ("entry-1-3-0".to_owned(), href("rope")),
            ("entry-1-4-0".to_owned(), href("rope")),
            ("entry-1-7-0".to_owned(), href("strasse")),
            ("entry-1-8-0".to_owned(), href("rope")),
        ]
    );
    // The panel names the link: the label of the catalogue the aria-labelledby points at.
    let panel = scraper::Selector::parse("li#catalogue-0").unwrap();
    assert!(page.select(&panel).next().is_some());
    // The key property is rendered as the link only: no control, no label, no form key.
    let html = page.html();
    for gone in ["row-1-0-2", "set-1-0-2", "set-1-1-2", "Gear key"] {
        assert!(!html.contains(gone), "{gone}");
    }
}

#[tokio::test]
async fn the_entry_type_filter_decides_which_entries_are_eligible() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(
        Some("weapon"),
        &[
            ("Shield", None),
            ("Sword", Some("sword-b")),
            ("sword", None),
        ],
    )
    .await;
    // Only the weapon is eligible: a tool is never linked, and a key naming a tool is ignored.
    assert_eq!(
        links(&page(&setup).await),
        [
            ("entry-1-1-0".to_owned(), href("sword-a")),
            ("entry-1-2-0".to_owned(), href("sword-a")),
        ]
    );
}

#[tokio::test]
async fn a_failed_lookup_shows_no_links() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Shield", None)]).await;
    assert_eq!(links(&page(&setup).await).len(), 1);
    setup
        .store
        .run(|db| Ok(db.execute_batch("DROP TABLE catalogue_entry_names")?))
        .await
        .unwrap();
    let after = page(&setup).await;
    assert_eq!(links(&after).len(), 0);
    assert!(after.html().contains("Shield"), "the sheet still renders");
}

#[tokio::test]
async fn the_json_answer_carries_the_link_and_a_save_keeps_the_stored_key() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Shield", None), ("Sword", Some("sword-b"))]).await;
    let post = |path: &str, base: i64, value: &str| {
        use tower::ServiceExt;
        let request = axum::http::Request::builder()
            .uri(path.to_owned())
            .method("POST")
            .header("host", "127.0.0.1:3000")
            .header("origin", "http://127.0.0.1:3000")
            .header(
                "content-type",
                "application/x-www-form-urlencoded;charset=UTF-8",
            )
            .header("accept", "application/json")
            .header("sec-fetch-mode", "cors")
            .header("sec-fetch-dest", "empty")
            .header("cookie", setup.auth[1].fixture_cookie())
            .body(axum::body::Body::from(
                serde_urlencoded::to_string([
                    ("csrf", "a".repeat(64)),
                    ("base", base.to_string()),
                    ("value", value.to_owned()),
                ])
                .unwrap(),
            ))
            .unwrap();
        let app = setup.app.clone();
        async move {
            let response = app.oneshot(request).await.unwrap();
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), 1 << 20)
                .await
                .unwrap();
            (status, serde_json::from_slice::<Value>(&body).unwrap())
        }
    };
    // Row 2 (index 1) is renamed: its key no longer matches, rule 2 finds the rope.
    let (status, answer) = post("/private-sheets/1/cells/1/1/0", 1, "Rope").await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["status"], "saved");
    assert_eq!(
        answer["unit"]["link"],
        json!({"href": href("rope"), "catalogue": "0"})
    );
    assert_eq!(
        stored(&setup).await["data"]["equipment"][1]["gear_key"],
        "sword-b",
        "a save keeps the stored key"
    );
    // A name no entry has drops the link (null, so a script removes it).
    let (_, answer) = post("/private-sheets/1/cells/1/1/0", 2, "Nothing here").await;
    assert_eq!(answer["status"], "saved");
    assert_eq!(answer["unit"]["link"], Value::Null);
    // The quantity cell has no link member either way.
    let (_, answer) = post("/private-sheets/1/cells/1/0/1", 3, "4").await;
    assert_eq!(answer["unit"]["link"], Value::Null);
}

fn full_form(extra: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut form = vec![
        pair("csrf", "a".repeat(64)),
        pair("revision", "1"),
        pair("name", "Gear sheet"),
        pair("notes", ""),
        pair("value-label", "Sheet"),
        pair("id-1-0", "r1-1"),
        pair("row-1-0-0", "Shield"),
        pair("row-1-0-1", "5"),
        pair("id-1-1", "r1-2"),
        pair("row-1-1-0", "Rope"),
        pair("row-1-1-1", "1"),
    ];
    form.extend(extra);
    form
}

fn new_row_form(extra: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut form = vec![
        pair("csrf", "a".repeat(64)),
        pair("base", "1"),
        pair("row-0", "Lamp"),
        pair("row-1", "2"),
    ];
    form.extend(extra);
    form
}

#[tokio::test]
async fn no_route_takes_the_key_property_and_every_save_keeps_it() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Shield", Some("shield")), ("Rope", None)]).await;
    let revision = || async { stored(&setup).await["revision"].as_i64().unwrap() };
    let full = full_form;
    // A posted key property (value or presence) is an unknown key: 400, nothing written.
    for key in ["row-1-0-2", "set-1-0-2"] {
        let (status, _) = setup
            .post("/private-sheets/1", full(vec![pair(key, "forged")]))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{key}");
    }
    assert_eq!(revision().await, 1);
    // The new-row form refuses it too; the cell route never takes the key cell.
    let new_row = new_row_form;
    let (status, _) = setup
        .post(
            "/private-sheets/1/lists/1/rows",
            new_row(vec![pair("row-2", "forged")]),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = setup
        .post(
            "/private-sheets/1/cells/1/0/2",
            vec![
                pair("csrf", "a".repeat(64)),
                pair("base", "1"),
                pair("value", "forged"),
                pair("set", "present"),
            ],
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(revision().await, 1);
}

#[tokio::test]
async fn every_save_keeps_the_stored_key_and_new_rows_leave_it_absent() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Shield", Some("shield")), ("Rope", None)]).await;
    let revision = || async { stored(&setup).await["revision"].as_i64().unwrap() };
    let full = full_form;
    let new_row = new_row_form;
    // The full form saves and keeps the stored key.
    let (status, _) = setup.post("/private-sheets/1", full(Vec::new())).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let rows = stored(&setup).await["data"]["equipment"].clone();
    assert_eq!(rows[0]["quantity"], 5);
    assert_eq!(rows[0]["gear_key"], "shield");
    assert!(rows[1].get("gear_key").is_none());
    // A new row and a quick-added row leave the key absent.
    let (status, _) = setup
        .post("/private-sheets/1/lists/1/rows", new_row(Vec::new()))
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let base = revision().await.to_string();
    let (status, _) = setup
        .post(
            "/private-sheets/1/lists/1/quick",
            vec![
                pair("csrf", "a".repeat(64)),
                pair("base", base),
                pair("text", "2 torches"),
            ],
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let rows = stored(&setup).await["data"]["equipment"].clone();
    assert_eq!(rows.as_array().unwrap().len(), 4);
    assert!(rows[2].get("gear_key").is_none() && rows[3].get("gear_key").is_none());
    // A name cell save keeps the stored key as well, and the page shows no key control.
    let base = revision().await.to_string();
    let (status, _) = setup
        .post(
            "/private-sheets/1/cells/1/0/0",
            vec![
                pair("csrf", "a".repeat(64)),
                pair("base", base),
                pair("value", "Shield II"),
            ],
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        stored(&setup).await["data"]["equipment"][0]["gear_key"],
        "shield"
    );
    let response = router_request(&setup.app, &setup.auth[1], "/private-sheets/1", None).await;
    let html = String::from_utf8(
        axum::body::to_bytes(response.into_body(), 1 << 24)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(
        !html.contains("new-1-2") && !html.contains("row-2"),
        "no key control anywhere"
    );
}

/// The worst-case page with every row linked (plan 3a): 256 rows of 32 properties with
/// 64-character IDs and labels of 100 `"`, the name property keyed by a 64-character entry key
/// in a catalogue with a 64-character ID and entry type, so every name cell carries its link.
/// The page stays within the per-unit markup bound that `form_budget` enforces at publication;
/// the bound itself is never enlarged.
#[tokio::test]
async fn worst_label_page_with_every_row_linked_is_within_the_unit_bound() {
    use super::list_tests::{render_units, wide_list_definition, within_unit_bound};
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let catalogue_id = format!("c{}", "a".repeat(63));
    let entry_type = format!("t{}", "b".repeat(63));
    let mut definition = wide_list_definition(false);
    let list = definition["fields"][0]["id"].as_str().unwrap().to_owned();
    let properties = &mut definition["fields"][0]["items"]["properties"];
    let name = properties[0]["id"].as_str().unwrap().to_owned();
    let key = properties[31]["id"].as_str().unwrap().to_owned();
    properties[0]["control"] = "text".into();
    properties[0]["catalogue"] = json!({"id": catalogue_id, "entry_type": entry_type, "key": key});
    properties[31]["control"] = "text".into();
    properties[31]["optional"] = true.into();
    let others: Vec<String> = (1..31)
        .map(|p| properties[p]["id"].as_str().unwrap().to_owned())
        .collect();
    definition["catalogues"] = json!([{"id": catalogue_id}]);
    let entries: Vec<Value> = (1..=256)
        .map(|k| {
            let mut entry = entry(
                &format!("e{k:03}-{}", "k".repeat(59)),
                &entry_type,
                &format!("Item {k:03}"),
                false,
            );
            entry["description"] = json!({"owner_text": "Synthetic."});
            entry
        })
        .collect();
    let mut catalogue = gear();
    catalogue["catalogue_id"] = catalogue_id.clone().into();
    catalogue["entry_types"] = json!([{"id": entry_type, "label": "Gear", "properties": []}]);
    catalogue["entries"] = Value::Array(entries);
    let setup = published(&definition, &catalogue).await;
    let items: Vec<Value> = (1..=256)
        .map(|k| {
            let mut item = json!({"id": format!("i{k:03}-{}", "x".repeat(59)),
                                  name.clone(): format!("Item {k:03}"),
                                  key.clone(): format!("e{k:03}-{}", "k".repeat(59))});
            for other in &others {
                item[other] = 0.into();
            }
            item
        })
        .collect();
    legacy_create(&setup, 1, json!({list: items})).await;
    let response = router_request(&setup.app, &setup.auth[1], "/private-sheets/1", None).await;
    let html = String::from_utf8(
        axum::body::to_bytes(response.into_body(), 256 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert_eq!(html.matches("class=\"entry-link\"").count(), 256);
    assert!(
        !html.contains("row-0-0-31"),
        "the key property is no control"
    );
    let units = render_units(&definition, &[256]);
    within_unit_bound(&html, units, "every row linked, 64-character IDs and keys");
}

fn refusal(error: crate::definitions::DefinitionError) -> String {
    match error {
        crate::definitions::DefinitionError::Invalid(report) => format!(
            "{} {}",
            report.diagnostics[0].path, report.diagnostics[0].code
        ),
        other => format!("other {other}"),
    }
}

/// The entry type is checked when a draft is saved and in the publication snapshot, against the
/// linked catalogue's latest sealed version; readiness stays existence-only (R27, D8).
#[tokio::test]
async fn the_entry_type_is_checked_at_draft_save_and_publication_but_not_by_readiness() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = published(&definition(Some("weapon")), &gear()).await;
    legacy_create(
        &setup,
        1,
        json!({"label": "Sheet", "equipment": [{"id": "r1-1", "name": "Sword", "quantity": 1}]}),
    )
    .await;
    assert_eq!(
        links(&page(&setup).await).len(),
        1,
        "linked while the type exists"
    );
    let path = "/fields/1/items/properties/0/catalogue linked-catalogue-entry-type-unknown";
    // Draft save: an undeclared type is refused, a declared one saves.
    let mut unknown = definition(Some("nothing"));
    unknown["definition_id"] = "other-sheet".into();
    let saved = setup
        .store
        .save_definition_draft(serde_json::to_vec(&unknown).unwrap(), 0)
        .await;
    assert_eq!(refusal(saved.unwrap_err()), path);
    let mut known = definition(Some("tool"));
    known["definition_id"] = "other-sheet".into();
    let draft = setup
        .store
        .save_definition_draft(serde_json::to_vec(&known).unwrap(), 0)
        .await
        .unwrap();
    // Publication snapshot: with the type gone from the catalogue (tampered; a version cannot
    // drop a type), comparing the draft is refused with the same code.
    setup
        .store
        .run(|db| {
            db.execute_batch(
                "PRAGMA foreign_keys=OFF; DROP TRIGGER catalogue_entry_types_update; UPDATE catalogue_entry_types SET type_id='moved' WHERE type_id='tool'; PRAGMA foreign_keys=ON;",
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let compared = setup.store.compare_definition(draft.id, false).await;
    assert_eq!(refusal(compared.unwrap_err()), path);
    // Readiness: the published version names `weapon`. With that type gone from the catalogue
    // (tampered; a version cannot drop a type) the sheet page still renders: 200, no stale
    // wiring warning, and no entry links (nothing is eligible). The type check is not part of
    // the snapshot that readiness and the wiring status recompute.
    setup
        .store
        .run(|db| {
            db.execute_batch(
                "PRAGMA foreign_keys=OFF; DROP TRIGGER catalogue_entries_update; UPDATE catalogue_entry_types SET type_id='lost' WHERE type_id='weapon'; UPDATE catalogue_entries SET type_id='lost' WHERE type_id='weapon'; PRAGMA foreign_keys=ON;",
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let (status, html) = setup.get("/private-sheets/1").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(
        !html.contains("Wiring approval is stale"),
        "readiness is existence-only"
    );
    assert!(!html.contains("entry-link"));
}

/// The read-only rendering (a frozen sheet) carries the same link, and a
/// history page, which reads no catalogue through its own pin, carries none.
#[tokio::test]
async fn read_only_pages_link_too_and_history_pages_do_not() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = published(&definition(None), &gear()).await;
    setup
        .store
        .request_join(setup.auth[2].clone(), setup.campaign)
        .await
        .unwrap();
    setup
        .store
        .decide_join(setup.auth[1].clone(), setup.campaign, 3, 1, true)
        .await
        .unwrap();
    legacy_create(
        &setup,
        2,
        json!({"label": "Sheet", "equipment": [{"id": "r1-1", "name": "Shield", "quantity": 1}]}),
    )
    .await;
    let campaign = setup.campaign;
    let revision: i64 = setup
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
    // The custodian departs: the sheet is frozen and read-only for everyone.
    setup
        .store
        .depart(setup.auth[1].clone(), setup.campaign, 3, revision)
        .await
        .unwrap();
    let (status, html) = setup.get("/private-sheets/1").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(
        !html.contains("/private-sheets/1/cells/1/0/0"),
        "a frozen sheet: the read-only rendering"
    );
    assert_eq!(
        links(&scraper::Html::parse_document(&html)),
        [("entry-1-0-0".to_owned(), href("shield"))]
    );
    let (status, history) = setup.get("/private-sheets/1/history/1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!history.contains("entry-link"));
}

/// Moving, removing and restoring a row keep the stored key of every row.
#[tokio::test]
async fn row_moves_remove_and_undo_keep_the_stored_key() {
    use super::assist_tests::click;
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Shield", Some("shield")), ("Rope", Some("rope"))]).await;
    let keys = || async {
        stored(&setup).await["data"]["equipment"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row["name"].as_str().unwrap().to_owned(),
                    row["gear_key"].clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    let row_action = |action: &'static str, row: usize| {
        let setup = &setup;
        async move {
            let base = stored(setup).await["revision"]
                .as_i64()
                .unwrap()
                .to_string();
            setup
                .post(
                    &format!("/private-sheets/1/lists/1/rows/{row}/{action}"),
                    vec![pair("csrf", "a".repeat(64)), pair("base", base)],
                )
                .await
                .0
        }
    };
    assert_eq!(row_action("down", 0).await, StatusCode::SEE_OTHER);
    assert_eq!(
        keys().await,
        [
            ("Rope".to_owned(), json!("rope")),
            ("Shield".to_owned(), json!("shield"))
        ]
    );
    assert_eq!(row_action("up", 1).await, StatusCode::SEE_OTHER);
    assert_eq!(keys().await[0].0, "Shield");
    assert_eq!(row_action("remove", 0).await, StatusCode::SEE_OTHER);
    assert_eq!(keys().await, [("Rope".to_owned(), json!("rope"))]);
    let (_, html) = setup.get("/private-sheets/1").await;
    assert_eq!(
        click(&setup, &html, "undo-row-1").await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        keys().await,
        [
            ("Shield".to_owned(), json!("shield")),
            ("Rope".to_owned(), json!("rope"))
        ]
    );
}

/// A lookup that exceeds its budget shows no links for the list (the page still renders).
#[tokio::test]
async fn a_lookup_past_its_budget_gives_no_links() {
    use crate::definitions::private_pin;
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Shield", None)]).await;
    let version = setup.version;
    let data = stored(&setup).await["data"].clone();
    let counts = setup
        .store
        .run(move |db| {
            let tx = db.transaction()?;
            let pin = private_pin::validate(private_pin::load(&tx, version)?)?;
            let within = super::super::catalogue::entry_links_within(
                &tx,
                &pin,
                &data,
                (std::time::Duration::from_secs(60), Some(1)),
            );
            let past = super::super::catalogue::entry_links_within(
                &tx,
                &pin,
                &data,
                (std::time::Duration::ZERO, Some(1)),
            );
            Ok((within.len(), past.len()))
        })
        .await
        .unwrap();
    assert_eq!(counts, (1, 0));
}
