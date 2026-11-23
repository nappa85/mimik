//! Regressions of the native-sheet removal (PROD-25): a sheet without an admitted immutable
//! runtime pin admits no form, and a sheet is never created without one.
use super::{Pin, PrivateSnapshot, fixture, published_fixture, router_request};
use crate::campaigns::{sheets::SheetView, transport::Pairs, web::admit_form};
use axum::http::StatusCode;

/// An unsupported pin (schema 2 of a runtime definition) has no admitted definition, hence no
/// closed key set: its POST is a 400 whatever keys it carries. Before the removal this branch
/// admitted the keys of the native sheet form instead.
#[tokio::test]
async fn regression_a_sheet_without_an_admitted_pin_admits_no_form_keys() {
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let snapshot = PrivateSnapshot {
        encoding: 1,
        sheet_id: 1,
        campaign_id: 1,
        game_id: "synthetic-game".into(),
        edition_id: "first".into(),
        custodian_id: 2,
        kind: "runtime".into(),
        pin: Pin::Runtime {
            version_id: 1,
            version: 1,
            content_hash: "0".repeat(64),
            schema: 2,
        },
        revision: 1,
        state: "active".into(),
        name: "Retained".into(),
        notes: String::new(),
        data: serde_json::json!({}),
    };
    let view = SheetView {
        snapshot,
        immutable: None,
        warning: String::new(),
        editable: true,
        changes: None,
        catalogues: Vec::new(),
        entry_links: crate::campaigns::catalogue::EntryLinks::new(),
        entry_matches: crate::campaigns::catalogue::Matches::new(),
        notes: crate::campaigns::notes::Notes::default(),
    };
    // Exactly the keys of the removed native sheet form: the closed set the old branch admitted.
    let keys = [
        "csrf",
        "revision",
        "name",
        "notes",
        "agility",
        "presence",
        "strength",
        "toughness",
        "hp",
        "max_hp",
        "worn_armor",
        "protection",
        "standard_items",
        "silver",
        "omens",
        "equipment",
        "description",
        "class_notes",
        "powers",
        "weapon_one",
        "weapon_two",
        "campaign_miseries",
    ];
    let pairs = Pairs::from_pairs(keys.map(|key| (key.into(), "1".into())).to_vec());
    let outcome = admit_form(view, pairs, crate::definitions::admit().unwrap()).await;
    assert_eq!(outcome.err(), Some(StatusCode::BAD_REQUEST));
}

/// The create route needs an approved runtime version of the campaign's own ruleset. `version=0`
/// used to select the hardcoded native sheet of a registered, source-loaded MÖRK BORG pair; it
/// now names no version at all, on the page and on the save (the uniform 403 of a refused sheet
/// route), and nothing is written.
#[tokio::test]
async fn regression_create_without_an_immutable_pin_is_refused_and_writes_nothing() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    store
        .register_ruleset(crate::registry::RegisterRuleset {
            game_id: "mork-borg".into(),
            game_label: "MÖRK BORG".into(),
            edition_id: "it-2025".into(),
            edition_label: "MÖRK BORG — IT 2025".into(),
        })
        .await
        .unwrap();
    store.run(|db| {
        db.execute("INSERT INTO sources(bundle_hash,source_id,title,system_id,edition,language,pdf_hash,raw_json,prepared_json,total_pages) VALUES('pinless','fixture','Fixture','mork-borg','Italian fixture','it','fixture',x'',x'',1)", [])?;
        db.execute("INSERT INTO source_rulesets VALUES(1,'mork-borg','it-2025')", [])?;
        Ok(())
    }).await.unwrap();
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Pinless".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    let path = format!("/campaigns/{campaign}/sheets/new?version=0");
    assert_eq!(
        router_request(&app, &auth[1], &path, None).await.status(),
        StatusCode::FORBIDDEN
    );
    let form: Vec<(String, String)> = [
        ("csrf", "a".repeat(64)),
        ("revision", "0".into()),
        ("name", "Pinless".into()),
        ("notes", "No pin".into()),
        ("agility", "0".into()),
        ("presence", "0".into()),
        ("strength", "0".into()),
        ("toughness", "0".into()),
        ("hp", "1".into()),
        ("max_hp", "1".into()),
        ("worn_armor", "0".into()),
        ("protection", "0".into()),
        ("standard_items", "0".into()),
        ("silver", "0".into()),
        ("omens", "0".into()),
        ("equipment", String::new()),
        ("description", String::new()),
        ("class_notes", String::new()),
        ("powers", String::new()),
        ("weapon_one", String::new()),
        ("weapon_two", String::new()),
        ("campaign_miseries", String::new()),
    ]
    .map(|(key, value)| (key.into(), value))
    .to_vec();
    assert_eq!(
        router_request(&app, &auth[1], &path, Some(form))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let sheets: i64 = store
        .run(|db| Ok(db.query_row("SELECT count(*) FROM private_sheets", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(sheets, 0);
}

/// Even a create that reaches the store with a runtime snapshot but no admitted pin is refused
/// there, before anything is written.
#[tokio::test]
async fn regression_the_store_refuses_a_create_without_an_admitted_pin() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let version = published_fixture(&store).await;
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Store".into(),
            "synthetic-game".into(),
            "first".into(),
        )
        .await
        .unwrap();
    let (mut view, permit) = store
        .private_create_context(
            auth[1].clone(),
            campaign,
            version,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    view.immutable = None;
    view.snapshot.name = "No pin".into();
    assert!(matches!(
        store.private_save(auth[1].clone(), view, permit).await,
        Err(crate::AppError::Invalid(_))
    ));
    let sheets: i64 = store
        .run(|db| Ok(db.query_row("SELECT count(*) FROM private_sheets", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(sheets, 0);
}
