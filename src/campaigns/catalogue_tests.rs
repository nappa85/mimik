//! Catalogue plan §3 step 3: the `catalogues` link checked at its three sites (D8, R10, R27),
//! and the read-only catalogue routes reached through a sheet (D4, D11, R6, R7, R12, R21,
//! R25), through the real router. Synthetic original text only.
use super::{fixture, insert_native, native, router_request};
use crate::{
    AppError, Store,
    auth::RequestAuth,
    catalogues::PublishRequest,
    sources::{PageReview, UnitReview},
};
use axum::http::StatusCode;
use rusqlite::params;
use serde_json::{Value, json};
use std::sync::Arc;

const RLO: char = '\u{202E}';
const ZEROS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Unit (id, page, raw text) of the synthetic source. Distinctive letters let a test count
/// exactly what a page shows.
fn units() -> Vec<(i64, i64, String)> {
    vec![
        (1, 1, "Lantern flare: a synthetic beam of light.".into()),
        (2, 1, "Second passage of the flare.".into()),
        (3, 2, "ж".repeat(25_000)),
        (4, 2, "ф".repeat(15_000)),
        (5, 3, "ш".repeat(8_000)),
        (6, 3, format!("{}{}", "é".repeat(19_999), "🦀🦀🦀")),
        (
            8,
            4,
            format!("<script>zqx()</script> Hostile passage {RLO}tail"),
        ),
        (9, 4, "Passage rejected later.".into()),
        (10, 4, "Sheet field evidence.".into()),
    ]
}

async fn catalogue_source(store: &Store) {
    store.run(|db| {
        for game in ["synthetic-game", "other-game"] {
            db.execute("INSERT INTO registry_games VALUES(?1,'Synthetic')", [game])?;
            db.execute("INSERT INTO registry_editions VALUES(?1,'first','First',0)", [game])?;
        }
        let prepared = json!({"contract_version":1,"extraction_sha256":ZEROS}).to_string();
        db.execute("INSERT INTO sources(bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages) VALUES(?1,'fixture','Synthetic <b>manual</b>','synthetic-game','First','en',?1,x'',x'',?2,4)", params![ZEROS, prepared.as_bytes()])?;
        db.execute("INSERT INTO source_rulesets VALUES(1,'synthetic-game','first')", [])?;
        for page in 1..=4 {
            db.execute("INSERT INTO source_pages(id,source_id,pdf_page,png) VALUES(?1,1,?1,x'')", [page])?;
        }
        for (id, page, text) in units() {
            db.execute("INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(?1,?2,?3,'text','[[0,0],[1,0],[1,1],[0,1]]','<p>Synthetic unit.</p>',?4)", params![id, page, format!("b{id}"), text])?;
        }
        // A unit whose text holds NUL (R21: shown as malformed, never as text).
        db.execute("INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(7,4,'b7','text','[[0,0],[1,0],[1,1],[0,1]]','<p>Synthetic unit.</p>',CAST(x'5a00515a' AS TEXT))", [])?;
        // A unit whose text is a BLOB, and one over the 256 KiB text bound (R21: malformed).
        db.execute("INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(11,4,'b11','text','[[0,0],[1,0],[1,1],[0,1]]','<p>Synthetic unit.</p>',CAST(x'5a515a' AS BLOB))", [])?;
        db.execute("INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(12,4,'b12','text','[[0,0],[1,0],[1,1],[0,1]]','<p>Synthetic unit.</p>',?1)", ["q".repeat(256 * 1024 + 1)])?;
        crate::sources::search::ensure(db)?;
        for page in 1..=4 {
            db.execute("INSERT INTO source_page_hashes VALUES(?1,1,?2)", params![page, ZEROS])?;
        }
        db.execute("INSERT INTO source_finalizations VALUES(1,?1,?1)", [ZEROS])?;
        Ok(())
    }).await.unwrap();
}

fn unit_entry(key: &str, name: &str, units: &[i64]) -> Value {
    let page = |unit: i64| units_page(unit);
    json!({"key": key, "type": "spell", "name": name,
           "properties": {"level": 1, "school": "light", "note": "Bright"},
           "description": {"units": units.iter().map(|unit| json!({"source_id": 1, "page_id": page(*unit), "unit_id": unit})).collect::<Vec<_>>()}})
}

fn units_page(unit: i64) -> i64 {
    if matches!(unit, 7 | 11 | 12) {
        return 4;
    }
    units()
        .into_iter()
        .find(|(id, _, _)| *id == unit)
        .unwrap()
        .1
}

fn owner_entry(key: &str, entry_type: &str, name: &str, text: &str) -> Value {
    json!({"key": key, "type": entry_type, "name": name, "properties": {},
           "description": {"owner_text": text}})
}

/// Owner text of exactly 20,000 scalars with markup, a line break and an unterminated
/// right-to-left override; the filler letter `ŋ` appears nowhere else.
fn house_head() -> String {
    format!("<script>zqx()</script>\nWard ink {RLO}reversed tail ")
}

fn house_text() -> String {
    let head = house_head();
    format!("{head}{}", "ŋ".repeat(20_000 - head.chars().count()))
}

fn spells(extra: Vec<Value>) -> Value {
    let mut entries = vec![
        unit_entry("lantern-flare", "Lantern Flare", &[1, 2]),
        unit_entry("giant", "Giant Text", &[3]),
        unit_entry("cumulative", "Cumulative Text", &[4, 5]),
        unit_entry("multibyte", "Multibyte Text", &[6]),
        unit_entry("malformed", "Malformed Text", &[7]),
        unit_entry("hostile", "Hostile <b>Name</b>", &[8]),
        unit_entry("cut-rejected", "Cut Then Rejected", &[3, 9]),
        unit_entry("cut-nul", "Cut Then NUL", &[3, 7]),
        unit_entry("cut-blob", "Cut Then BLOB", &[3, 11]),
        unit_entry("cut-oversized", "Cut Then Oversized", &[3, 12]),
        owner_entry("house-charm", "charm", "House Charm", &house_text()),
    ];
    let mut retired = owner_entry("retired-charm", "charm", "Retired Charm", "Old charm.");
    retired["retired"] = true.into();
    entries.push(retired);
    entries.extend(extra);
    json!({"contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
           "catalogue_id": "spells", "title": "Spells <i>of</i> light",
           "sources": [{"source_id": 1, "pdf_sha256": ZEROS}],
           "entry_types": [
               {"id": "spell", "label": "Spell", "properties": [
                   {"id": "level", "label": "Level", "control": "integer"},
                   {"id": "school", "label": "School", "control": "enum",
                    "options": [{"id": "light", "label": "Light"}, {"id": "shadow", "label": "Shadow"}]},
                   {"id": "note", "label": "Note", "control": "text"}]},
               {"id": "charm", "label": "Charm", "properties": []}],
           "entries": entries})
}

fn owner_catalogue(game: &str, id: &str, entries: &[Value]) -> Value {
    json!({"contract_version": 1, "game_id": game, "edition_id": "first",
           "catalogue_id": id, "title": format!("Synthetic {id}"), "sources": [],
           "entry_types": [{"id": "item", "label": "Item", "properties": []},
                           {"id": "charm", "label": "Charm", "properties": []}],
           "entries": entries})
}

async fn publish_catalogue(store: &Store, value: &Value, expected_revision: i64) {
    let draft = store
        .save_catalogue_draft(serde_json::to_vec(value).unwrap(), expected_revision)
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

fn definition(id: &str, catalogues: &[&str]) -> Value {
    json!({"contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
           "definition_id": id, "title": "Catalogue sheet",
           "sources": [{"source_id": 1, "pdf_sha256": ZEROS, "roles": ["sheet"]}],
           "fields": [{"id": "label", "label": "Label", "control": "text", "nullable": false,
                       "mapping": {"state": "mapped", "regions": [{"source_id": 1, "page_id": 4, "unit_id": 10}]}}],
           "groups": [{"id": "main", "label": "Main", "fields": ["label"], "column_hint": "main"}],
           "catalogues": catalogues.iter().map(|id| json!({"id": id})).collect::<Vec<_>>()})
}

async fn publish_definition(store: &Store, value: &Value) -> i64 {
    let draft = store
        .save_definition_draft(serde_json::to_vec(value).unwrap(), 0)
        .await
        .unwrap();
    let comparison = store.compare_definition(draft.id, false).await.unwrap();
    store
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
        .id
}

struct World {
    _dir: tempfile::TempDir,
    store: Store,
    auth: Vec<RequestAuth>,
    app: axum::Router,
    campaign: i64,
    version: i64,
}

/// Accounts: 1 Owner, 2 Master (campaign creator), 3 custodian of sheet 1, 4 another player,
/// 5 an outsider. Catalogues: `spells` and `items` (linked, with a colliding key),
/// `bestiary` (same pair, unlinked), and `spells` and `tomes` of another game.
async fn world() -> World {
    let (dir, store, auth, _clock) = fixture().await;
    catalogue_source(&store).await;
    publish_catalogue(&store, &spells(Vec::new()), 0).await;
    publish_catalogue(
        &store,
        &owner_catalogue(
            "synthetic-game",
            "items",
            &[owner_entry(
                "lantern-flare",
                "item",
                "Brass Lantern",
                "Oil lamp.",
            )],
        ),
        0,
    )
    .await;
    publish_catalogue(
        &store,
        &owner_catalogue(
            "synthetic-game",
            "bestiary",
            &[owner_entry("beast", "item", "Beast", "Unlinked.")],
        ),
        0,
    )
    .await;
    for id in ["spells", "tomes"] {
        publish_catalogue(
            &store,
            &owner_catalogue(
                "other-game",
                id,
                &[owner_entry(
                    "other-only",
                    "item",
                    "Other Game Entry",
                    "Elsewhere.",
                )],
            ),
            0,
        )
        .await;
    }
    let version = publish_definition(&store, &definition("sheet", &["spells", "items"])).await;
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Catalogue table".into(),
            "synthetic-game".into(),
            "first".into(),
        )
        .await
        .unwrap();
    for account in [3, 4] {
        store
            .request_join(
                auth[usize::try_from(account - 1).unwrap()].clone(),
                campaign,
            )
            .await
            .unwrap();
        store
            .decide_join(auth[1].clone(), campaign, account, 1, true)
            .await
            .unwrap();
    }
    let app = crate::auth::app(store.clone()).await.unwrap();
    let world = World {
        _dir: dir,
        store,
        auth,
        app,
        campaign,
        version,
    };
    create_sheet(&world, &world.auth[2], version).await;
    world
}

async fn create_sheet(world: &World, auth: &RequestAuth, version: i64) {
    let path = format!("/campaigns/{}/sheets/new?version={version}", world.campaign);
    let form = vec![
        ("csrf".into(), "a".repeat(64)),
        ("revision".into(), "0".into()),
        ("name".into(), "Reader sheet".into()),
        ("notes".into(), String::new()),
        ("value-label".into(), "Text".into()),
    ];
    assert_eq!(
        router_request(&world.app, auth, &path, Some(form))
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
}

async fn get(app: &axum::Router, auth: &RequestAuth, path: &str) -> (StatusCode, String) {
    let response = router_request(app, auth, path, None).await;
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

/// A browser navigation: the auth layer's document presentation applies to error pages.
async fn navigate(app: &axum::Router, auth: &RequestAuth, path: &str) -> (StatusCode, String) {
    use tower::ServiceExt;
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri(path)
                .header("host", "127.0.0.1:3000")
                .header("accept", "text/html")
                .header("sec-fetch-mode", "navigate")
                .header("sec-fetch-dest", "document")
                .header("cookie", auth.fixture_cookie())
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

/// The uniform catalogue refusal's explanation (plan §19).
const REFUSED: &str = "This catalogue can’t be opened from this sheet.";

/// The uniform catalogue refusal for `sheet`, as the outsider (account 5) receives it: 403
/// with the explanation and links back, the same for every cause (plan §19).
async fn refusal(world: &World, sheet: i64) -> (StatusCode, String) {
    let refused = get(
        &world.app,
        &world.auth[4],
        &format!("/private-sheets/{sheet}/catalogue/spells"),
    )
    .await;
    assert_eq!(refused.0, StatusCode::FORBIDDEN, "{}", refused.1);
    assert!(refused.1.contains(REFUSED), "{}", refused.1);
    assert!(
        refused
            .1
            .contains(&format!("href=\"/private-sheets/{sheet}\""))
    );
    refused
}

async fn revision(store: &Store, sql: &'static str, id: i64) -> i64 {
    store
        .run(move |db| Ok(db.query_row(sql, [id], |row| row.get(0))?))
        .await
        .unwrap()
}

async fn review(store: &Store, unit: i64, state: &str, correction: &str) {
    let current = revision(store, "SELECT revision FROM source_units WHERE id=?1", unit).await;
    store
        .review_source_unit(
            units_page(unit),
            unit,
            UnitReview {
                revision: current,
                state: state.into(),
                correction: correction.into(),
                actor: "Synthetic reviewer".into(),
                rationale: "Synthetic review".into(),
            },
        )
        .await
        .unwrap();
}

async fn complete(store: &Store, page: i64, label: &str) {
    let current = revision(store, "SELECT revision FROM source_pages WHERE id=?1", page).await;
    store
        .complete_source_page(
            page,
            PageReview {
                revision: current,
                printed_label: label.into(),
                actor: "Synthetic reviewer".into(),
                rationale: "Synthetic completion".into(),
            },
        )
        .await
        .unwrap();
}

async fn remove_seals(store: &Store, catalogue: &'static str) {
    store
        .run(move |db| {
            let original: String = db.query_row(
                "SELECT sql FROM sqlite_schema WHERE name='catalogue_seals_delete'",
                [],
                |row| row.get(0),
            )?;
            db.execute_batch("DROP TRIGGER catalogue_seals_delete")?;
            db.execute("DELETE FROM catalogue_seals WHERE version_id IN (SELECT id FROM catalogue_versions WHERE game_id='synthetic-game' AND catalogue_id=?1)", [catalogue])?;
            db.execute_batch(&original)?;
            Ok(())
        })
        .await
        .unwrap();
}

const PATHS: [&str; 4] = [
    "/private-sheets/1/catalogue/spells",
    "/private-sheets/1/catalogue/spells/lantern-flare",
    "/private-sheets/1/catalogue/items",
    "/private-sheets/1/catalogue/items/lantern-flare",
];

#[tokio::test]
async fn sheet_links_its_catalogues_and_routes_follow_the_sheet_read_matrix() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let world = world().await;
    let (status, sheet) = get(&world.app, &world.auth[2], "/private-sheets/1").await;
    assert_eq!(status, StatusCode::OK);
    for id in ["spells", "items"] {
        assert!(
            sheet.contains(&format!(
                "href=\"/private-sheets/1/catalogue/{id}\" target=\"_blank\" rel=\"noopener\"><bdi>"
            )),
            "{id}"
        );
    }
    assert!(sheet.contains("<bdi>Spells &#60;i&#62;of&#60;/i&#62; light</bdi>"));
    assert!(!sheet.contains("<i>of</i>"));
    assert!(!sheet.contains("bestiary"));
    // The create page has no sheet to read catalogues through: no section.
    let (status, create) = get(
        &world.app,
        &world.auth[2],
        &format!(
            "/campaigns/{}/sheets/new?version={}",
            world.campaign, world.version
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!create.contains("catalogues-heading") && !create.contains("/catalogue/"));
    assert!(
        !sheet
            .replace(super::script_tests::MODULE_TAG, "")
            .contains("<script"),
        "sheet pages load only the sheet module"
    );
    let reader = |auth: usize| (world.app.clone(), world.auth[auth].clone());
    // Custodian (3) and Master (2) read; another player (4), an outsider (5) and the
    // production Owner (1) receive the uniform catalogue refusal.
    for (account, expected) in [
        (2, StatusCode::OK),
        (1, StatusCode::OK),
        (3, StatusCode::FORBIDDEN),
        (4, StatusCode::FORBIDDEN),
        (0, StatusCode::FORBIDDEN),
    ] {
        let (app, auth) = reader(account);
        for path in PATHS {
            let (status, html) = get(&app, &auth, path).await;
            assert_eq!(status, expected, "account index {account} {path}");
            if status == StatusCode::OK {
                assert!(!html.contains("<script"), "{path} loads a script");
                crate::auth::nav::check::assert_header(
                    &html,
                    false,
                    Some("/private-sheets"),
                    true,
                    path,
                );
            }
        }
    }
    // The custodian departs: the sheet freezes and both readers keep reading (R15, Q5).
    let membership = revision(
        &world.store,
        "SELECT revision FROM campaign_members WHERE account_id=?1",
        3,
    )
    .await;
    world
        .store
        .depart(world.auth[2].clone(), world.campaign, 3, membership)
        .await
        .unwrap();
    let (_, frozen) = get(&world.app, &world.auth[2], "/private-sheets/1").await;
    assert!(frozen.contains("Frozen on departure"));
    for account in [2, 1] {
        for path in PATHS {
            assert_eq!(
                get(&world.app, &world.auth[account], path).await.0,
                StatusCode::OK,
                "frozen sheet, account index {account} {path}"
            );
        }
    }
    // A disabled custodian's session ends: catalogue routes answer exactly like the sheet
    // (the session layer's 401, before any catalogue check).
    world
        .store
        .run(|db| {
            db.execute(
                "UPDATE accounts SET epoch=2,state='disabled' WHERE id=3",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let refused = get(&world.app, &world.auth[2], "/private-sheets/1").await;
    assert_eq!(refused.0, StatusCode::UNAUTHORIZED);
    for path in PATHS {
        assert_eq!(
            get(&world.app, &world.auth[2], path).await,
            refused,
            "{path}"
        );
    }
}

#[tokio::test]
async fn refusals_are_uniform_and_explain_themselves() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let world = world().await;
    // A native sheet (id 2) of the Master in a native campaign.
    let native_campaign = world
        .store
        .create_campaign(
            world.auth[1].clone(),
            "Native".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    let mut snapshot = native(native_campaign, 2);
    snapshot.sheet_id = 2;
    insert_native(&world.store, world.auth[1].clone(), snapshot).await;
    assert_eq!(
        get(&world.app, &world.auth[1], "/private-sheets/2").await.0,
        StatusCode::OK
    );
    let reference = refusal(&world, 1).await;
    // The refusal names no catalogue, title or cause.
    for hidden in ["spells", "Spells", "Access denied", "could not be accepted"] {
        assert!(!reference.1.contains(hidden), "{hidden}");
    }
    let mut causes = vec![
        // Unlinked catalogue of the same pair; another game's catalogue id; unknown id.
        (2, "/private-sheets/1/catalogue/bestiary"),
        (2, "/private-sheets/1/catalogue/bestiary/beast"),
        (2, "/private-sheets/1/catalogue/tomes"),
        (2, "/private-sheets/1/catalogue/nothing"),
        (2, "/private-sheets/1/catalogue/nothing/beast"),
        (1, "/private-sheets/1/catalogue/bestiary"),
        // Readers who cannot read the sheet, for linked catalogues and existing keys.
        (3, "/private-sheets/1/catalogue/spells"),
        (3, "/private-sheets/1/catalogue/spells/lantern-flare"),
        (3, "/private-sheets/1/catalogue/spells/nothing"),
        (3, "/private-sheets/1/catalogue/spells?type=potion"),
        (4, "/private-sheets/1/catalogue/nothing"),
        // The production Owner reads no sheet.
        (0, "/private-sheets/1/catalogue/items/lantern-flare"),
        (0, "/private-sheets/1/catalogue/spells/nothing"),
    ];
    // An unknown sheet and a native pin.
    let other_sheets = [
        (1, "/private-sheets/99/catalogue/spells"),
        (1, "/private-sheets/99/catalogue/spells/nothing"),
        (1, "/private-sheets/2/catalogue/spells"),
        (1, "/private-sheets/2/catalogue/spells/lantern-flare"),
        (1, "/private-sheets/2/catalogue/spells?type=potion"),
    ];
    for (account, path) in causes.iter().chain(other_sheets.iter()) {
        let sheet = if path.starts_with("/private-sheets/99/") {
            "99"
        } else if path.starts_with("/private-sheets/2/") {
            "2"
        } else {
            "1"
        };
        // Identical to every other cause, apart from the sheet link taken from the address.
        let expected = (
            reference.0,
            reference
                .1
                .replace("/private-sheets/1\"", &format!("/private-sheets/{sheet}\"")),
        );
        assert_eq!(
            get(&world.app, &world.auth[*account], path).await,
            expected,
            "{path}"
        );
        assert_eq!(
            navigate(&world.app, &world.auth[*account], path).await,
            expected,
            "document: {path}"
        );
    }
    // A linked catalogue without a sealed version.
    remove_seals(&world.store, "items").await;
    causes = vec![
        (2, "/private-sheets/1/catalogue/items"),
        (2, "/private-sheets/1/catalogue/items/lantern-flare"),
        (2, "/private-sheets/1/catalogue/items/nothing"),
    ];
    for (account, path) in causes {
        assert_eq!(
            get(&world.app, &world.auth[account], path).await,
            reference,
            "{path}"
        );
    }
}

#[tokio::test]
async fn missing_entries_and_types_of_a_linked_catalogue_are_not_found() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let world = world().await;
    let spells = "<bdi>Spells &#60;i&#62;of&#60;/i&#62; light</bdi>";
    let found = |path: &'static str| {
        let (app, auth) = (world.app.clone(), world.auth[2].clone());
        async move {
            let page = get(&app, &auth, path).await;
            assert_eq!(page.0, StatusCode::NOT_FOUND, "{path}: {}", page.1);
            assert_eq!(navigate(&app, &auth, path).await, page, "document: {path}");
            page.1
        }
    };
    // Unknown keys, a key of an unlinked catalogue and a key of another game's catalogue all
    // read the same: the entry is not in this catalogue.
    let nothing = found("/private-sheets/1/catalogue/spells/nothing").await;
    assert!(
        nothing.contains(&format!("No entry “nothing” in {spells}.")),
        "{nothing}"
    );
    assert!(nothing.contains("href=\"/private-sheets/1/catalogue/spells\""));
    assert!(nothing.contains("href=\"/private-sheets/1\""));
    for (path, key) in [
        ("/private-sheets/1/catalogue/spells/beast", "beast"),
        (
            "/private-sheets/1/catalogue/spells/other-only",
            "other-only",
        ),
    ] {
        assert_eq!(found(path).await.replace(key, "nothing"), nothing, "{path}");
    }
    // A key of another linked catalogue (`giant` is a spell) is not an item.
    let giant = found("/private-sheets/1/catalogue/items/giant").await;
    assert!(giant.contains("No entry “giant” in <bdi>Synthetic items</bdi>."));
    assert!(giant.contains("href=\"/private-sheets/1/catalogue/items\""));
    assert!(!giant.contains("Giant Text"));
    // An undeclared type filter, with or without a search and a cursor.
    let potion = found("/private-sheets/1/catalogue/spells?type=potion").await;
    assert!(
        potion.contains(&format!("No entry type “potion” in {spells}.")),
        "{potion}"
    );
    assert!(potion.contains("href=\"/private-sheets/1/catalogue/spells\""));
    assert_eq!(
        found("/private-sheets/1/catalogue/spells?type=potion&q=lantern&after=giant").await,
        potion
    );
    // An empty type means all types; an unknown cursor restarts from the beginning (R13).
    for path in [
        "/private-sheets/1/catalogue/spells?type=&q=&after=",
        "/private-sheets/1/catalogue/spells?after=nothing",
    ] {
        assert_eq!(
            get(&world.app, &world.auth[2], path).await.0,
            StatusCode::OK,
            "{path}"
        );
    }
}

#[tokio::test]
async fn malformed_addresses_answer_400_before_any_lookup_and_say_why() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let world = world().await;
    let long = format!("/private-sheets/1/catalogue/spells?q={}", "a".repeat(101));
    let mut first: Option<(StatusCode, String)> = None;
    for path in [
        "/private-sheets/x/catalogue/spells",
        "/private-sheets/x/catalogue/spells/lantern-flare",
        "/private-sheets/1/catalogue/Spells",
        "/private-sheets/1/catalogue/spells/Lantern_Flare",
        "/private-sheets/1/catalogue/spells?type=Spell",
        "/private-sheets/1/catalogue/spells?after=-x",
        "/private-sheets/1/catalogue/spells?unknown=1",
        "/private-sheets/1/catalogue/spells?q=a&q=b",
        long.as_str(),
    ] {
        // The reader and an outsider get the same page: syntax only, nothing is looked up.
        for account in [2, 4] {
            let page = get(&world.app, &world.auth[account], path).await;
            assert_eq!(page.0, StatusCode::BAD_REQUEST, "{path}");
            assert!(
                page.1.contains("This catalogue address is not valid."),
                "{path}: {}",
                page.1
            );
            assert_eq!(
                navigate(&world.app, &world.auth[account], path).await,
                page,
                "{path}"
            );
            match &first {
                Some(first) => assert_eq!(&page, first, "{path}"),
                None => first = Some(page),
            }
        }
    }
    let limit = format!("/private-sheets/1/catalogue/spells?q={}", "a".repeat(100));
    assert_eq!(
        get(&world.app, &world.auth[2], &limit).await.0,
        StatusCode::OK
    );
}

fn count(html: &str, letter: char) -> usize {
    html.chars().filter(|c| *c == letter).count()
}

#[tokio::test]
async fn entry_badges_descriptions_and_citations_are_live() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let world = world().await;
    let entry = "/private-sheets/1/catalogue/spells/lantern-flare";
    let (status, html) = get(&world.app, &world.auth[2], entry).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("<h1><bdi>Lantern Flare</bdi></h1>"));
    assert!(html.contains("Unchecked source text"));
    assert!(html.contains("Extracted from your manual; not yet reviewed."));
    assert!(html.contains("Lantern flare: a synthetic beam of light."));
    assert!(html.contains("Second passage of the flare."));
    assert!(html.contains("<dt><bdi>Level</bdi></dt><dd><bdi>1</bdi></dd><dt><bdi>School</bdi></dt><dd><bdi>Light</bdi></dd><dt><bdi>Note</bdi></dt><dd><bdi>Bright</bdi></dd>"));
    assert!(html.contains("PDF page 1</li>"));
    assert!(!html.contains("<b>manual</b>"), "source titles are escaped");
    assert!(
        !html.contains("/sources/"),
        "players get citations, never page images"
    );
    assert!(html.contains("Current text"));
    assert!(html.contains("<p class=\"catalogue-type\"><bdi>Spell</bdi></p>"));
    // A review and a correction: the corrected text shows at once, the page is incomplete.
    review(&world.store, 1, "accepted", "").await;
    review(&world.store, 2, "corrected", "Corrected flare passage.").await;
    let (_, html) = get(&world.app, &world.auth[2], entry).await;
    assert!(html.contains("Unchecked source text"));
    assert!(html.contains("Corrected flare passage."));
    assert!(!html.contains("Second passage of the flare."));
    // Page completion makes the entry reviewed and brings in the printed label.
    complete(&world.store, 1, "12").await;
    let (_, html) = get(&world.app, &world.auth[2], entry).await;
    assert!(html.contains("Reviewed extraction"));
    assert!(html.contains(
        "Source text reviewed. Which passages form this entry was chosen by the catalogue author."
    ));
    assert!(html.contains("p. <bdi>12</bdi> (PDF page 1)</li>"));
    // A label change is read live.
    complete(&world.store, 1, "xii").await;
    let (_, html) = get(&world.app, &world.auth[2], entry).await;
    assert!(html.contains("p. <bdi>xii</bdi> (PDF page 1)</li>"));
    assert!(!html.contains("<bdi>12</bdi>"));
    // A rejection hides every passage of the entry; the citation stays.
    review(&world.store, 2, "rejected", "").await;
    let (_, html) = get(&world.app, &world.auth[2], entry).await;
    assert!(html.contains("Description unavailable"));
    assert!(!html.contains("Lantern flare: a synthetic beam"));
    assert!(!html.contains("Corrected flare passage."));
    assert!(html.contains("p. <bdi>xii</bdi> (PDF page 1)</li>"));
    let (_, browse) = get(
        &world.app,
        &world.auth[2],
        "/private-sheets/1/catalogue/spells",
    )
    .await;
    assert!(browse.contains(
        "<a href=\"/private-sheets/1/catalogue/spells/lantern-flare\"><bdi>Lantern Flare</bdi></a> · <bdi>Spell</bdi> <span class=\"badge\">Description unavailable</span>"
    ));
    assert!(browse.contains(
        "<bdi>House Charm</bdi></a> · <bdi>Charm</bdi> <span class=\"badge\">Owner-written</span>"
    ));
    assert!(browse.contains("<bdi>Retired Charm</bdi></a> · <bdi>Charm</bdi> <span class=\"badge\">Owner-written</span> <span class=\"badge\">Retired</span>"));
}

#[tokio::test]
async fn descriptions_are_cut_at_20000_characters_and_owner_text_never() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let world = world().await;
    let page = |key: &str| {
        let app = world.app.clone();
        let auth = world.auth[2].clone();
        let path = format!("/private-sheets/1/catalogue/spells/{key}");
        async move {
            let (status, html) = get(&app, &auth, &path).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            html
        }
    };
    // An oversized first unit.
    let giant = page("giant").await;
    assert_eq!(count(&giant, 'ж'), 20_000);
    assert!(giant.contains("Continues in the manual at PDF page 2.</span></p>"));
    // A cumulative cut names the page of the first unit not fully shown.
    let cumulative = page("cumulative").await;
    assert_eq!(count(&cumulative, 'ф'), 15_000);
    assert_eq!(count(&cumulative, 'ш'), 5_000);
    assert!(cumulative.contains("Continues in the manual at PDF page 3."));
    // The cut never splits a multibyte character.
    let multibyte = page("multibyte").await;
    assert_eq!(count(&multibyte, 'é'), 19_999);
    assert_eq!(count(&multibyte, '🦀'), 1);
    assert!(multibyte.contains("Continues in the manual at PDF page 3."));
    // NUL in a unit: no text, a static note.
    let malformed = page("malformed").await;
    assert!(
        malformed.contains("Description unavailable (source text malformed or citations missing).")
    );
    assert!(!malformed.contains("ZQZ") && !malformed.contains("Z\u{0}"));
    // Owner text: whole, escaped, line breaks kept, isolated.
    let house = page("house-charm").await;
    assert_eq!(count(&house, 'ŋ'), 20_000 - house_head().chars().count());
    assert!(!house.contains("Continues in the manual"));
    assert!(house.contains("Owner-written"));
    assert!(!house.contains("<script>"));
    assert!(house.contains(&format!(
        "<p class=\"catalogue-text\"><bdi>&#60;script&#62;zqx()&#60;/script&#62;\nWard ink {RLO}reversed tail ŋ"
    )));
    assert!(house.contains("ŋ</bdi></p></div>"));
    // Hostile unit text and names are escaped and isolated.
    let hostile = page("hostile").await;
    assert!(!hostile.contains("<script>") && !hostile.contains("<b>Name</b>"));
    assert!(hostile.contains(&format!("Hostile passage {RLO}tail</bdi></p>")));
    // A rejected unit after the cut still hides every passage (R7).
    let before = page("cut-rejected").await;
    assert_eq!(count(&before, 'ж'), 20_000);
    review(&world.store, 9, "rejected", "").await;
    let after = page("cut-rejected").await;
    assert_eq!(count(&after, 'ж'), 0);
    assert!(after.contains("Description unavailable"));
    assert!(after.contains("PDF page 2</li>") && after.contains("PDF page 4</li>"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catalogue_reads_hold_the_permit_and_recheck_grants_after_validation() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let world = world().await;
    // Account 4 becomes a Master, so it reads sheet 1 only through that grant.
    world
        .store
        .change_role(world.auth[1].clone(), world.campaign, 4, 1, true)
        .await
        .unwrap();
    for (request, demote) in [
        (
            super::super::catalogue::Request::Entry("lantern-flare".into()),
            false,
        ),
        (
            super::super::catalogue::Request::Browse(crate::catalogues::read::BrowseQuery {
                q: Some("lantern".into()),
                ..Default::default()
            }),
            false,
        ),
        (
            super::super::catalogue::Request::Entry("lantern-flare".into()),
            true,
        ),
    ] {
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let entered = std::sync::Mutex::new(Some(entered));
        let released = std::sync::Mutex::new(released);
        *super::super::catalogue::AFTER_CATALOGUE_VALIDATION
            .lock()
            .unwrap() = Some(Arc::new(move || {
            // The runtime permit is held across validation.
            let busy = crate::definitions::admit().is_err();
            entered.lock().unwrap().take().unwrap().send(busy).unwrap();
            released
                .lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
        }));
        let store = world.store.clone();
        let auth = world.auth[3].clone();
        let permit = crate::definitions::admit().unwrap();
        let reading = tokio::spawn(async move {
            store
                .private_catalogue(auth, 1, "spells".into(), request, permit)
                .await
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(5), waiting)
                .await
                .unwrap()
                .unwrap(),
            "permit released before validation"
        );
        if demote {
            world
                .store
                .change_role(world.auth[1].clone(), world.campaign, 4, 2, false)
                .await
                .unwrap();
        }
        release.send(()).unwrap();
        let result = reading.await.unwrap();
        if demote {
            assert!(
                matches!(result, Err(AppError::NotFound)),
                "A demoted Master read the catalogue across the validation gap"
            );
        } else {
            let (_, permit) = result.unwrap();
            assert!(crate::definitions::admit().is_err(), "permit not returned");
            drop(permit);
        }
        assert!(crate::definitions::admit().is_ok());
    }
}

/// Damage one stored row past its guards: the named immutability trigger is dropped and
/// restored around `statement`, with foreign keys and CHECK constraints off meanwhile.
async fn tamper(store: &Store, trigger: &'static str, statement: String) {
    store
        .run(move |db| {
            let original: String = db.query_row(
                "SELECT sql FROM sqlite_schema WHERE name=?1",
                [trigger],
                |row| row.get(0),
            )?;
            db.execute_batch(&format!(
                "PRAGMA foreign_keys=OFF; PRAGMA ignore_check_constraints=ON; DROP TRIGGER {trigger}"
            ))?;
            db.execute_batch(&statement)?;
            db.execute_batch(&original)?;
            db.execute_batch("PRAGMA ignore_check_constraints=OFF; PRAGMA foreign_keys=ON")?;
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn corrupt_pins_and_snapshots_are_refused() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let damages: [(&str, &str, &str, StatusCode); 4] = [
        (
            "definition hash",
            "definition_versions_update",
            "UPDATE definition_versions SET content_hash='ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff'",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "snapshot hash",
            "private_sheet_history_update",
            "UPDATE private_sheet_history SET hash='ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff'",
            // The bounded loader finds no valid current event: the uniform refusal.
            StatusCode::FORBIDDEN,
        ),
        // R6: a corrupt pair. The pinned definition row names another game.
        (
            "definition pair",
            "definition_versions_update",
            "UPDATE definition_versions SET game_id='other-game'",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        // Isolated pin-column damage: the bounded loader refuses it (the uniform refusal),
        // because `private_sheet_events_valid` requires the stored snapshot's pin to equal
        // `json(s.pin)`. A coordinated rewrite of the pin, snapshots and hashes passes the
        // loader and reaches the route's own arm instead
        // (`a_consistently_rewritten_unsupported_pin_is_refused_by_the_route`).
        (
            "pin column",
            "private_sheets_update",
            "UPDATE private_sheets SET pin=json_set(pin,'$.schema',2)",
            StatusCode::FORBIDDEN,
        ),
    ];
    for (damage, trigger, statement, expected) in damages {
        let world = world().await;
        let unreadable = refusal(&world, 1).await;
        tamper(&world.store, trigger, statement.into()).await;
        for path in PATHS {
            let (status, html) = get(&world.app, &world.auth[2], path).await;
            assert_eq!(status, expected, "{damage}: {path}");
            assert!(!html.contains("Lantern"), "{damage}: {path}");
            if expected == StatusCode::FORBIDDEN {
                assert_eq!((status, html), unreadable, "{damage}: {path}");
            }
        }
    }
}

#[tokio::test]
async fn definition_links_are_checked_at_save_publication_and_readiness() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let world = world().await;
    let code = |result: Result<
        crate::definitions::DefinitionDraft,
        crate::definitions::DefinitionError,
    >| match result {
        Err(crate::definitions::DefinitionError::Invalid(report)) => format!(
            "{} {}",
            report.diagnostics[0].path, report.diagnostics[0].code
        ),
        Err(other) => format!("other {other}"),
        Ok(_) => "saved".into(),
    };
    // Site 1, draft save: only published catalogues of the definition's own pair.
    for (links, expected) in [
        (
            vec!["nothing"],
            "/catalogues/0 linked-catalogue-not-published",
        ),
        (
            vec!["spells", "tomes"],
            "/catalogues/1 linked-catalogue-not-published",
        ),
        (vec!["bestiary"], "saved"),
    ] {
        let draft = definition(&format!("draft-{}", links.len()), &links);
        let saved = world
            .store
            .save_definition_draft(serde_json::to_vec(&draft).unwrap(), 0)
            .await;
        assert_eq!(code(saved), expected, "{links:?}");
    }
    // Site 2, publication: the comparison and the approval transaction recheck the link.
    let draft = world
        .store
        .save_definition_draft(
            serde_json::to_vec(&definition("late", &["items"])).unwrap(),
            0,
        )
        .await
        .unwrap();
    let comparison = world
        .store
        .compare_definition(draft.id, false)
        .await
        .unwrap();
    remove_seals(&world.store, "items").await;
    let approval = world
        .store
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
        .await;
    assert!(
        matches!(&approval, Err(crate::definitions::DefinitionError::Invalid(report)) if report.diagnostics[0].code == "linked-catalogue-not-published"),
        "{:?}",
        approval.err()
    );
    assert!(matches!(
        world.store.compare_definition(draft.id, false).await,
        Err(crate::definitions::DefinitionError::Invalid(_))
    ));
    assert_readiness_site(&world).await;
}

/// Site 3, readiness: after damage removed the seals of `items`, the published definition
/// linking it is no longer current, the sheet says so, and new sheets of it cannot be created.
async fn assert_readiness_site(world: &World) {
    let (status, sheet) = get(&world.app, &world.auth[2], "/private-sheets/1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(sheet.contains("Wiring approval is stale, withdrawn, or evidence is unavailable."));
    assert!(sheet.contains("items: catalogue unavailable"));
    assert_eq!(
        get(
            &world.app,
            &world.auth[2],
            "/private-sheets/1/catalogue/items"
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(
            &world.app,
            &world.auth[2],
            "/private-sheets/1/catalogue/spells"
        )
        .await
        .0,
        StatusCode::OK
    );
    let path = format!(
        "/campaigns/{}/sheets/new?version={}",
        world.campaign, world.version
    );
    let form = vec![
        ("csrf".into(), "a".repeat(64)),
        ("revision".into(), "0".into()),
        ("name".into(), "Second sheet".into()),
        ("notes".into(), String::new()),
        ("value-label".into(), "Text".into()),
    ];
    assert_ne!(
        router_request(&world.app, &world.auth[3], &path, Some(form))
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
}

#[tokio::test]
async fn catalogue_publication_never_stales_definitions_or_sheets() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let world = world().await;
    // A second definition links the same catalogue (two definitions, one catalogue).
    let second = publish_definition(&world.store, &definition("second", &["spells"])).await;
    let pins = |store: Store| async move {
        store
            .run(|db| {
                let rows = db
                    .prepare("SELECT id,content_hash FROM definition_versions ORDER BY id")?
                    .query_map([], |row| {
                        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                let sheet: String =
                    db.query_row("SELECT hash FROM private_sheets WHERE id=1", [], |row| {
                        row.get(0)
                    })?;
                Ok((rows, sheet))
            })
            .await
            .unwrap()
    };
    let before = pins(world.store.clone()).await;
    let comparison = world.store.compare_definition(1, false).await.unwrap();
    // Version 2 adds an entry and renames one (append-only).
    let mut next = spells(vec![owner_entry(
        "new-charm",
        "charm",
        "New Charm",
        "Added later.",
    )]);
    next["entries"][0]["name"] = "Lantern Flare Renamed".into();
    publish_catalogue(&world.store, &next, 1).await;
    assert_eq!(pins(world.store.clone()).await, before);
    assert_eq!(
        world
            .store
            .compare_definition(1, false)
            .await
            .unwrap()
            .comparison_hash,
        comparison.comparison_hash,
        "catalogue state entered the wiring comparison"
    );
    for version in [world.version, second] {
        assert_eq!(
            world
                .store
                .definition_version(version)
                .await
                .unwrap()
                .wiring_status,
            crate::definitions::WiringStatus::Current
        );
    }
    let (_, sheet) = get(&world.app, &world.auth[2], "/private-sheets/1").await;
    assert!(!sheet.contains("Wiring approval is stale"));
    let (_, entry) = get(
        &world.app,
        &world.auth[2],
        "/private-sheets/1/catalogue/spells/lantern-flare",
    )
    .await;
    assert!(entry.contains("<h1><bdi>Lantern Flare Renamed</bdi></h1>"));
    let (_, browse) = get(
        &world.app,
        &world.auth[2],
        "/private-sheets/1/catalogue/spells?q=charm",
    )
    .await;
    assert!(browse.contains("New Charm") && browse.contains("House Charm"));
    // New sheets of both definitions can still be created and read the catalogue.
    create_sheet(&world, &world.auth[3], world.version).await;
    create_sheet(&world, &world.auth[3], second).await;
    for sheet in [2, 3] {
        assert_eq!(
            get(
                &world.app,
                &world.auth[3],
                &format!("/private-sheets/{sheet}/catalogue/spells/new-charm")
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    assert_eq!(
        get(
            &world.app,
            &world.auth[3],
            "/private-sheets/3/catalogue/items"
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "the second definition does not link items"
    );
}

#[tokio::test]
async fn browse_pages_link_to_the_next_page_with_the_filters() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let world = world().await;
    let extra = (0..55)
        .map(|n| owner_entry(&format!("echo-{n:02}"), "charm", "Echo", "Echo charm."))
        .collect();
    publish_catalogue(&world.store, &spells(extra), 1).await;
    let (status, first) = get(
        &world.app,
        &world.auth[2],
        "/private-sheets/1/catalogue/spells?type=charm&q=echo",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    crate::auth::nav::check::assert_header(&first, false, Some("/private-sheets"), true, "browse");
    assert_eq!(first.matches("<li><a href=").count(), 50);
    assert!(
        first.contains("href=\"/private-sheets/1/catalogue/spells?type=charm&#38;q=echo&#38;after=echo-49\">Next entries</a>"),
        "{}",
        &first[first.find("Next").map_or(0, |at| at.saturating_sub(200))..]
    );
    assert!(first.contains("<option value=\"charm\" dir=\"auto\" selected>Charm</option>"));
    assert!(first.contains("value=\"echo\""));
    let (_, second) = get(
        &world.app,
        &world.auth[2],
        "/private-sheets/1/catalogue/spells?type=charm&q=echo&after=echo-49",
    )
    .await;
    assert_eq!(second.matches("<li><a href=").count(), 5);
    assert!(!second.contains("Next entries"));
    assert!(second.contains(
        "href=\"/private-sheets/1/catalogue/spells?type=charm&#38;q=echo\">First entries</a>"
    ));
    // A query that cannot run shows its static message with the form.
    let (status, problem) = get(
        &world.app,
        &world.auth[2],
        "/private-sheets/1/catalogue/spells?q=%2A%2A",
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    crate::auth::nav::check::assert_header(
        &problem,
        false,
        Some("/private-sheets"),
        true,
        "problem",
    );
    assert!(problem.contains("Search text needs at least one word with letters or digits."));
    assert!(problem.contains("role=\"search\""));
}

/// What changes while a catalogue read is paused between validation and its second
/// transaction (R6/R25; step 3 round-1 review, Sol 1 and Space Bunny M2/m8).
#[derive(Clone, Copy, Debug)]
enum Gap {
    Nothing,
    /// The custodian's sheet is saved: the second transaction must refuse a stale view (409).
    SheetSaved,
    /// The pinned definition row changes: `pin.unchanged` must refuse it.
    PinChanged,
    /// The reader's account epoch advances (session revoked): `auth.recheck` must refuse it.
    SessionRevoked,
}

async fn across_the_gap(gap: Gap) -> Result<(), AppError> {
    let world = world().await;
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let entered = std::sync::Mutex::new(Some(entered));
    let released = std::sync::Mutex::new(released);
    *super::super::catalogue::AFTER_CATALOGUE_VALIDATION
        .lock()
        .unwrap() = Some(Arc::new(move || {
        entered.lock().unwrap().take().unwrap().send(()).unwrap();
        released
            .lock()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
    }));
    let store = world.store.clone();
    let reader = world.auth[2].clone();
    let permit = crate::definitions::admit().unwrap();
    let reading = tokio::spawn(async move {
        store
            .private_catalogue(
                reader,
                1,
                "spells".into(),
                super::super::catalogue::Request::Entry("lantern-flare".into()),
                permit,
            )
            .await
            .map(|_| ())
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), waiting)
        .await
        .unwrap()
        .unwrap();
    match gap {
        Gap::Nothing => {}
        Gap::SheetSaved => {
            let auth = world.auth[2].clone();
            let version = world.version;
            world
                .store
                .run_campaign(auth.clone(), move |db| {
                    let tx =
                        db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                    let mut snapshot = super::super::snapshots::load(&tx, 1)?;
                    snapshot.name = "Saved meanwhile".into();
                    let pin = crate::definitions::private_pin::validate(
                        crate::definitions::private_pin::load(&tx, version)?,
                    )?;
                    super::super::snapshots::write(&tx, &auth, snapshot, "edit", Some(&pin))?;
                    tx.commit()?;
                    Ok(())
                })
                .await
                .unwrap();
        }
        Gap::PinChanged => {
            tamper(
                &world.store,
                "definition_versions_update",
                format!(
                    "UPDATE definition_versions SET content_hash='{}' WHERE id={}",
                    "e".repeat(64),
                    world.version
                ),
            )
            .await;
        }
        Gap::SessionRevoked => {
            world
                .store
                .run(|db| {
                    db.execute("UPDATE accounts SET epoch=2 WHERE id=3", [])?;
                    Ok(())
                })
                .await
                .unwrap();
        }
    }
    release.send(()).unwrap();
    reading.await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_second_transaction_rechecks_the_sheet_the_pin_and_the_session() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    assert!(across_the_gap(Gap::Nothing).await.is_ok());
    let saved = across_the_gap(Gap::SheetSaved).await;
    assert!(matches!(saved, Err(AppError::Conflict)), "{saved:?}");
    let changed = across_the_gap(Gap::PinChanged).await;
    assert!(
        matches!(
            changed,
            Err(AppError::Invalid("Immutable definition changed."))
        ),
        "{changed:?}"
    );
    let revoked = across_the_gap(Gap::SessionRevoked).await;
    assert!(matches!(revoked, Err(AppError::NotFound)), "{revoked:?}");
}

/// The 404 page of `key` in the linked `spells` catalogue, as the custodian reads it.
async fn no_spell(world: &World, key: &str) -> String {
    let (status, html) = get(
        &world.app,
        &world.auth[2],
        &format!("/private-sheets/1/catalogue/spells/{key}"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{html}");
    assert!(html.contains(&format!(
        "No entry “{key}” in <bdi>Spells &#60;i&#62;of&#60;/i&#62; light</bdi>."
    )));
    html
}

#[tokio::test]
async fn catalogues_are_confined_to_the_pair_the_seal_and_the_catalogue() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let world = world().await;
    // The same game and catalogue id in another edition, at a higher version.
    world
        .store
        .run(|db| {
            db.execute(
                "INSERT INTO registry_editions VALUES('synthetic-game','second','Second',0)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let mut second = owner_catalogue(
        "synthetic-game",
        "spells",
        &[owner_entry(
            "second-only",
            "item",
            "Second Edition Spell",
            "Elsewhere.",
        )],
    );
    second["edition_id"] = "second".into();
    publish_catalogue(&world.store, &second, 0).await;
    second["title"] = "Second edition spells, again".into();
    publish_catalogue(&world.store, &second, 1).await;
    // The key exists only in the other edition: not found in this catalogue, nothing shown.
    // The key exists only in the other edition: not found in this catalogue, nothing shown.
    let html = no_spell(&world, "second-only").await;
    assert!(!html.contains("Second Edition") && !html.contains("Elsewhere"));
    let (_, found) = get(
        &world.app,
        &world.auth[2],
        "/private-sheets/1/catalogue/spells?q=second",
    )
    .await;
    assert!(found.contains("No entries match.") && !found.contains("Second Edition"));
    // A cursor key that exists only in another catalogue restarts from the beginning.
    let (_, first_page) = get(
        &world.app,
        &world.auth[2],
        "/private-sheets/1/catalogue/spells",
    )
    .await;
    let (_, foreign_cursor) = get(
        &world.app,
        &world.auth[2],
        "/private-sheets/1/catalogue/spells?after=other-only",
    )
    .await;
    let results = |html: &str| {
        let start = html.find("<ul").unwrap();
        html[start..start + html[start..].find("</ul>").unwrap()].to_owned()
    };
    assert_eq!(results(&foreign_cursor), results(&first_page));
    // A sealed v1 beside an unsealed v2: readers stay on v1.
    let mut next = spells(vec![owner_entry(
        "unsealed-only",
        "charm",
        "Unsealed Only",
        "Never sealed.",
    )]);
    next["title"] = "Unsealed spells".into();
    world
        .store
        .save_catalogue_draft(serde_json::to_vec(&next).unwrap(), 1)
        .await
        .unwrap();
    world.store.run(|db| {
        db.execute("INSERT INTO catalogue_versions(catalogue,game_id,edition_id,catalogue_id,version,title,content,content_hash,draft_id,draft_revision,comparison_hash,actor,rationale,admission_profile) SELECT c.id,d.game_id,d.edition_id,d.catalogue_id,2,json_extract(d.content,'$.title'),d.content,d.content_hash,d.id,d.revision,d.content_hash,'Synthetic operator','Unsealed fixture','catalogue-admission-1' FROM catalogue_drafts d JOIN catalogues c ON c.game_id=d.game_id AND c.edition_id=d.edition_id AND c.catalogue_id=d.catalogue_id WHERE d.game_id='synthetic-game' AND d.edition_id='first' AND d.catalogue_id='spells'", [])?;
        let unsealed = db.last_insert_rowid();
        db.execute("INSERT INTO catalogue_entry_types SELECT ?1,t.type_id,t.ordinal,t.label,t.properties FROM catalogue_entry_types t JOIN catalogue_versions v ON v.id=t.version_id WHERE v.game_id='synthetic-game' AND v.edition_id='first' AND v.catalogue_id='spells' AND v.version=1", [unsealed])?;
        db.execute("INSERT INTO catalogue_entries(version_id,key,ordinal,type_id,name,retired,desc_kind,owner_text,properties) VALUES(?1,'unsealed-only',0,'charm','Unsealed Only',0,'owner_text','Never sealed.','{}')", [unsealed])?;
        Ok(())
    }).await.unwrap();
    let html = no_spell(&world, "unsealed-only").await;
    assert!(!html.contains("Unsealed") && !html.contains("Never sealed"));
    let (status, sealed) = get(
        &world.app,
        &world.auth[2],
        "/private-sheets/1/catalogue/spells",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(sealed.contains("<h1><bdi>Spells &#60;i&#62;of&#60;/i&#62; light</bdi></h1>"));
    assert!(!sealed.contains("Unsealed"));
}

#[tokio::test]
async fn malformed_units_hide_every_passage_and_never_show_reviewed() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let world = world().await;
    // Review every unit of page 4 and complete it: a malformed text still never reads
    // "Reviewed extraction", on browse or on the entry page.
    for unit in [7, 8, 9, 10, 11, 12] {
        review(&world.store, unit, "accepted", "").await;
    }
    complete(&world.store, 4, "iv").await;
    let (_, browse) = get(
        &world.app,
        &world.auth[2],
        "/private-sheets/1/catalogue/spells",
    )
    .await;
    let (_, entry) = get(
        &world.app,
        &world.auth[2],
        "/private-sheets/1/catalogue/spells/malformed",
    )
    .await;
    assert!(browse.contains("<bdi>Malformed Text</bdi></a> · <bdi>Spell</bdi> <span class=\"badge\">Description unavailable</span>"));
    assert!(browse.contains("<bdi>Hostile &#60;b&#62;Name&#60;/b&#62;</bdi></a> · <bdi>Spell</bdi> <span class=\"badge\">Reviewed extraction</span>"));
    assert!(entry.contains("<span class=\"badge\">Description unavailable</span> The stored text of a cited passage cannot be read, or the citations of this entry are missing"));
    assert!(
        entry.contains("Description unavailable (source text malformed or citations missing).")
    );
    assert!(!entry.contains("Reviewed extraction"));
    // A long valid unit followed by a malformed one (NUL, BLOB, over 256 KiB): none of the
    // first unit's 20,000 displayable characters is shown.
    for key in ["cut-nul", "cut-blob", "cut-oversized"] {
        let (status, html) = get(
            &world.app,
            &world.auth[2],
            &format!("/private-sheets/1/catalogue/spells/{key}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{key}");
        assert_eq!(count(&html, 'ж'), 0, "{key}");
        assert!(
            html.contains("Description unavailable (source text malformed or citations missing)."),
            "{key}"
        );
        assert!(
            html.contains("PDF page 2</li>") && html.contains("p. <bdi>iv</bdi> (PDF page 4)</li>"),
            "{key}"
        );
        assert!(
            browse.contains(&format!(
                "/{key}\"><bdi>{}</bdi></a> · <bdi>Spell</bdi> <span class=\"badge\">Description unavailable</span>",
                match key {
                    "cut-nul" => "Cut Then NUL",
                    "cut-blob" => "Cut Then BLOB",
                    _ => "Cut Then Oversized",
                }
            )),
            "{key}"
        );
    }
    // An entry whose unit rows are missing (damage): the same badge and note on both routes.
    tamper(
        &world.store,
        "catalogue_entry_units_delete",
        "DELETE FROM catalogue_entry_units WHERE key='multibyte'".into(),
    )
    .await;
    let (_, browse) = get(
        &world.app,
        &world.auth[2],
        "/private-sheets/1/catalogue/spells",
    )
    .await;
    assert!(browse.contains("<bdi>Multibyte Text</bdi></a> · <bdi>Spell</bdi> <span class=\"badge\">Description unavailable</span>"));
    let (status, empty) = get(
        &world.app,
        &world.auth[2],
        "/private-sheets/1/catalogue/spells/multibyte",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        empty.contains("Description unavailable (source text malformed or citations missing).")
    );
    assert_eq!(count(&empty, 'é'), 0);
    // A retired entry's page stays readable and says so.
    let (status, retired) = get(
        &world.app,
        &world.auth[2],
        "/private-sheets/1/catalogue/spells/retired-charm",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(retired.contains("<span class=\"badge\">Retired</span> Kept for existing sheets"));
    assert!(retired.contains("Old charm."));
}

#[tokio::test]
async fn history_pages_omit_links_and_damaged_titles_degrade() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let world = world().await;
    let (status, history) = get(&world.app, &world.auth[2], "/private-sheets/1/history/1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!history.contains("catalogues-heading") && !history.contains("/catalogue/"));
    // A damaged (oversized) title: the sheet still renders, the link reads as unavailable,
    // and the routes show the catalogue id instead.
    tamper(
        &world.store,
        "catalogue_versions_update",
        format!(
            "UPDATE catalogue_versions SET title='{}' WHERE game_id='synthetic-game' AND catalogue_id='spells'",
            "t".repeat(500)
        ),
    )
    .await;
    let (status, sheet) = get(&world.app, &world.auth[2], "/private-sheets/1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(sheet.contains("spells: catalogue unavailable"));
    assert!(sheet.contains("href=\"/private-sheets/1/catalogue/items\""));
    let (status, browse) = get(
        &world.app,
        &world.auth[2],
        "/private-sheets/1/catalogue/spells",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(browse.contains("<h1><bdi>spells</bdi></h1>"));
    assert!(!browse.contains("ttttt"));
}

#[tokio::test]
async fn type_options_render_with_automatic_direction() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let world = world().await;
    let (_, browse) = get(
        &world.app,
        &world.auth[2],
        "/private-sheets/1/catalogue/spells",
    )
    .await;
    // An <option> cannot hold <bdi>; `dir="auto"` gives each label its own direction.
    assert!(browse.contains("<option value=\"spell\" dir=\"auto\">Spell</option>"));
}

/// Rewrites sheet 1's pin, current snapshot and history consistently (the loader's equality
/// checks all hold) so that its runtime pin names an unsupported schema 2.
async fn rewrite_pin_consistently(store: &Store) {
    let content: String = store
        .run(|db| {
            Ok(db.query_row(
                "SELECT json_set(snapshot,'$.pin.schema',2) FROM private_sheets WHERE id=1",
                [],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    let hash = super::super::snapshots::hash(content.as_bytes());
    let quoted = content.replace('\'', "''");
    tamper(
        store,
        "private_sheets_update",
        format!("UPDATE private_sheets SET pin=json_set(pin,'$.schema',2),snapshot='{quoted}',hash='{hash}' WHERE id=1"),
    )
    .await;
    tamper(
        store,
        "private_sheet_history_update",
        format!(
            "UPDATE private_sheet_history SET snapshot='{quoted}',hash='{hash}' WHERE sheet_id=1"
        ),
    )
    .await;
}

#[tokio::test]
async fn a_consistently_rewritten_unsupported_pin_is_refused_by_the_route() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let world = world().await;
    let unreadable = refusal(&world, 1).await;
    rewrite_pin_consistently(&world.store).await;
    // The loader accepts the rewrite: the sheet reads as an unsupported, read-only pin.
    let (status, sheet) = get(&world.app, &world.auth[2], "/private-sheets/1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(sheet.contains("Unsupported immutable interpretation pin"));
    assert!(!sheet.contains("catalogues-heading"));
    // So only the route's own arm (runtime schema 1, supported) refuses it.
    for path in PATHS {
        assert_eq!(
            get(&world.app, &world.auth[2], path).await,
            unreadable,
            "{path}"
        );
    }
}

/// Slice 1.5a review (Sol Minor 2): an unsupported pin is read-only, so its unit routes
/// answer 403 (no edit rights) before any key or route check, and nothing is written.
#[tokio::test]
async fn unit_routes_refuse_an_unsupported_pin_as_read_only() {
    use tower::ServiceExt;
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let world = world().await;
    rewrite_pin_consistently(&world.store).await;
    let revision = || async {
        world
            .store
            .run(|db| {
                Ok(
                    db.query_row("SELECT revision FROM private_sheets WHERE id=1", [], |r| {
                        r.get::<_, i64>(0)
                    })?,
                )
            })
            .await
            .unwrap()
    };
    let before = revision().await;
    for path in [
        "/private-sheets/1/meta/name",
        "/private-sheets/1/meta/notes",
        "/private-sheets/1/fields/missing",
    ] {
        let body = serde_urlencoded::to_string([
            ("csrf", "a".repeat(64)),
            ("base", before.to_string()),
            ("value", "Renamed".to_owned()),
        ])
        .unwrap();
        let response = world
            .app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri(path)
                    .method("POST")
                    .header("host", "127.0.0.1:3000")
                    .header("origin", "http://127.0.0.1:3000")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("cookie", world.auth[2].fixture_cookie())
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
    }
    assert_eq!(revision().await, before, "nothing written");
}

#[tokio::test]
async fn index_identity_damage_never_renders_the_query_problem_form() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let world = world().await;
    let largest = i64::MAX;
    tamper(
        &world.store,
        "catalogue_versions_update",
        format!("UPDATE catalogue_versions SET catalogue={largest} WHERE game_id='synthetic-game' AND edition_id='first' AND catalogue_id='spells'"),
    )
    .await;
    tamper(
        &world.store,
        "catalogues_update",
        format!("UPDATE catalogues SET id={largest} WHERE game_id='synthetic-game' AND edition_id='first' AND catalogue_id='spells'"),
    )
    .await;
    // A plain browse does not use the index identity and still works.
    let (status, _) = get(
        &world.app,
        &world.auth[2],
        "/private-sheets/1/catalogue/spells",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // A search does, and the damage answers as an error, not as the reader's query problem.
    let (status, html) = get(
        &world.app,
        &world.auth[2],
        "/private-sheets/1/catalogue/spells?q=lantern",
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        !html.contains("role=\"search\"") && !html.contains("index identity"),
        "{html}"
    );
}

#[tokio::test]
async fn a_lost_source_row_gives_the_same_badge_on_both_routes() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let world = world().await;
    tamper(
        &world.store,
        "sources_finalized_delete",
        "DELETE FROM sources WHERE id=1".into(),
    )
    .await;
    let (_, browse) = get(
        &world.app,
        &world.auth[2],
        "/private-sheets/1/catalogue/spells",
    )
    .await;
    assert!(browse.contains("<bdi>Lantern Flare</bdi></a> · <bdi>Spell</bdi> <span class=\"badge\">Description unavailable</span>"));
    let (status, entry) = get(
        &world.app,
        &world.auth[2],
        "/private-sheets/1/catalogue/spells/lantern-flare",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(entry.contains("<span class=\"badge\">Description unavailable</span>"));
    assert!(
        entry.contains("Description unavailable (source text malformed or citations missing).")
    );
}
