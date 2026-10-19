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
    if unit == 7 {
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
    assert!(!sheet.contains("<script"), "sheet pages load no script");
    let reader = |auth: usize| (world.app.clone(), world.auth[auth].clone());
    // Custodian (3) and Master (2) read; another player (4), an outsider (5) and the
    // production Owner (1) are refused like an unreadable sheet.
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
    // A disabled custodian is refused exactly like the sheet itself.
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
    assert_ne!(refused.0, StatusCode::OK);
    for path in PATHS {
        assert_eq!(
            get(&world.app, &world.auth[2], path).await,
            refused,
            "{path}"
        );
    }
}

#[tokio::test]
async fn unlinked_unknown_foreign_and_native_catalogues_answer_like_an_unreadable_sheet() {
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
    let unreadable = get(&world.app, &world.auth[4], "/private-sheets/1").await;
    assert_eq!(unreadable.0, StatusCode::FORBIDDEN);
    for (account, path) in [
        (2, "/private-sheets/1/catalogue/bestiary"),
        (2, "/private-sheets/1/catalogue/bestiary/beast"),
        (2, "/private-sheets/1/catalogue/tomes"),
        (2, "/private-sheets/1/catalogue/nothing"),
        (2, "/private-sheets/1/catalogue/spells/nothing"),
        (2, "/private-sheets/1/catalogue/spells/beast"),
        (2, "/private-sheets/1/catalogue/spells/other-only"),
        (2, "/private-sheets/1/catalogue/items/giant"),
        (1, "/private-sheets/1/catalogue/bestiary"),
        (1, "/private-sheets/99/catalogue/spells"),
        (1, "/private-sheets/2/catalogue/spells"),
        (1, "/private-sheets/2/catalogue/spells/lantern-flare"),
        (4, "/private-sheets/1/catalogue/nothing"),
    ] {
        assert_eq!(
            get(&world.app, &world.auth[account], path).await,
            unreadable,
            "{path}"
        );
    }
    // Syntax is checked before any lookup.
    for path in [
        "/private-sheets/1/catalogue/Spells",
        "/private-sheets/1/catalogue/spells/Lantern_Flare",
        "/private-sheets/1/catalogue/spells?type=Spell",
        "/private-sheets/1/catalogue/spells?after=-x",
        "/private-sheets/1/catalogue/spells?unknown=1",
        "/private-sheets/1/catalogue/spells?q=a&q=b",
    ] {
        assert_eq!(
            get(&world.app, &world.auth[2], path).await.0,
            StatusCode::BAD_REQUEST,
            "{path}"
        );
    }
    let long = format!("/private-sheets/1/catalogue/spells?q={}", "a".repeat(101));
    assert_eq!(
        get(&world.app, &world.auth[2], &long).await.0,
        StatusCode::BAD_REQUEST
    );
    let limit = format!("/private-sheets/1/catalogue/spells?q={}", "a".repeat(100));
    assert_eq!(
        get(&world.app, &world.auth[2], &limit).await.0,
        StatusCode::OK
    );
    // An unknown type is refused like an unknown catalogue; an empty one means all types.
    assert_eq!(
        get(
            &world.app,
            &world.auth[2],
            "/private-sheets/1/catalogue/spells?type=potion"
        )
        .await,
        unreadable
    );
    assert_eq!(
        get(
            &world.app,
            &world.auth[2],
            "/private-sheets/1/catalogue/spells?type=&q=&after="
        )
        .await
        .0,
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
    assert!(html.contains("<dt>Level</dt><dd><bdi>1</bdi></dd><dt>School</dt><dd><bdi>Light</bdi></dd><dt>Note</dt><dd><bdi>Bright</bdi></dd>"));
    assert!(html.contains("PDF page 1</li>"));
    assert!(!html.contains("<b>manual</b>"), "source titles are escaped");
    assert!(
        !html.contains("/sources/"),
        "players get citations, never page images"
    );
    assert!(html.contains("Current text"));
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
        "<a href=\"/private-sheets/1/catalogue/spells/lantern-flare\"><bdi>Lantern Flare</bdi></a> · Spell <span class=\"badge\">Description unavailable</span>"
    ));
    assert!(
        browse.contains(
            "<bdi>House Charm</bdi></a> · Charm <span class=\"badge\">Owner-written</span>"
        )
    );
    assert!(browse.contains("<bdi>Retired Charm</bdi></a> · Charm <span class=\"badge\">Owner-written</span> <span class=\"badge\">Retired</span>"));
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
    assert!(malformed.contains("Description unavailable (source text malformed)."));
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

#[tokio::test]
async fn corrupt_pins_and_snapshots_are_refused() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    for damage in ["definition hash", "snapshot hash"] {
        let world = world().await;
        world
            .store
            .run(move |db| {
                let (trigger, statement) = if damage == "definition hash" {
                    (
                        "definition_versions_update",
                        "UPDATE definition_versions SET content_hash=?1",
                    )
                } else {
                    (
                        "private_sheet_history_update",
                        "UPDATE private_sheet_history SET hash=?1",
                    )
                };
                let original: String = db.query_row(
                    "SELECT sql FROM sqlite_schema WHERE name=?1",
                    [trigger],
                    |row| row.get(0),
                )?;
                db.execute_batch(&format!("PRAGMA foreign_keys=OFF; DROP TRIGGER {trigger}"))?;
                db.execute(statement, ["f".repeat(64)])?;
                db.execute_batch(&original)?;
                db.execute_batch("PRAGMA foreign_keys=ON")?;
                Ok(())
            })
            .await
            .unwrap();
        for path in PATHS {
            let (status, html) = get(&world.app, &world.auth[2], path).await;
            assert_ne!(status, StatusCode::OK, "{damage}: {path}");
            assert!(!html.contains("Lantern"), "{damage}: {path}");
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
    assert_eq!(first.matches("<li><a href=").count(), 50);
    assert!(
        first.contains("href=\"/private-sheets/1/catalogue/spells?type=charm&#38;q=echo&#38;after=echo-49\">Next entries</a>"),
        "{}",
        &first[first.find("Next").map_or(0, |at| at.saturating_sub(200))..]
    );
    assert!(first.contains("<option value=\"charm\" selected>Charm</option>"));
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
    assert!(problem.contains("Search text needs at least one word with letters or digits."));
    assert!(problem.contains("role=\"search\""));
}
