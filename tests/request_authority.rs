use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use mimik::{Store, app};
use tower::ServiceExt;

#[tokio::test]
async fn foreign_authority_cannot_read_or_write_even_with_matching_origin() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.sqlite3")).unwrap();
    let router = app(store.clone());
    for path in ["/", "/assets/app.css", "/characters/1"] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("host", "attacker.example:3000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/characters")
                .header("host", "attacker.example:3000")
                .header("origin", "http://attacker.example:3000")
                .header("sec-fetch-site", "same-origin")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "name=Injected&system_id=dnd&edition=Example&notes=",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(store.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn only_documented_local_authorities_are_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let router = app(Store::open(&dir.path().join("test.sqlite3")).unwrap());
    for host in ["127.0.0.1:3000", "localhost:3000"] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("host", host)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    for host in [
        None,
        Some("localhost:9999"),
        Some("localhost.attacker.example:3000"),
        Some("user@localhost:3000"),
    ] {
        let mut request = Request::builder().uri("/");
        if let Some(host) = host {
            request = request.header("host", host);
        }
        let response = router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
