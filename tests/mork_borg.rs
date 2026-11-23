use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mimik::{
    AppError, CharacterInput, Store, app,
    mork_borg::{EDITION, Sheet},
};
use tower::ServiceExt;

fn character() -> CharacterInput {
    CharacterInput {
        name: "Varg".into(),
        system_id: "mork-borg".into(),
        edition: EDITION.into(),
        notes: String::new(),
        revision: 0,
    }
}

#[test]
fn load_thresholds_and_damaged_heavy_armor_keep_distinct_penalties() {
    let mut sheet = Sheet {
        strength: -3,
        worn_armor: 3,
        protection: 0,
        standard_items: 5,
        ..Sheet::default()
    };
    assert_eq!(sheet.carrying_limit(), 5);
    assert!(!sheet.overburdened());
    assert_eq!(sheet.defense_dr(), 14);
    assert_eq!(sheet.agility_dr(), 16);
    assert_eq!(sheet.armor_reduction(), "0");
    sheet.standard_items = 6;
    assert!(sheet.overburdened());
    assert_eq!(sheet.melee_dr(), 14);
    assert_eq!(sheet.defense_dr(), 16);
    assert_eq!(sheet.agility_dr(), 18);
    assert_eq!(sheet.ranged_dr(), 12);
    sheet.standard_items = 10;
    assert!(!sheet.over_maximum());
    sheet.standard_items = 11;
    assert!(sheet.over_maximum());
}

#[test]
fn hp_zero_and_negative_are_valid_states_not_clamped_to_one() {
    let mut sheet = Sheet {
        hp: 0,
        ..Sheet::default()
    };
    assert!(sheet.validate().is_ok());
    assert!(sheet.health_status().contains("Broken"));
    sheet.hp = -1;
    assert!(sheet.validate().is_ok());
    assert!(sheet.health_status().contains("Dead"));
    sheet.hp = 2;
    assert!(sheet.validate().is_err());
    sheet.hp = 1;
    sheet.protection = 3;
    assert!(sheet.validate().is_err());
}

#[tokio::test]
async fn stats_survive_reopen_and_identity_edits_invalidate_old_sheet_revision() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.sqlite3");
    let store = Store::open(&path).unwrap();
    let created = store.create(character()).await.unwrap();
    let (_, mut sheet, saved) = store.mork_borg_sheet(created.id).await.unwrap();
    assert!(!saved);
    sheet.hp = 0;
    sheet.strength = 3;
    sheet.equipment = "Locally entered gear".into();
    let (current, saved_sheet) = store.save_mork_borg_sheet(created.id, sheet).await.unwrap();
    let mut identity = current.input;
    identity.notes = "New identity notes".into();
    store.update(created.id, identity).await.unwrap();
    assert!(matches!(
        store.save_mork_borg_sheet(created.id, saved_sheet).await,
        Err(AppError::Conflict)
    ));
    drop(store);
    let store = Store::open(&path).unwrap();
    let (_, sheet, saved) = store.mork_borg_sheet(created.id).await.unwrap();
    assert!(saved);
    assert_eq!(sheet.strength, 3);
    assert_eq!(sheet.hp, 0);
    assert_eq!(sheet.revision, 3);
}

#[tokio::test]
async fn existing_v1_database_migrates_without_losing_characters() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.sqlite3");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(include_str!("../migrations/001_characters.sql"))
        .unwrap();
    db.execute("INSERT INTO characters (name, system_id, edition) VALUES ('Old character', 'mork-borg', ?1)", [EDITION]).unwrap();
    drop(db);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.get(1).await.unwrap().input.name, "Old character");
    let (_, sheet, _) = store.mork_borg_sheet(1).await.unwrap();
    store.save_mork_borg_sheet(1, sheet).await.unwrap();
}

#[tokio::test]
async fn wrong_editions_are_not_silently_treated_as_italian_2025() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.sqlite3")).unwrap();
    let mut input = character();
    input.edition = "English 2019".into();
    let created = store.create(input).await.unwrap();
    assert!(matches!(
        store.mork_borg_sheet(created.id).await,
        Err(AppError::Invalid(_))
    ));
}

#[tokio::test]
async fn htmx_sheet_save_refreshes_stats_and_preserves_invalid_edits() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.sqlite3")).unwrap();
    store.create(character()).await.unwrap();
    let router = app(store.clone());
    let body = "agility=0&presence=0&strength=-3&toughness=0&hp=0&max_hp=5&worn_armor=3&protection=1&standard_items=6&silver=10&omens=2&equipment=gear&revision=1";
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/characters/1/mork-borg")
                .header("host", "127.0.0.1:3000")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("hx-request", "true")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = String::from_utf8(
        to_bytes(response.into_body(), 100_000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("Overburdened"));
    assert!(html.contains("against DR 16"));
    assert!(html.contains("name=\"revision\" value=\"2\""));
    assert!(!html.contains("<!doctype"));
    let body = body
        .replace("strength=-3", "strength=20")
        .replace("revision=1", "revision=2");
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/characters/1/mork-borg")
                .header("host", "127.0.0.1:3000")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("hx-request", "true")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let html = String::from_utf8(
        to_bytes(response.into_body(), 100_000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("Base ability modifiers"));
    assert!(html.contains("name=\"strength\" value=\"20\""));
    assert_eq!(store.mork_borg_sheet(1).await.unwrap().1.strength, -3);
}
