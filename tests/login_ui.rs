use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use mimik::{Store, accounts::Accounts, auth};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::PermissionsExt,
    sync::LazyLock,
};
use tower::ServiceExt;

static LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));
const PASSWORD: &str = " exact synthetic password 2026! ";

async fn fixture() -> (tempfile::TempDir, Router) {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let password = directory.path().join("password");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&password)
        .unwrap();
    fs::set_permissions(&password, fs::Permissions::from_mode(0o600)).unwrap();
    file.write_all(PASSWORD.as_bytes()).unwrap();
    let store = Store::open(&directory.path().join("synthetic.sqlite3")).unwrap();
    Accounts::new(store.clone())
        .bootstrap("owner".into(), "Owner".into(), &password)
        .await
        .unwrap();
    let app = auth::app_with_test_sessions(store.clone(), auth::SqliteSessions::new(store))
        .await
        .unwrap();
    (directory, app)
}
async fn call(
    app: &Router,
    cookie: Option<&str>,
    login: Option<(&str, &str, &str)>,
    document: bool,
) -> Response {
    let mut request = Request::builder()
        .uri("/auth/login")
        .header("host", "127.0.0.1:3000");
    if document {
        request = request
            .header("accept", "text/html")
            .header("sec-fetch-mode", "navigate")
            .header("sec-fetch-dest", "document");
    } else {
        request = request.header("accept", "application/json");
    }
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    let body = if let Some((csrf, login, password)) = login {
        request = request
            .method("POST")
            .header("origin", "http://127.0.0.1:3000")
            .header("content-type", "application/x-www-form-urlencoded");
        serde_urlencoded::to_string([("csrf", csrf), ("login", login), ("password", password)])
            .unwrap()
    } else {
        String::new()
    };
    app.clone()
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap()
}
async fn body(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 32768)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
async fn form(app: &Router) -> (String, String) {
    let response = call(app, None, None, true).await;
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let html = body(response).await;
    let csrf = html
        .split("name=\"csrf\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned();
    (cookie, csrf)
}

#[tokio::test]
async fn native_wrong_then_correct_login_keeps_the_form_without_password_or_cookie_leak() {
    let _lock = LOCK.lock().await;
    let (_directory, app) = fixture().await;
    let (cookie, csrf) = form(&app).await;
    let response = call(
        &app,
        Some(&cookie),
        Some((&csrf, "owner", "wrong synthetic password")),
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    let html = body(response).await;
    assert!(html.contains("role=\"alert\""));
    assert!(html.contains("action=\"/auth/login\""));
    assert!(html.contains("value=\"owner\""));
    assert!(!html.contains("wrong synthetic password"));
    assert!(!html.contains(PASSWORD));
    assert_eq!(
        call(&app, Some(&cookie), Some((&csrf, "owner", PASSWORD)), true)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
}
#[tokio::test]
async fn browser_identity_is_case_and_whitespace_friendly_but_password_bytes_are_exact() {
    let _lock = LOCK.lock().await;
    let (_directory, app) = fixture().await;
    let (cookie, csrf) = form(&app).await;
    assert_eq!(
        call(
            &app,
            Some(&cookie),
            Some((&csrf, " OWNER ", PASSWORD)),
            true
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let (cookie, csrf) = form(&app).await;
    let response = call(
        &app,
        Some(&cookie),
        Some((&csrf, "owner", PASSWORD.trim())),
        false,
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(body(response).await, "Invalid credentials or session.");
}

#[tokio::test]
async fn retained_identity_is_bounded_escaped_and_never_truncated_for_authentication() {
    let _lock = LOCK.lock().await;
    let (directory, app) = fixture().await;
    let accounts = Accounts::new(Store::open(&directory.path().join("synthetic.sqlite3")).unwrap());
    let token = directory.path().join("invite");
    let long = "a".repeat(64);
    accounts
        .invite(long.clone(), "Long login".into(), &token)
        .await
        .unwrap();
    accounts
        .enroll(&token, &directory.path().join("password"))
        .await
        .unwrap();
    let (cookie, csrf) = form(&app).await;
    for login in ["a".repeat(65), "<script>अ&\"".repeat(20)] {
        let response = call(&app, Some(&cookie), Some((&csrf, &login, PASSWORD)), true).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let html = body(response).await;
        assert!(!html.contains("<script>अ"));
        assert!(html.contains("role=\"alert\""));
        assert!(!html.contains(PASSWORD));
        let retained = html
            .split("name=\"login\" value=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        assert!(retained.len() <= 64 * 8);
    }
    assert_eq!(
        call(&app, Some(&cookie), Some((&csrf, &long, PASSWORD)), true)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
}

#[tokio::test]
async fn early_document_rejections_are_styled_and_explicit_fetch_errors_remain_plain() {
    let _lock = LOCK.lock().await;
    let (_directory, app) = fixture().await;
    for (origin, content, payload, status) in [
        (
            "http://localhost:3000",
            "application/x-www-form-urlencoded",
            "login=owner",
            StatusCode::FORBIDDEN,
        ),
        (
            "http://127.0.0.1:3000",
            "application/json",
            "{}",
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            "http://127.0.0.1:3000",
            "application/x-www-form-urlencoded",
            "login=owner&password=not-echoed&csrf=x&unknown=x",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let request = Request::builder()
            .method("POST")
            .uri("/auth/login")
            .header("host", "127.0.0.1:3000")
            .header("origin", origin)
            .header("content-type", content)
            .header("accept", "text/html")
            .header("sec-fetch-mode", "navigate")
            .header("sec-fetch-dest", "document")
            .body(Body::from(payload))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), status);
        assert!(!response.headers().contains_key(header::SET_COOKIE));
        let html = body(response).await;
        assert!(html.contains("role=\"alert\""));
        assert!(html.contains("Mimik home"));
        assert!(!html.contains("not-echoed"));
    }
    let request = Request::builder()
        .method("POST")
        .uri("/auth/login")
        .header("host", "127.0.0.1:3000")
        .header("origin", "http://localhost:3000")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "text/html")
        .header("sec-fetch-mode", "cors")
        .header("sec-fetch-dest", "empty")
        .body(Body::from("login=owner"))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(body(response).await, "Access denied.");
}
