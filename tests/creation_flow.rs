use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mimik::{Store, app};
use tower::ServiceExt;

#[path = "support/source_fixture.rs"]
mod support;

#[tokio::test]
async fn creation_options_are_loaded_exact_editions_without_duplicates_and_legacy_remains() {
    let dir = tempfile::tempdir().unwrap();
    support::fixture_pair(dir.path()).unwrap();
    let store = Store::open(&dir.path().join("catalog.sqlite3")).unwrap();
    let pdf = dir.path().join("original.pdf");
    store
        .import_source(&dir.path().join("bundle"), &pdf)
        .await
        .unwrap();
    assert!(store.creation_catalog().await.unwrap().options.is_empty());
    store
        .import_source(&dir.path().join("adapter-bundle"), &pdf)
        .await
        .unwrap();
    assert_eq!(store.creation_catalog().await.unwrap().options.len(), 1);
    let created = store
        .create_available_character(mimik::games::CreateRequest {
            adapter_id: "mork-borg-it-2025".into(),
            name: "Loaded sheet".into(),
            notes: "notes preserved".into(),
        })
        .await
        .unwrap();
    assert!(created.has_mork_borg_adapter());
    let legacy = store
        .create(mimik::CharacterInput {
            name: "Legacy vampire".into(),
            system_id: "vampire".into(),
            edition: "Legacy other edition".into(),
            notes: String::new(),
            revision: 0,
        })
        .await
        .unwrap();
    assert_eq!(legacy.system_name(), "Vampire");
    assert_eq!(legacy.input.edition, "Legacy other edition");
    let response = app(store.clone())
        .oneshot(
            Request::builder()
                .uri("/")
                .header("host", "localhost:3000")
                .body(Body::empty())
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
    assert_eq!(html.matches("action=\"/characters\"").count(), 1);
    assert!(!html.contains("Game family"));
    assert!(!html.contains("New character record"));
    assert!(!html.contains("title/edition to confirm"));
    assert!(html.contains("Legacy vampire"));
    let updated = store
        .update(
            legacy.id,
            mimik::CharacterInput {
                name: "Updated legacy".into(),
                ..legacy.input
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.input.edition, "Legacy other edition");
}

#[tokio::test]
async fn home_has_one_creation_flow_and_does_not_advertise_unloaded_game_families() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("empty.sqlite3")).unwrap();
    let response = app(store)
        .oneshot(
            Request::builder()
                .uri("/")
                .header("host", "localhost:3000")
                .body(Body::empty())
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
    assert!(html.contains("No registered materials"));
    assert!(!html.contains("Game family"));
    assert!(!html.contains("Start a MÖRK BORG character"));
    assert!(!html.contains("New character record"));
    assert!(!html.contains("title/edition to confirm"));
    assert!(!html.contains("action=\"/characters\""));
}

#[tokio::test]
async fn legacy_creation_payload_cannot_create_an_unloaded_unimplemented_character() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("empty.sqlite3")).unwrap();
    let response = app(store.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/characters")
                .header("host", "localhost:3000")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "name=Unsupported&system_id=vampire&edition=V5&notes=",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(store.list().await.unwrap().is_empty());
}
