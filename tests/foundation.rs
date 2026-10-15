use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mimik::{CharacterInput, Store, app};
use tower::ServiceExt;

fn input() -> CharacterInput {
    CharacterInput {
        name: "  Elara  ".into(),
        system_id: "dnd".into(),
        edition: "Example edition".into(),
        notes: "A character record, not validated game stats.".into(),
        revision: 0,
    }
}

#[tokio::test]
async fn persists_across_reopen_and_rejects_stale_updates() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("characters.sqlite3");
    let store = Store::open(&path).unwrap();
    let created = store.create(input()).await.unwrap();
    assert_eq!(created.input.name, "Elara");
    assert_eq!(created.input.revision, 1);
    let mut edit = created.input.clone();
    edit.notes = "New notes".into();
    let saved = store.update(created.id, edit.clone()).await.unwrap();
    assert_eq!(saved.input.revision, 2);
    assert!(matches!(
        store.update(created.id, edit).await,
        Err(mimik::AppError::Conflict)
    ));
    drop(store);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(
        reopened.get(created.id).await.unwrap().input.notes,
        "New notes"
    );
    assert_eq!(reopened.list().await.unwrap().len(), 1);
}

#[tokio::test]
async fn invalid_records_and_edition_changes_do_not_modify_storage() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.sqlite3")).unwrap();
    let mut invalid = input();
    invalid.name = "   ".into();
    assert!(matches!(
        store.create(invalid).await,
        Err(mimik::AppError::Invalid(_))
    ));
    let mut unknown = input();
    unknown.system_id = "invented".into();
    assert!(matches!(
        store.create(unknown).await,
        Err(mimik::AppError::Invalid(_))
    ));
    assert!(store.list().await.unwrap().is_empty());
    let created = store.create(input()).await.unwrap();
    let mut wrong_edition = created.input.clone();
    wrong_edition.edition = "Different edition".into();
    assert!(matches!(
        store.update(created.id, wrong_edition).await,
        Err(mimik::AppError::Invalid(_))
    ));
    assert_eq!(store.get(created.id).await.unwrap().input.revision, 1);
}

fn post(uri: &str, body: &str, htmx: bool) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "127.0.0.1:3000")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("hx-request", if htmx { "true" } else { "false" })
        .body(Body::from(body.to_owned()))
        .unwrap()
}

#[tokio::test]
async fn web_flow_escapes_input_and_returns_htmx_fragment_with_fresh_revision() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.sqlite3")).unwrap();
    let router = app(store.clone());
    // Legacy records remain editable; new HTTP creation is capability-gated separately.
    store
        .create(CharacterInput {
            name: "Elara".into(),
            system_id: "dnd".into(),
            edition: "Example".into(),
            notes: "<script>bad</script>".into(),
            revision: 0,
        })
        .await
        .unwrap();
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/characters/1")
                .header("host", "127.0.0.1:3000")
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
    assert!(!html.contains("<script>bad</script>"));
    assert!(html.contains("Edition adapter pending"));
    assert!(html.contains("hx-post="));
    let response = router
        .clone()
        .oneshot(post(
            "/characters/1",
            "name=Elara&system_id=dnd&edition=Example&notes=updated&revision=1",
            true,
        ))
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
    assert!(html.contains("Saved on this device."));
    assert!(html.contains("name=\"revision\" value=\"2\""));
    assert!(!html.contains("<!doctype"));
    let response = router
        .oneshot(post(
            "/characters/1",
            "name=Elara&system_id=dnd&edition=Example&notes=unsaved+notes&revision=1",
            true,
        ))
        .await
        .unwrap();
    let html = String::from_utf8(
        to_bytes(response.into_body(), 100_000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("another tab"));
    assert!(html.contains("unsaved notes"));
    assert_eq!(store.get(1).await.unwrap().input.notes, "updated");
}

#[tokio::test]
async fn missing_characters_and_cross_site_writes_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.sqlite3")).unwrap();
    let router = app(store.clone());
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/characters/99")
                .header("host", "127.0.0.1:3000")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let mut request = post(
        "/characters",
        "name=Elara&system_id=dnd&edition=Example&notes=",
        false,
    );
    request
        .headers_mut()
        .insert("origin", "https://other.example".parse().unwrap());
    request
        .headers_mut()
        .insert("host", "127.0.0.1:3000".parse().unwrap());
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(store.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn home_and_browser_assets_are_available_locally() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.sqlite3")).unwrap();
    let router = app(store);
    for uri in ["/", "/assets/app.css", "/assets/htmx.min.js"] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header("host", "127.0.0.1:3000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            !to_bytes(response.into_body(), 100_000)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
