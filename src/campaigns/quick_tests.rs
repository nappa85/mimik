//! Slice 1.6 "Quick-add" (`docs/assisted-entry.md` "Quick add"; D-C, D-S, ASSIST-3): a list
//! whose definition declares `items.quick_add` gets one line under its rows. "3 torches" +
//! Enter posts `lists/{f}/quick` with exactly `csrf`, `base` and `text` and appends one row
//! (name and count read by a fixed rule, every other required number unknown, no note), in
//! one revision. It protects nothing, like the new-row form.
use super::cell_tests::{document_post, gear, gear_sheet, hold, kept, many, page, three_rows};
use super::formula_tests::{Setup, body, setup};
use super::notes_tests::{assist, create_path, form, history_len, notes_at, saved};
use super::unit_tests::unit_form;
use axum::http::StatusCode;
use serde_json::{Value, json};

type Pairs = Vec<(String, String)>;

fn pair(key: &str, value: &str) -> (String, String) {
    (key.into(), value.into())
}

/// The assist definition with a quick-add line on Equipment (field 4): `name` and
/// `quantity`, or `member` instead.
fn quick_assist(member: Option<Value>) -> Value {
    let mut definition = assist();
    definition["fields"][4]["items"]["quick_add"] =
        member.unwrap_or_else(|| json!({"name": "name", "quantity": "quantity"}));
    definition
}

/// The quick-add form of list `f` with `typed` in its line: its action and what Enter
/// submits.
fn quick(html: &str, f: usize, typed: &str) -> (String, Pairs) {
    unit_form(html, &format!("quick-{f}"), Some(typed))
}

/// Types `typed` into Equipment's quick-add line of `html` and presses Enter.
async fn add(setup: &Setup, html: &str, typed: &str) -> axum::response::Response {
    let (action, pairs) = quick(html, 4, typed);
    super::router_request(&setup.app, &setup.auth[1], &action, Some(pairs)).await
}

/// The accessible name `aria-labelledby` gives the element `id`.
fn labelled(html: &str, id: &str) -> String {
    let page = scraper::Html::parse_document(html);
    let element = page
        .select(&scraper::Selector::parse(&format!("[id=\"{id}\"]")).unwrap())
        .next()
        .unwrap_or_else(|| panic!("no element {id:?}"));
    element
        .value()
        .attr("aria-labelledby")
        .unwrap_or_else(|| panic!("{id} has no aria-labelledby"))
        .split(' ')
        .map(|part| super::assist_tests::text(html, &format!("[id=\"{part}\"]")).unwrap())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The ids of the page's `autofocus` elements.
fn autofocused(html: &str) -> Vec<String> {
    let page = scraper::Html::parse_document(html);
    page.select(&scraper::Selector::parse("[autofocus]").unwrap())
        .map(|element| element.value().attr("id").unwrap_or_default().to_owned())
        .collect()
}

/// The value of the input `id`.
fn value_of(html: &str, id: &str) -> Option<String> {
    let page = scraper::Html::parse_document(html);
    page.select(&scraper::Selector::parse(&format!("input[id=\"{id}\"]")).unwrap())
        .next()
        .map(|input| input.value().attr("value").unwrap_or_default().to_owned())
}

fn equipment(data: &Value) -> &[Value] {
    data["equipment"].as_array().unwrap()
}

/// "3 torches" + Enter adds torches ×3 and "healing potion x2" the potion ×2: one revision
/// each, no note, the weight (another required number) unknown and the optional Broken not
/// set. The line is named "Quick add Equipment", its button "Add Equipment", and the page
/// comes back with the line focused for the next item.
#[tokio::test]
async fn three_torches_and_healing_potion_x2_each_add_one_row() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&quick_assist(None)).await;
    three_rows(&setup).await;
    let html = page(&setup).await;
    let (action, pairs) = quick(&html, 4, "3 torches");
    assert_eq!(action, "/private-sheets/1/lists/4/quick");
    let keys: Vec<&str> = pairs.iter().map(|(key, _)| key.as_str()).collect();
    assert_eq!(keys, ["csrf", "base", "text"]);
    assert_eq!(labelled(&html, "quick-4"), "Quick add Equipment");
    assert_eq!(labelled(&html, "quick-add-4"), "Add Equipment");
    assert!(
        html.contains("aria-describedby=\"quick-hint-4\""),
        "the line is described by its example"
    );
    assert!(
        super::assist_tests::text(&html, "#quick-hint-4")
            .unwrap()
            .contains("3 torches")
    );
    let before = history_len(&setup, 1).await;
    let response = super::router_request(&setup.app, &setup.auth[1], &action, Some(pairs)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers()["location"],
        "/private-sheets/1?focus=quick-4"
    );
    let (revision, data) = saved(&setup, 1).await;
    assert_eq!(revision, 2);
    assert_eq!(history_len(&setup, 1).await, before + 1, "one revision");
    assert_eq!(
        equipment(&data)[3],
        json!({"id": "r2-1", "name": "torches", "quantity": 3, "weight": null})
    );
    assert_eq!(equipment(&data).len(), 4);
    assert_eq!(notes_at(&setup, 1, 2).await, vec![], "no note");
    let (status, focused) = setup.get("/private-sheets/1?focus=quick-4").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(autofocused(&focused), ["quick-4"]);
    assert_eq!(value_of(&focused, "quick-4").as_deref(), Some(""));
    let response = add(&setup, &focused, "healing potion x2").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        equipment(&saved(&setup, 1).await.1)[4],
        json!({"id": "r3-1", "name": "healing potion", "quantity": 2, "weight": null})
    );
    assert_eq!(notes_at(&setup, 1, 3).await, vec![]);
}

/// The fixed reading of the line: a leading count of 1–6 ASCII digits and a space, or white
/// space, a trailing `x` or `×` and 1–6 digits (so "Box 2" and "Hex 5" are never split); anything else is all name with a count of 1. The
/// name is kept as typed (trimmed); a blank line is "Nothing to add" and writes nothing.
#[tokio::test]
async fn the_line_is_read_by_a_fixed_rule() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&quick_assist(None)).await;
    three_rows(&setup).await;
    let cases: [(&str, &str, i64); 23] = [
        ("rope", "rope", 1),
        ("  12   arrows  ", "arrows", 12),
        ("007 bolts", "bolts", 7),
        ("lamp oil ×3", "lamp oil", 3),
        ("lamp oilx3", "lamp oilx3", 1),
        ("potion x 2", "potion", 2),
        ("potion ×2", "potion", 2),
        ("potionx2", "potionx2", 1),
        ("Box 2", "Box 2", 1),
        ("Hex 5", "Hex 5", 1),
        ("box x2", "box", 2),
        ("box x 4", "box", 4),
        ("3 arrows x2", "arrows x2", 3),
        ("999999 pebbles", "pebbles", 999_999),
        ("1234567 torches", "1234567 torches", 1),
        ("torch x1234567", "torch x1234567", 1),
        ("3", "3", 1),
        ("x2", "x2", 1),
        ("2x4 plank", "2x4 plank", 1),
        ("3\ttorches", "torches", 3),
        ("3\u{a0}torches", "torches", 3),
        ("\u{663} torches", "\u{663} torches", 1),
        ("Rope X2", "Rope X2", 1),
    ];
    for (typed, name, quantity) in cases {
        let html = page(&setup).await;
        let response = add(&setup, &html, typed).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{typed:?}");
        let data = saved(&setup, 1).await.1;
        let last = equipment(&data).last().unwrap().clone();
        assert_eq!(
            (last["name"].clone(), last["quantity"].clone()),
            (json!(name), json!(quantity)),
            "{typed:?}"
        );
    }
    for blank in ["", "   ", "\t"] {
        let before = saved(&setup, 1).await;
        let html = page(&setup).await;
        let response = add(&setup, &html, blank).await;
        assert_eq!(response.status(), StatusCode::OK, "{blank:?}");
        let html = body(response).await;
        assert!(
            super::assist_tests::text(&html, "[role=\"status\"]")
                .unwrap()
                .contains("Nothing to add"),
            "{html}"
        );
        assert_eq!(saved(&setup, 1).await, before, "nothing written");
    }
}

/// The line takes at most 200 characters (counted as characters, not bytes); a longer line
/// or one with a control character is refused (422) with the entry kept, escaped, in the
/// line and focused; nothing is written.
#[tokio::test]
async fn the_line_takes_200_characters_and_keeps_a_refused_entry() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&quick_assist(None)).await;
    three_rows(&setup).await;
    let longest = format!("3 {}", "a".repeat(198));
    assert_eq!(longest.chars().count(), 200);
    let response = add(&setup, &page(&setup).await, &longest).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let data = saved(&setup, 1).await.1;
    assert_eq!(equipment(&data)[3]["name"], json!("a".repeat(198)));
    let wide = "é".repeat(200);
    let response = add(&setup, &page(&setup).await, &wide).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER, "200 characters");
    let before = saved(&setup, 1).await;
    for (typed, reason) in [
        (
            format!("\"<b>{}", "a".repeat(197)),
            "at most 200 characters",
        ),
        ("3 torches\u{7}".to_owned(), "control character"),
    ] {
        let response = add(&setup, &page(&setup).await, &typed).await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{typed:?}"
        );
        let html = body(response).await;
        assert!(html.contains(reason), "{reason}: {html}");
        assert!(html.contains("Nothing was added to Equipment"), "{html}");
        assert_eq!(value_of(&html, "quick-4").as_deref(), Some(typed.as_str()));
        assert_eq!(autofocused(&html), ["quick-4"]);
        assert!(!html.contains("<b>"), "escaped");
        assert_eq!(saved(&setup, 1).await, before, "nothing written");
    }
}

/// The mapped name and count are filled even when they are optional properties; other
/// required numbers are unknown and every other optional property stays unset.
#[tokio::test]
async fn an_optional_mapped_name_and_quantity_are_filled() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let mut definition = gear();
    let property = |id: &str, label: &str, control: &str, optional: bool| json!({"id": id, "label": label, "control": control, "optional": optional});
    let mut kind = property("kind", "Kind", "enum", true);
    kind["options"] = json!([{"id": "tool", "label": "Tool"}]);
    definition["fields"][1]["items"] = json!({
        "properties": [
            property("name", "Name", "text", true),
            property("count", "Count", "integer", true),
            property("weight", "Weight", "integer", false),
            property("cost", "Cost", "integer", false),
            kind,
            property("worn", "Worn", "boolean", true),
            property("maker", "Maker", "text", true),
            property("story", "Story", "multiline", true),
            property("charges", "Charges", "integer", true),
        ],
        "quick_add": {"name": "name", "quantity": "count"},
    });
    let setup = setup(&definition).await;
    gear_sheet(&setup).await;
    let html = page(&setup).await;
    let (action, pairs) = quick(&html, 1, "2 lanterns");
    assert_eq!(action, "/private-sheets/1/lists/1/quick");
    let response = super::router_request(&setup.app, &setup.auth[1], &action, Some(pairs)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        saved(&setup, 1).await.1["gear"],
        json!([{"id": "r2-1", "name": "lanterns", "count": 2, "weight": null, "cost": null}])
    );
    assert_eq!(notes_at(&setup, 1, 2).await, vec![], "no note");
}

/// Decision 1.6-2: without a mapped quantity the whole line is the name, so a typed count is
/// never lost; the list's own required Quantity is unknown.
#[tokio::test]
async fn without_a_mapped_quantity_the_count_stays_in_the_name() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&quick_assist(Some(json!({"name": "name"})))).await;
    three_rows(&setup).await;
    let response = add(&setup, &page(&setup).await, "3 torches").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        equipment(&saved(&setup, 1).await.1)[3],
        json!({"id": "r2-1", "name": "3 torches", "quantity": null, "weight": null})
    );
    // Review round 1 (Sol Minor 3): the hint promises no count it would not read.
    let hint = super::assist_tests::text(&page(&setup).await, "#quick-hint-4").unwrap();
    assert!(
        !hint.contains("3 torches") && !hint.contains("x2"),
        "{hint}"
    );
    assert!(hint.contains("name"), "{hint}");
}

/// Review round 1 (Sol Minor 1): an entry with a line break (refused: a control character)
/// is kept in a textarea, on the refusal page and on the busy page, so a browser keeps the
/// break and a retry posts exactly what was typed (an `<input>` would strip it).
#[tokio::test]
async fn an_entry_with_a_line_break_is_kept_in_a_textarea() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&quick_assist(None)).await;
    three_rows(&setup).await;
    let typed = "3 torches\nand rope";
    let response = add(&setup, &page(&setup).await, typed).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = body(response).await;
    let control = |html: &str, id: &str| {
        let page = scraper::Html::parse_document(html);
        page.select(&scraper::Selector::parse(&format!("[id=\"{id}\"]")).unwrap())
            .next()
            .map(|element| {
                (
                    element.value().name().to_owned(),
                    element.text().collect::<String>(),
                    element.value().attr("name").map(str::to_owned),
                )
            })
            .unwrap()
    };
    assert_eq!(
        control(&html, "quick-4"),
        ("textarea".into(), typed.into(), Some("text".into()))
    );
    assert_eq!(autofocused(&html), ["quick-4"]);
    let (_, retry) = unit_form(&html, "quick-4", None);
    assert_eq!(retry[2], pair("text", typed), "a retry posts the break");
    let (action, pairs) = quick(&page(&setup).await, 4, typed);
    let holder = hold(3);
    let response = document_post(&setup, &action, pairs).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let busy = body(response).await;
    assert_eq!(
        control(&busy, "busy-value"),
        ("textarea".into(), typed.into(), Some("text".into()))
    );
    holder.await.unwrap();
}

/// Review round 1 (Sol coverage): another writer fills the 256th row between planning and
/// the writer's IMMEDIATE transaction; the line is refused with the entry kept, and the list
/// never holds 257 rows.
#[tokio::test]
async fn a_list_filled_before_the_write_keeps_the_entry() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&quick_assist(None)).await;
    let (status, html) = setup
        .post(&create_path(&setup), form(0, &[], &many(255)))
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
    let (action, pairs) = quick(&page(&setup).await, 4, "3 torches");
    let (planned, planned_rx) = tokio::sync::oneshot::channel();
    let (resume, resume_rx) = tokio::sync::oneshot::channel();
    *super::super::web::AFTER_UNIT_PLAN.lock().unwrap() = Some((planned, resume_rx));
    let app = setup.app.clone();
    let cookie = setup.auth[1].clone();
    let post =
        tokio::spawn(
            async move { super::router_request(&app, &cookie, &action, Some(pairs)).await },
        );
    tokio::time::timeout(std::time::Duration::from_secs(20), planned_rx)
        .await
        .expect("planned within 20 s")
        .unwrap();
    // Another writer adds the 256th row while the line is planned (outside the permit).
    let dummy = std::sync::Arc::new(tokio::sync::Semaphore::new(1))
        .acquire_owned()
        .await
        .unwrap();
    let (mut view, permit) = setup
        .store
        .private_read(setup.auth[1].clone(), 1, None, dummy)
        .await
        .unwrap();
    view.snapshot.data["equipment"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id": "r2-1", "name": "other", "quantity": 1, "weight": 1}));
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
    let status = response.status();
    assert!(
        matches!(
            status,
            StatusCode::CONFLICT | StatusCode::UNPROCESSABLE_ENTITY
        ),
        "{status}"
    );
    let html = body(response).await;
    let kept = kept(&html, 4).expect("the kept entry");
    assert!(
        kept.contains("3 torches") && kept.contains("full"),
        "{kept}"
    );
    let data = saved(&setup, 1).await.1;
    assert_eq!(equipment(&data).len(), 256, "never 257 rows");
    assert_eq!(equipment(&data)[255]["name"], json!("other"));
}

/// A list without the member shows no quick-add line and its route is 400; the route names
/// a list of the pin by canonical index and takes exactly `csrf`, `base` and `text`.
#[tokio::test]
async fn the_route_refuses_what_it_does_not_name() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let mut plain = assist();
    plain["fields"][4]["items"]
        .as_object_mut()
        .unwrap()
        .remove("quick_add");
    let setup_plain = setup(&plain).await;
    three_rows(&setup_plain).await;
    let html = page(&setup_plain).await;
    assert!(!html.contains("/quick\""), "no quick-add line");
    assert!(!html.contains("id=\"quick-4\""));
    let exact = vec![
        pair("csrf", &"a".repeat(64)),
        pair("base", "1"),
        pair("text", "3 torches"),
    ];
    assert_eq!(
        setup_plain
            .post("/private-sheets/1/lists/4/quick", exact.clone())
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    drop(setup_plain);
    let setup = setup(&quick_assist(None)).await;
    three_rows(&setup).await;
    for path in [
        "/private-sheets/1/lists/9/quick",
        "/private-sheets/1/lists/0/quick",
        "/private-sheets/1/lists/5/quick",
        "/private-sheets/1/lists/04/quick",
        "/private-sheets/1/lists/+4/quick",
    ] {
        assert_eq!(
            setup.post(path, exact.clone()).await.0,
            StatusCode::BAD_REQUEST,
            "{path}"
        );
    }
    let path = "/private-sheets/1/lists/4/quick";
    for keys in [
        vec![pair("csrf", &"a".repeat(64)), pair("base", "1")],
        vec![
            pair("csrf", &"a".repeat(64)),
            pair("base", "1"),
            pair("value", "3 torches"),
        ],
        vec![
            pair("csrf", &"a".repeat(64)),
            pair("base", "1"),
            pair("text", "3 torches"),
            pair("set", "present"),
        ],
    ] {
        assert_eq!(setup.post(path, keys).await.0, StatusCode::BAD_REQUEST);
    }
    for base in ["0", "-1", "x", "01"] {
        let mut form = exact.clone();
        form[1].1 = base.into();
        assert_eq!(
            setup.post(path, form).await.0,
            StatusCode::BAD_REQUEST,
            "base {base}"
        );
    }
    assert_eq!(saved(&setup, 1).await.0, 1, "nothing written");
}

/// Rights before keys: an outsider's quick-add is 403 whatever its keys or list; a bad
/// CSRF token is 403; a frozen sheet's is 403 and its page has no quick-add line.
#[tokio::test]
async fn the_route_rechecks_rights_before_keys() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&quick_assist(None)).await;
    let player = super::notes_tests::player_sheet(&setup).await;
    let outsider = &setup.auth[0];
    let exact = vec![
        pair("csrf", &"a".repeat(64)),
        pair("base", "1"),
        pair("text", "3 torches"),
    ];
    for (path, form) in [
        ("/private-sheets/1/lists/4/quick", exact.clone()),
        ("/private-sheets/1/lists/9/quick", exact.clone()),
        (
            "/private-sheets/1/lists/4/quick",
            vec![pair("csrf", &"a".repeat(64)), pair("value", "x")],
        ),
    ] {
        let response = super::router_request(&setup.app, outsider, path, Some(form)).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
    }
    let mut forged = exact.clone();
    forged[0].1 = "b".repeat(64);
    let response = super::router_request(
        &setup.app,
        &player,
        "/private-sheets/1/lists/4/quick",
        Some(forged),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN, "bad CSRF");
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
        "/private-sheets/1/lists/4/quick",
        Some(exact),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let frozen =
        body(super::router_request(&setup.app, &player, "/private-sheets/1", None).await).await;
    assert!(!frozen.contains("/quick\""), "{frozen}");
    assert!(!frozen.contains("id=\"quick-4\""));
}

/// Quick-add protects nothing: a stale page still adds (append only). A `base` above the
/// live revision names no page that was read: 409 with the entry kept in the line.
#[tokio::test]
async fn a_stale_page_still_adds_and_a_base_above_l_conflicts() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&quick_assist(None)).await;
    three_rows(&setup).await;
    let old = page(&setup).await;
    assert_eq!(
        add(&setup, &page(&setup).await, "rope").await.status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        add(&setup, &old, "2 lamps").await.status(),
        StatusCode::SEE_OTHER
    );
    let data = saved(&setup, 1).await.1;
    assert_eq!(equipment(&data)[4]["name"], json!("lamps"));
    let (action, mut pairs) = quick(&old, 4, "5 nails");
    pairs[1].1 = "999".into();
    let before = saved(&setup, 1).await;
    let (status, html) = setup.post(&action, pairs).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(html.contains("Nothing was added to Equipment"), "{html}");
    assert_eq!(value_of(&html, "quick-4").as_deref(), Some("5 nails"));
    assert_eq!(autofocused(&html), ["quick-4"]);
    assert_eq!(saved(&setup, 1).await, before);
}

/// D-X3: while busy the line waits up to 2 s, then answers the styled busy page keeping the
/// entry in a control named `text` and its `base`; posting it again adds the row.
#[tokio::test]
async fn the_busy_page_keeps_the_line() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&quick_assist(None)).await;
    three_rows(&setup).await;
    let (action, pairs) = quick(&page(&setup).await, 4, "3 \"torches\"");
    let holder = hold(3);
    let response = document_post(&setup, &action, pairs).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["retry-after"], "1");
    let busy = body(response).await;
    assert!(
        busy.contains("Your entry for the quick-add line of list field 4"),
        "{busy}"
    );
    let (again, kept) = unit_form(&busy, "busy-value", None);
    assert_eq!(again, "/private-sheets/1/lists/4/quick");
    assert_eq!(
        kept,
        vec![
            pair("csrf", &"a".repeat(64)),
            pair("base", "1"),
            pair("text", "3 \"torches\""),
        ]
    );
    holder.await.unwrap();
    assert_eq!(
        document_post(&setup, &again, kept).await.status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        equipment(&saved(&setup, 1).await.1)[3]["name"],
        json!("\"torches\"")
    );
}

/// A full list (256 rows) shows no quick-add line; an entry from a page loaded at 255 rows
/// is refused (422) and kept read-only to copy, with no form that could add it.
#[tokio::test]
async fn a_full_list_keeps_a_late_entry_read_only() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&quick_assist(None)).await;
    let (status, html) = setup
        .post(&create_path(&setup), form(0, &[], &many(255)))
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
    let old = page(&setup).await;
    assert!(old.contains("id=\"quick-4\""));
    assert_eq!(
        add(&setup, &page(&setup).await, "other").await.status(),
        StatusCode::SEE_OTHER
    );
    let full = page(&setup).await;
    assert!(!full.contains("/quick\""), "no quick-add line at 256 rows");
    let before = saved(&setup, 1).await;
    let (action, pairs) = quick(&old, 4, "3 \"late\" <b>");
    let response = document_post(&setup, &action, pairs).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = body(response).await;
    assert_eq!(saved(&setup, 1).await, before, "nothing written");
    assert!(!html.contains("/quick\""), "no form adds it");
    let kept = kept(&html, 4).expect("the kept entry");
    assert!(
        kept.contains("3 \"late\" <b>") && kept.contains("full"),
        "{kept}"
    );
    assert!(!html.contains("<b>"), "escaped");
    let page = scraper::Html::parse_document(&html);
    let textareas: Vec<_> = page
        .select(&scraper::Selector::parse("#kept-4 textarea").unwrap())
        .collect();
    assert_eq!(textareas.len(), 1);
    assert!(textareas[0].value().attr("readonly").is_some());
}

/// Slice 1.5b leftover (Sol round 2): a repeated `?focus=` is ignored, not 400; one valid
/// focus still focuses its control, and any other parameter is ignored (a list without the
/// quick-add line, so the leftover is shown on its own).
#[tokio::test]
async fn repeated_focus_parameters_are_ignored() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let mut plain = assist();
    plain["fields"][4]["items"]
        .as_object_mut()
        .unwrap()
        .remove("quick_add");
    let setup = setup(&plain).await;
    three_rows(&setup).await;
    for query in [
        "?focus=up-4-1&focus=up-4-1",
        "?focus=quick-4&focus=down-4-0",
        "?focus=&focus=",
    ] {
        let (status, html) = setup.get(&format!("/private-sheets/1{query}")).await;
        assert_eq!(status, StatusCode::OK, "{query}");
        assert!(autofocused(&html).is_empty(), "{query}");
    }
    for (query, focused) in [
        ("?focus=up-4-1", "up-4-1"),
        ("?focus=down-4-0&other=1&other=2", "down-4-0"),
    ] {
        let (status, html) = setup.get(&format!("/private-sheets/1{query}")).await;
        assert_eq!(status, StatusCode::OK, "{query}");
        assert_eq!(autofocused(&html), [focused], "{query}");
    }
}
