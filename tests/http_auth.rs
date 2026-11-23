use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use mimik::{Store, accounts::Accounts};
use std::{
    fs::OpenOptions,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
};
use tower::ServiceExt;

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    body: String,
) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "127.0.0.1:3000");
    if method == "POST" {
        request = request
            .header(header::ORIGIN, "http://127.0.0.1:3000")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(header::ACCEPT, "text/html")
            .header("sec-fetch-mode", "navigate")
            .header("sec-fetch-dest", "document");
    }
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        response.headers()[header::X_CONTENT_TYPE_OPTIONS],
        "nosniff"
    );
    if response.status().is_client_error() || response.status().is_server_error() {
        assert!(!response.headers().contains_key(header::SET_COOKIE));
    }
    response
}
fn cookie(response: &Response) -> String {
    let value = response.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(value.contains("HttpOnly"));
    assert!(value.contains("SameSite=Strict"));
    assert!(value.contains("Path=/"));
    assert!(!value.contains("Secure"));
    assert!(!value.contains("Domain"));
    value.split(';').next().unwrap().into()
}
async fn text(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 16384)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
async fn csrf(response: Response) -> String {
    let html = text(response).await;
    html.split("name=\"csrf\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .into()
}
async fn form(app: &Router, cookie_value: Option<&str>) -> (String, String) {
    let response = request(app, "GET", "/auth/login", cookie_value, String::new()).await;
    assert_eq!(response.status(), StatusCode::OK);
    let c = cookie_value.map_or_else(|| cookie(&response), str::to_owned);
    if cookie_value.is_some() {
        assert!(!response.headers().contains_key(header::SET_COOKIE));
    }
    (c, csrf(response).await)
}
fn password_file(path: &std::path::Path) {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap()
        .write_all(b"synthetic password 123")
        .unwrap();
}
async fn login(app: &Router, c: &str, token: &str, user: &str, password: &str) -> Response {
    request(
        app,
        "POST",
        "/auth/login",
        Some(c),
        format!("login={user}&password={password}&csrf={token}"),
    )
    .await
}

#[tokio::test]
async fn actual_router_persistent_fixed_deadline_rotation_switch_epoch_disable_logout() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.path().join("auth.sqlite3");
    let pw = dir.path().join("password");
    password_file(&pw);
    let store = Store::open(&path).unwrap();
    assert!(mimik::auth::app(store.clone()).await.is_err());
    let accounts = Accounts::new(store.clone());
    let owner = accounts
        .bootstrap("owner".into(), "Owner <escaped>".into(), &pw)
        .await
        .unwrap();
    let invite = dir.path().join("invite");
    accounts
        .invite("player".into(), "Player".into(), &invite)
        .await
        .unwrap();
    let player = accounts.enroll(&invite, &pw).await.unwrap();
    let app = mimik::auth::app(store.clone()).await.unwrap();
    let owner_cookie = readonly_and_second_save(app.clone(), &path).await;
    let player_cookie = rotate_and_switch(app.clone(), owner_cookie).await;
    drop(app);
    drop(store);
    let reopened = Store::open(&path).unwrap();
    let app = mimik::auth::app(reopened.clone()).await.unwrap();
    check_logout(&app, &player_cookie).await;
    check_epoch_revocation(&app, &accounts, &player, &owner, dir.path(), &pw, &reopened).await;
    accounts.disable(owner.id, owner.epoch).await.unwrap();
    assert!(
        mimik::auth::app(reopened.clone()).await.is_ok(),
        "a disabled Owner still retains the durable startup binding"
    );
    assert!(
        mimik::app(reopened)
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::HOST, "127.0.0.1:3000")
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .unwrap()
            .status()
            .is_success()
    );
}

async fn readonly_and_second_save(app: Router, path: &std::path::Path) -> String {
    let (anon, anon_csrf) = form(&app, None).await;
    let db = rusqlite::Connection::open(path).unwrap();
    let deadline: i64 = db
        .query_row(
            "SELECT expires_at FROM auth_sessions WHERE id=?1",
            [anon.trim_start_matches("mimik_session=")],
            |r| r.get(0),
        )
        .unwrap();
    for _ in 0..3 {
        assert_eq!(form(&app, Some(&anon)).await.1, anon_csrf);
    }
    for user in ["unknown", "owner"] {
        let response = login(&app, &anon, &anon_csrf, user, "incorrect+password+123").await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let html = text(response).await;
        assert!(html.contains("role=\"alert\""));
        assert!(html.contains("Check your login name and password"));
        assert!(!html.contains("incorrect password 123"));
    }
    assert_eq!(
        db.query_row(
            "SELECT expires_at FROM auth_sessions WHERE id=?1",
            [anon.trim_start_matches("mimik_session=")],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        deadline
    );
    // Explicit create commits; the library's response-side second save fails.
    db.execute_batch("CREATE TRIGGER injected_second_save BEFORE UPDATE ON auth_sessions WHEN NEW.state='active' AND NEW.account_id IS NOT NULL BEGIN SELECT RAISE(ABORT,'synthetic save failure'); END;").unwrap();
    let failed = login(&app, &anon, &anon_csrf, "owner", "synthetic+password+123").await;
    assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let html = text(failed).await;
    assert!(html.contains("Request status: 500"));
    assert!(html.contains("role=\"alert\""));
    assert!(!html.contains("synthetic password 123"));
    assert_eq!(
        db.query_row(
            "SELECT state FROM auth_sessions WHERE id=?1",
            [anon.trim_start_matches("mimik_session=")],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "revoked"
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM auth_sessions WHERE state='active' AND account_id IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    db.execute_batch("DROP TRIGGER injected_second_save")
        .unwrap();
    let (anon, anon_csrf) = form(&app, None).await;
    let response = login(&app, &anon, &anon_csrf, "owner", "synthetic+password+123").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let owner_cookie = cookie(&response);
    assert_ne!(anon, owner_cookie);
    assert_eq!(
        request(&app, "GET", "/auth/session", Some(&anon), String::new())
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let response = request(&app, "GET", "/", Some(&owner_cookie), String::new()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    let html = text(response).await;
    assert!(html.contains("Owner &#60;escaped&#62;") || html.contains("Owner &lt;escaped&gt;"));
    assert!(!html.contains("synthetic password"));
    owner_cookie
}

async fn rotate_and_switch(app: Router, owner_cookie: String) -> String {
    let owner_csrf = form(&app, Some(&owner_cookie)).await.1;
    for path in [
        "/characters",
        "/characters/1",
        "/sources",
        "/sources/1/pages/1",
        "/history",
        "/assets/htmx.min.js",
    ] {
        assert_eq!(
            request(&app, "GET", path, Some(&owner_cookie), String::new())
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let response = login(
        &app,
        &owner_cookie,
        &owner_csrf,
        "owner",
        "synthetic+password+123",
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let rotated = cookie(&response);
    assert_ne!(rotated, owner_cookie);
    assert_eq!(
        request(
            &app,
            "GET",
            "/auth/session",
            Some(&owner_cookie),
            String::new()
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let token = form(&app, Some(&rotated)).await.1;
    let response = login(&app, &rotated, &token, "player", "synthetic+password+123").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let player_cookie = cookie(&response);
    assert_ne!(player_cookie, rotated);
    player_cookie
}

async fn check_logout(app: &Router, player_cookie: &str) {
    let response = request(app, "GET", "/", Some(player_cookie), String::new()).await;
    assert_eq!(response.status(), StatusCode::OK);
    let token = csrf(response).await;
    assert_eq!(
        request(
            app,
            "POST",
            "/auth/logout",
            Some(player_cookie),
            format!("csrf={token}")
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        request(
            app,
            "GET",
            "/auth/session",
            Some(player_cookie),
            String::new()
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
}

async fn check_epoch_revocation(
    app: &Router,
    accounts: &Accounts,
    player: &mimik::accounts::Summary,
    owner: &mimik::accounts::Summary,
    dir: &std::path::Path,
    pw: &std::path::Path,
    store: &Store,
) {
    let (anon, token) = form(app, None).await;
    let response = login(app, &anon, &token, "player", "synthetic+password+123").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let c = cookie(&response);
    let reset = dir.join("reset");
    let issued = accounts
        .issue_reset(player.id, player.epoch, &reset)
        .await
        .unwrap();
    assert_eq!(
        request(app, "GET", "/auth/session", Some(&c), String::new())
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let during_reset = seed_authenticated(store, issued.id, issued.epoch).await;
    assert_eq!(
        request(app, "GET", "/", Some(&during_reset), String::new())
            .await
            .status(),
        StatusCode::OK
    );
    let changed = accounts.complete_reset(&reset, pw).await.unwrap();
    assert_eq!(
        request(app, "GET", "/auth/session", Some(&c), String::new())
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            app,
            "GET",
            "/auth/session",
            Some(&during_reset),
            String::new()
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let before_disable = seed_authenticated(store, changed.id, changed.epoch).await;
    assert_eq!(
        request(app, "GET", "/", Some(&before_disable), String::new())
            .await
            .status(),
        StatusCode::OK
    );
    accounts.disable(changed.id, changed.epoch).await.unwrap();
    assert_eq!(
        request(
            app,
            "GET",
            "/auth/session",
            Some(&before_disable),
            String::new()
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(
        accounts
            .authenticate(
                "player".into(),
                zeroize::Zeroizing::new(b"synthetic password 123".to_vec())
            )
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        accounts
            .current(owner.id, owner.epoch)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        accounts
            .current(player.id, player.epoch)
            .await
            .unwrap()
            .is_none()
    );
}

async fn seed_authenticated(store: &Store, id: i64, epoch: i64) -> String {
    use tower_sessions::SessionStore;
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let mut record = tower_sessions::session::Record {
        id: tower_sessions::session::Id::default(),
        expiry_date: time::OffsetDateTime::from_unix_timestamp(now + 28800).unwrap(),
        data: std::collections::HashMap::from([
            ("csrf".into(), serde_json::json!("a".repeat(64))),
            (
                "principal".into(),
                serde_json::json!({"id":id,"epoch":epoch}),
            ),
        ]),
    };
    mimik::auth::SqliteSessions::new(store.clone())
        .create(&mut record)
        .await
        .unwrap();
    format!("mimik_session={}", record.id)
}
