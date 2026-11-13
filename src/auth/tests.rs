use super::*;
use crate::store::register_auth_clock;
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use rusqlite::{Connection, params};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicI64, AtomicUsize},
};
use tower::ServiceExt;
use tower_sessions::{
    SessionStore,
    session::{Id, Record},
};
#[path = "error_tests.rs"]
mod error_tests;
#[cfg(target_os = "linux")]
#[path = "upload_route_tests.rs"]
mod upload_route_tests;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn login_rechecks_original_form_admission_after_password_verification() {
    let (_dir, store, sessions, clock) = fixture().await;
    let now = clock.load(Ordering::SeqCst);
    let mut anonymous = record(now);
    sessions.create(&mut anonymous).await.unwrap();
    let c = clock.clone();
    let mut fixture_state = state(store);
    fixture_state.accounts = fixture_state.accounts.with_auth_test_hooks(
        Arc::new(move || c.store(now + 600, Ordering::SeqCst)),
        Arc::new(|| {}),
    );
    let app = router(fixture_state);
    let body = serde_urlencoded::to_string([
        ("csrf", "a".repeat(64)),
        ("login", "unknown-player".into()),
        ("password", "abcdefghijkl".into()),
    ])
    .unwrap();
    let request = Request::builder()
        .method("POST")
        .uri("/auth/login")
        .header("host", "127.0.0.1:3000")
        .header("origin", "http://127.0.0.1:3000")
        .header("accept", "text/html")
        .header("sec-fetch-mode", "navigate")
        .header("sec-fetch-dest", "document")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("cookie", format!("mimik_session={}", anonymous.id))
        .body(Body::from(body))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    let html = String::from_utf8(
        to_bytes(response.into_body(), 16384)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("fresh sign-in form"));
    assert!(!html.contains("abcdefghijkl"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn verified_password_cannot_rotate_an_expired_or_revoked_login_form() {
    use std::{io::Write, os::unix::fs::PermissionsExt};
    for revoke in [false, true] {
        let (directory, store, sessions, clock) = fixture().await;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.path().join("password");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        file.write_all(b"synthetic owner password 2026").unwrap();
        let now = clock.load(Ordering::SeqCst);
        let accounts = Accounts::new(store.clone()).with_test_clock(Arc::new(move || Ok(now)));
        accounts
            .bootstrap("owner".into(), "Owner".into(), &path)
            .await
            .unwrap();
        let mut anonymous = record(now);
        sessions.create(&mut anonymous).await.unwrap();
        let database = directory.path().join("db");
        let id = anonymous.id.to_string();
        let c = clock.clone();
        let mut fixture_state = state(store.clone());
        fixture_state.accounts = accounts.with_auth_test_hooks(
            Arc::new(move || {
                if revoke {
                    let db = Connection::open(&database).unwrap();
                    register_auth_clock(&db, Arc::new(move || Ok(now))).unwrap();
                    db.execute(
                        "UPDATE auth_sessions SET state='revoked',payload=NULL WHERE id=?1",
                        [&id],
                    )
                    .unwrap();
                } else {
                    c.store(now + 600, Ordering::SeqCst);
                }
            }),
            Arc::new(|| {}),
        );
        let body = serde_urlencoded::to_string([
            ("csrf", "a".repeat(64)),
            ("login", "owner".into()),
            ("password", "synthetic owner password 2026".into()),
        ])
        .unwrap();
        let request = Request::builder()
            .method("POST")
            .uri("/auth/login")
            .header("host", "127.0.0.1:3000")
            .header("origin", "http://127.0.0.1:3000")
            .header("accept", "text/html")
            .header("sec-fetch-mode", "navigate")
            .header("sec-fetch-dest", "document")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("cookie", format!("mimik_session={}", anonymous.id))
            .body(Body::from(body))
            .unwrap();
        let response = router(fixture_state).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(!response.headers().contains_key(header::SET_COOKIE));
        store
            .run(|db| {
                assert_eq!(
                    db.query_row("SELECT count(*) FROM auth_sessions", [], |r| r
                        .get::<_, i64>(0))?,
                    1
                );
                assert_eq!(
                    db.query_row(
                        "SELECT count(*) FROM auth_sessions WHERE account_id IS NOT NULL",
                        [],
                        |r| r.get::<_, i64>(0)
                    )?,
                    0
                );
                Ok(())
            })
            .await
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_redemption_admission_failure_offers_fresh_get_and_preserves_credential() {
    let (_dir, store, sessions, clock) = fixture().await;
    store.run(|db|{db.execute("INSERT INTO accounts(id,login,display_name,state,epoch,verifier) VALUES(1,'owner','Owner','enabled',1,?1)",["$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"])?;db.execute("INSERT INTO account_owner VALUES(1,1)",[])?;Ok(())}).await.unwrap();
    let now = clock.load(Ordering::SeqCst);
    let mut owner = record(now);
    owner
        .data
        .insert("principal".into(), serde_json::json!({"id":1,"epoch":1}));
    sessions.create(&mut owner).await.unwrap();
    let auth = RequestAuth::fixture(1, owner.id, now + 600);
    let issued = Accounts::new(store.clone())
        .browser_invite(auth, "late-player".into(), "Late player".into(), 0)
        .await
        .unwrap();
    let mut anonymous = record(now);
    sessions.create(&mut anonymous).await.unwrap();
    let c = clock.clone();
    let mut fixture_state = state(store.clone());
    fixture_state.accounts = fixture_state.accounts.with_auth_test_hooks(
        Arc::new(move || c.store(now + 600, Ordering::SeqCst)),
        Arc::new(|| {}),
    );
    let app = router(fixture_state);
    let post = |id, csrf: &str| redemption_post(id, csrf, &issued.token);
    let response = app
        .clone()
        .oneshot(post(anonymous.id, &"a".repeat(64)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    let html =
        String::from_utf8(to_bytes(response.into_body(), 8192).await.unwrap().to_vec()).unwrap();
    assert!(html.contains("?fresh=1#token="));
    assert!(html.contains("form session expired"));
    assert!(!html.contains("abcdefghijkl"));
    store
        .run(|db| {
            let pending = db.query_row(
                "SELECT state FROM accounts WHERE login='late-player'",
                [],
                |r| r.get::<_, String>(0),
            )?;
            let active = db.query_row(
                "SELECT count(*) FROM account_credentials WHERE state='active'",
                [],
                |r| r.get::<_, i64>(0),
            )?;
            assert_eq!(pending, "pending");
            assert_eq!(active, 1);
            Ok(())
        })
        .await
        .unwrap();
    let response = app
        .clone()
        .oneshot(request("/auth/enroll?fresh=1", Some(anonymous.id)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .split('=')
        .nth(1)
        .unwrap()
        .parse::<Id>()
        .unwrap();
    let html =
        String::from_utf8(to_bytes(response.into_body(), 8192).await.unwrap().to_vec()).unwrap();
    let csrf = html
        .split("name=\"csrf\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    assert_ne!(cookie, anonymous.id);
    assert_eq!(
        app.oneshot(post(cookie, csrf)).await.unwrap().status(),
        StatusCode::SEE_OTHER
    );
}
fn redemption_post(id: Id, csrf: &str, token: &[u8]) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/auth/enroll")
        .header("host", "127.0.0.1:3000")
        .header("origin", "http://127.0.0.1:3000")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("cookie", format!("mimik_session={id}"))
        .body(Body::from(
            serde_urlencoded::to_string([
                ("csrf", csrf),
                ("token", std::str::from_utf8(token).unwrap()),
                ("password", "abcdefghijkl"),
                ("confirmation", "abcdefghijkl"),
            ])
            .unwrap(),
        ))
        .unwrap()
}

pub(super) static AUTH_TEST_LOCK: LazyLock<Arc<tokio::sync::Mutex<()>>> =
    LazyLock::new(|| Arc::new(tokio::sync::Mutex::new(())));

#[tokio::test]
async fn regression_b1_row_query_wait_expires_at_equality_and_after() {
    let (_dir, _store, sessions, clock) = fixture().await;
    for offset in [0, 1] {
        let mut r = record(clock.load(Ordering::SeqCst));
        sessions.create(&mut r).await.unwrap();
        let deadline = r.expiry_date.unix_timestamp();
        let c = clock.clone();
        let reader = sessions.clone().with_read_test_hook(Arc::new(move || {
            c.store(deadline + offset, Ordering::SeqCst);
        }));
        assert!(reader.load(&r.id).await.unwrap().is_none());
    }
}

#[tokio::test]
async fn regression_b2_present_null_is_corruption_in_real_router() {
    let (_dir, store, sessions, clock) = fixture().await;
    let mut r = record(clock.load(Ordering::SeqCst));
    sessions.create(&mut r).await.unwrap();
    let app = router(state(store.clone()));
    assert_eq!(
        app.clone()
            .oneshot(request("/auth/login", Some(r.id)))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let csrf = "a".repeat(64);
    for payload in [
        serde_json::json!({"csrf":csrf,"principal":null}).to_string(),
        format!(r#"{{"csrf":"{csrf}","csrf":"{csrf}"}}"#),
        serde_json::json!({"csrf":csrf,"extra":1}).to_string(),
        format!(r#"{{"csrf":"{csrf}","principal":{{"id":1,"id":1,"epoch":1}}}}"#),
        serde_json::json!({"csrf":csrf,"principal":{"id":1,"epoch":1,"extra":1}}).to_string(),
    ] {
        damage_fixture_payload(&store, r.id, payload).await;
        assert_error(
            app.clone()
                .oneshot(request("/auth/login", Some(r.id)))
                .await
                .unwrap(),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .await;
        assert!(sessions.load(&r.id).await.is_err());
    }
    let mut normal = record(clock.load(Ordering::SeqCst));
    sessions.create(&mut normal).await.unwrap();
    sessions.save(&normal).await.unwrap();
    assert_eq!(
        sessions.load(&normal.id).await.unwrap(),
        Some(normal.clone())
    );
    let id = normal.id.to_string();
    assert!(
        store
            .run(move |db| {
                db.execute(
                    "UPDATE auth_sessions SET payload=?1 WHERE id=?2",
                    params![
                        serde_json::json!({"csrf":"b".repeat(64),"principal":null}).to_string(),
                        id
                    ],
                )?;
                Ok(())
            })
            .await
            .is_err(),
        "normal production guards must be restored"
    );
}

async fn damage_fixture_payload(store: &Store, id: Id, payload: String) {
    store.run(move |db| {
        // Only disposable damaged-row fixtures bypass admission. Restore the
        // exact production trigger and checks before returning to the router.
        let trigger: String = db.query_row("SELECT sql FROM sqlite_master WHERE type='trigger' AND name='auth_sessions_update'", [], |row| row.get(0))?;
        db.execute_batch("DROP TRIGGER auth_sessions_update; PRAGMA ignore_check_constraints=ON;")?;
        db.execute("UPDATE auth_sessions SET payload=?1 WHERE id=?2", params![payload,id.to_string()])?;
        db.execute_batch("PRAGMA ignore_check_constraints=OFF;")?;
        db.execute_batch(&trigger)?;
        Ok(())
    }).await.unwrap();
}

#[tokio::test]
async fn regression_b1_account_read_wait_cannot_return_expired_principal() {
    let (_dir, store, sessions, clock) = fixture().await;
    store.run(|db| {
        db.execute("INSERT INTO accounts(login,display_name,state,epoch,verifier) VALUES('owner','Owner','enabled',1,?1)",["$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"])?;
        db.execute("INSERT INTO account_owner VALUES(1,1)",[])?;
        Ok(())
    }).await.unwrap();
    for offset in [0, 1] {
        let mut r = record(clock.load(Ordering::SeqCst));
        r.data
            .insert("principal".into(), serde_json::json!({"id":1,"epoch":1}));
        sessions.create(&mut r).await.unwrap();
        let mut auth = state(store.clone());
        let deadline = r.expiry_date.unix_timestamp();
        let c = clock.clone();
        // Simulate time consumed by the account query after session load. This
        // hook is inside identity reconstruction, not inside session load.
        auth.before_current = Some(Arc::new(move || {
            c.store(deadline + offset, Ordering::SeqCst);
        }));
        assert_error(
            router(auth)
                .oneshot(request("/auth/session", Some(r.id)))
                .await
                .unwrap(),
            StatusCode::UNAUTHORIZED,
        )
        .await;
    }
}

#[tokio::test]
async fn regression_b1_final_admission_cannot_replace_captured_deadline() {
    let (dir, store, sessions, clock) = fixture().await;
    store.run(|db| {
        db.execute("INSERT INTO accounts(login,display_name,state,epoch,verifier) VALUES('owner','Owner','enabled',1,?1)",["$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"])?;
        db.execute("INSERT INTO account_owner VALUES(1,1)",[])?;
        Ok(())
    }).await.unwrap();
    let mut r = record(clock.load(Ordering::SeqCst));
    r.data
        .insert("principal".into(), serde_json::json!({"id":1,"epoch":1}));
    sessions.create(&mut r).await.unwrap();
    let deadline = r.expiry_date.unix_timestamp();
    let id = r.id.to_string();
    let path = dir.path().join("db");
    let mut auth = state(store);
    auth.before_current = Some(Arc::new(move || {
        clock.store(deadline, Ordering::SeqCst);
        let raw = Connection::open(&path).unwrap();
        let c = clock.clone();
        register_auth_clock(&raw, Arc::new(move || Ok(c.load(Ordering::SeqCst)))).unwrap();
        raw.execute("DELETE FROM auth_sessions WHERE id=?1", [&id])
            .unwrap();
        raw.execute(
            "INSERT INTO auth_sessions VALUES(?1,?2,?3,'active',?4,1,1)",
            params![
                id,
                deadline,
                deadline + 600,
                serde_json::json!({"csrf":"a".repeat(64),"principal":{"id":1,"epoch":1}})
                    .to_string()
            ],
        )
        .unwrap();
    }));
    assert_error(
        router(auth)
            .oneshot(request("/auth/session", Some(r.id)))
            .await
            .unwrap(),
        StatusCode::UNAUTHORIZED,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn regression_b3_cleanup_recreate_rejects_captured_expired_deadline() {
    let (_dir, _store, sessions, clock) = fixture().await;
    let mut old = record(clock.load(Ordering::SeqCst));
    sessions.create(&mut old).await.unwrap();
    let captured = sessions.load(&old.id).await.unwrap().unwrap();
    let (entered_tx, entered) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let writer = sessions.clone();
    let task = tokio::spawn(async move {
        entered_tx.send(()).unwrap();
        released.await.unwrap();
        writer.save(&captured).await
    });
    entered.await.unwrap();
    clock.store(old.expiry_date.unix_timestamp(), Ordering::SeqCst);
    assert_eq!(sessions.cleanup().await.unwrap(), 1);
    let mut recreated = record(clock.load(Ordering::SeqCst));
    recreated.id = old.id;
    sessions.create(&mut recreated).await.unwrap();
    release.send(()).unwrap();
    assert!(task.await.unwrap().is_err());
    assert_eq!(sessions.load(&old.id).await.unwrap(), Some(recreated));
    assert!(sessions.save(&old).await.is_err());
}
struct TestDirectory {
    directory: tempfile::TempDir,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}
impl std::ops::Deref for TestDirectory {
    type Target = tempfile::TempDir;
    fn deref(&self) -> &Self::Target {
        &self.directory
    }
}

async fn fixture() -> (TestDirectory, Store, SqliteSessions, Arc<AtomicI64>) {
    let guard = AUTH_TEST_LOCK.clone().lock_owned().await;
    let dir = TestDirectory {
        directory: tempfile::tempdir().unwrap(),
        _guard: guard,
    };
    let store = Store::open(&dir.path().join("db")).unwrap();
    let clock = Arc::new(AtomicI64::new(1_000_000));
    let c = clock.clone();
    store
        .run(move |db| register_auth_clock(db, Arc::new(move || Ok(c.load(Ordering::SeqCst)))))
        .await
        .unwrap();
    let sessions = SqliteSessions::new(store.clone());
    (dir, store, sessions, clock)
}
fn record(now: i64) -> Record {
    Record {
        id: Id::default(),
        expiry_date: time::OffsetDateTime::from_unix_timestamp(now + 600).unwrap(),
        data: HashMap::from([("csrf".into(), serde_json::json!("a".repeat(64)))]),
    }
}
fn state(store: Store) -> AuthState {
    AuthState {
        accounts: Accounts::new(store.clone()),
        sessions: SqliteSessions::new(store),
        quota: Arc::new(RateLimiter::direct(Quota::per_second(
            NonZeroU32::new(1000).unwrap(),
        ))),
        options: AuthOptions::default(),
        before_current: None,
    }
}
fn request(path: &str, cookie: Option<Id>) -> Request<Body> {
    let mut request = Request::builder()
        .uri(path)
        .header(header::HOST, "127.0.0.1:3000");
    if let Some(id) = cookie {
        request = request.header(header::COOKIE, format!("mimik_session={id}"));
    }
    request.body(Body::empty()).unwrap()
}

fn navigation(path: &str, cookie: Option<Id>) -> Request<Body> {
    let mut request = request(path, cookie);
    request.headers_mut().insert(
        header::ACCEPT,
        "text/html,application/xhtml+xml,*/*;q=0.8".parse().unwrap(),
    );
    request
        .headers_mut()
        .insert("sec-fetch-mode", "navigate".parse().unwrap());
    request
        .headers_mut()
        .insert("sec-fetch-dest", "document".parse().unwrap());
    request
}

#[test]
fn browser_media_selection_is_explicit_and_metadata_only_selects_presentation() {
    for accept in [
        "text/html",
        "text/html;q=0.5",
        "text/html;q=1.000",
        "text/html;charset=\"UTF-8\";q=1",
        "text/html;note=\";q=1\";q=0.5",
        "text/html,application/xml;q=0.1,*/*;q=0.8",
    ] {
        let mut query = request("/auth/session", None);
        query
            .headers_mut()
            .insert(header::ACCEPT, accept.parse().unwrap());
        assert!(browser_auth_navigation(&query));
    }
    for accept in [
        "*/*",
        "text/html;q=0.000",
        "text/html;q=0.0001",
        "text/html,text/html;q=0",
        "text/html,*/*;q=invalid",
        "text/html;note=\"broken",
    ] {
        let mut query = navigation("/auth/session", None);
        query
            .headers_mut()
            .insert(header::ACCEPT, accept.parse().unwrap());
        assert!(!browser_auth_navigation(&query));
    }
    let mut query = navigation("/auth/session", None);
    query
        .headers_mut()
        .append("sec-fetch-mode", "cors".parse().unwrap());
    assert!(!browser_auth_navigation(&query));
    query.headers_mut().remove("sec-fetch-mode");
    assert!(!browser_auth_navigation(&query));
}

async fn assert_login_redirect(response: Response) {
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/auth/login");
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert!(
        to_bytes(response.into_body(), 8192)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn regression_anonymous_root_and_browser_bookmarks_reach_sign_in() {
    let (_dir, store, _sessions, _clock) = fixture().await;
    let app = router(state(store));
    assert_login_redirect(app.clone().oneshot(request("/", None)).await.unwrap()).await;
    for path in [
        "/auth/session",
        "/campaigns",
        "/private-sheets/1",
        "/private-sheets/1/history",
    ] {
        assert_login_redirect(app.clone().oneshot(navigation(path, None)).await.unwrap()).await;
    }
    let form = app.oneshot(navigation("/auth/login", None)).await.unwrap();
    assert_eq!(form.status(), StatusCode::OK);
    let body = to_bytes(form.into_body(), 8192).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains("name=\"csrf\""));
}

#[tokio::test]
async fn regression_browser_redirect_does_not_change_api_fetch_or_denial_semantics() {
    let (_dir, store, _sessions, _clock) = fixture().await;
    let app = router(state(store));
    for accept in [
        "application/json",
        "*/*",
        "text/html;q=0",
        "text/html;q=bogus",
        "text/not-html",
        "text/html;q=1.1",
        "text/html;q=0.5;q=1",
        "text/html;q=\"1\"",
        "text/html;q=\"0.5\"",
        "text/html+xml",
    ] {
        let mut query = navigation("/auth/session", None);
        query
            .headers_mut()
            .insert(header::ACCEPT, accept.parse().unwrap());
        assert_error(
            app.clone().oneshot(query).await.unwrap(),
            StatusCode::UNAUTHORIZED,
        )
        .await;
    }
    for (mode, destination) in [
        ("cors", "empty"),
        ("same-origin", "empty"),
        ("navigate", "iframe"),
    ] {
        let mut query = navigation("/auth/session", None);
        query
            .headers_mut()
            .insert("sec-fetch-mode", mode.parse().unwrap());
        query
            .headers_mut()
            .insert("sec-fetch-dest", destination.parse().unwrap());
        assert_error(
            app.clone().oneshot(query).await.unwrap(),
            StatusCode::UNAUTHORIZED,
        )
        .await;
    }
    assert_error_document(
        app.clone()
            .oneshot(navigation("/sources", None))
            .await
            .unwrap(),
        StatusCode::FORBIDDEN,
    )
    .await;
    assert_error(
        app.clone()
            .oneshot(request("/sources", None))
            .await
            .unwrap(),
        StatusCode::FORBIDDEN,
    )
    .await;
    assert_error(
        app.clone()
            .oneshot(request("/auth/session", None))
            .await
            .unwrap(),
        StatusCode::UNAUTHORIZED,
    )
    .await;
}

#[tokio::test]
async fn browser_entry_preserves_valid_sessions_and_errors_but_handles_expiry() {
    let (_dir, store, sessions, clock) = fixture().await;
    store.run(|db| {
        db.execute("INSERT INTO accounts(id,login,display_name,state,epoch,verifier) VALUES(1,'owner','Owner','enabled',1,?1)", ["$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"])?;
        Ok(())
    }).await.unwrap();
    let mut authenticated = record(clock.load(Ordering::SeqCst));
    authenticated
        .data
        .insert("principal".into(), serde_json::json!({"id":1,"epoch":1}));
    sessions.create(&mut authenticated).await.unwrap();
    let app = router(state(store.clone()));
    let root = app
        .clone()
        .oneshot(navigation("/", Some(authenticated.id)))
        .await
        .unwrap();
    assert_eq!(
        root.status(),
        StatusCode::OK,
        "a signed-in `/` is the home page"
    );
    assert_eq!(
        app.clone()
            .oneshot(navigation("/campaigns", Some(authenticated.id)))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_error_document(
        app.clone()
            .oneshot(navigation("/private-sheets/1", Some(authenticated.id)))
            .await
            .unwrap(),
        StatusCode::FORBIDDEN,
    )
    .await;
    assert_error(
        app.clone()
            .oneshot(request("/private-sheets/1", Some(authenticated.id)))
            .await
            .unwrap(),
        StatusCode::FORBIDDEN,
    )
    .await;
    clock.store(authenticated.expiry_date.unix_timestamp(), Ordering::SeqCst);
    assert_login_redirect(
        app.clone()
            .oneshot(navigation("/auth/session", Some(authenticated.id)))
            .await
            .unwrap(),
    )
    .await;
    assert_login_redirect(
        app.clone()
            .oneshot(navigation("/", Some(authenticated.id)))
            .await
            .unwrap(),
    )
    .await;
    let form = app
        .clone()
        .oneshot(navigation("/auth/login", Some(authenticated.id)))
        .await
        .unwrap();
    assert_eq!(form.status(), StatusCode::OK);
    assert!(
        to_bytes(form.into_body(), 8192)
            .await
            .unwrap()
            .starts_with(b"<!doctype html>")
    );
    let mut corrupted = record(clock.load(Ordering::SeqCst));
    sessions.create(&mut corrupted).await.unwrap();
    damage_fixture_payload(
        &store,
        corrupted.id,
        serde_json::json!({"csrf":"a".repeat(64),"principal":null}).to_string(),
    )
    .await;
    assert_error_document(
        app.clone()
            .oneshot(navigation("/auth/session", Some(corrupted.id)))
            .await
            .unwrap(),
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .await;
    assert_error(
        app.oneshot(request("/auth/session", Some(corrupted.id)))
            .await
            .unwrap(),
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .await;
}
async fn assert_error_document(response: Response, code: StatusCode) {
    assert_eq!(response.status(), code);
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        response.headers()[header::X_CONTENT_TYPE_OPTIONS],
        "nosniff"
    );
    assert_eq!(response.headers()[header::X_FRAME_OPTIONS], "DENY");
    assert_eq!(
        response.headers()[header::CONTENT_SECURITY_POLICY],
        "frame-ancestors 'none'"
    );
    assert!(
        response.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    let html = String::from_utf8(
        to_bytes(response.into_body(), 16384)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("role=\"alert\""));
    assert!(html.contains("Mimik home"));
    assert!(html.contains(&format!("Request status: {}", code.as_u16())));
    assert!(!html.contains("<script>"));
    assert!(!html.contains("principal"));
    assert!(!html.contains("a".repeat(64).as_str()));
}
async fn assert_error(response: Response, code: StatusCode) {
    assert_eq!(response.status(), code);
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        response.headers()[header::X_CONTENT_TYPE_OPTIONS],
        "nosniff"
    );
    let body = to_bytes(response.into_body(), 8192).await.unwrap();
    assert!(body.len() < 100);
}

#[tokio::test]
async fn immutable_deadlines_tombstones_cleanup_and_late_saves() {
    let (_dir, _store, sessions, clock) = fixture().await;
    let mut r = record(clock.load(Ordering::SeqCst));
    sessions.create(&mut r).await.unwrap();
    let loaded = sessions.load(&r.id).await.unwrap().unwrap();
    assert_eq!(loaded, r);
    sessions.save(&r).await.unwrap();
    sessions.save(&r).await.unwrap();
    let mut changed = r.clone();
    changed.expiry_date += time::Duration::seconds(1);
    assert!(sessions.save(&changed).await.is_err());
    changed = r.clone();
    changed
        .data
        .insert("csrf".into(), serde_json::json!("b".repeat(64)));
    assert!(sessions.save(&changed).await.is_err());
    // The library does not inherit loaded absolute expiry from the record.
    let session = Session::new(Some(r.id), Arc::new(sessions.clone()), None);
    assert_eq!(
        session.get::<String>("csrf").await.unwrap(),
        Some("a".repeat(64))
    );
    assert!(session.save().await.is_err());
    assert_eq!(sessions.load(&r.id).await.unwrap(), Some(r.clone()));
    sessions.delete(&r.id).await.unwrap();
    sessions.delete(&r.id).await.unwrap();
    assert!(sessions.load(&r.id).await.unwrap().is_none());
    assert!(sessions.save(&r).await.is_err());
    assert_eq!(sessions.cleanup().await.unwrap(), 0);
    clock.store(r.expiry_date.unix_timestamp(), Ordering::SeqCst);
    assert_eq!(sessions.cleanup().await.unwrap(), 1);
    assert!(sessions.save(&r).await.is_err());
    assert!(sessions.create(&mut r).await.is_err());
    let mut fractional = record(clock.load(Ordering::SeqCst));
    fractional.expiry_date += time::Duration::nanoseconds(1);
    assert!(sessions.create(&mut fractional).await.is_err());
    let mut oversized = record(clock.load(Ordering::SeqCst));
    oversized
        .data
        .insert("csrf".into(), serde_json::json!("a".repeat(4097)));
    assert!(sessions.create(&mut oversized).await.is_err());
    clock.store(0, Ordering::SeqCst);
    assert!(sessions.load(&Id::default()).await.is_err());
    assert!(sessions.delete(&Id::default()).await.is_err());
    assert!(sessions.cleanup().await.is_err());
    clock.store(253_402_300_800, Ordering::SeqCst);
    assert!(sessions.cleanup().await.is_err());
}

#[tokio::test]
async fn sql_guards_replacement_payload_projections_current_epoch_and_missing_clock() {
    let (dir, store, sessions, clock) = fixture().await;
    let mut r = record(clock.load(Ordering::SeqCst));
    sessions.create(&mut r).await.unwrap();
    let id = r.id.to_string();
    let x = id.clone();
    store.run(move|db| {
        for sql in [
            "INSERT OR REPLACE INTO auth_sessions SELECT * FROM auth_sessions",
            "UPDATE auth_sessions SET id='AAAAAAAAAAAAAAAAAAAAAA'",
            "UPDATE auth_sessions SET issued_at=issued_at-1",
            "UPDATE auth_sessions SET expires_at=expires_at+1",
            "UPDATE auth_sessions SET payload=json_object('csrf',printf('%064d',0))",
            "UPDATE auth_sessions SET account_id=1,epoch=1",
            "DELETE FROM auth_sessions",
        ] {assert!(db.execute_batch(sql).is_err(),"{sql}");}
        let payload=serde_json::json!({"csrf":"c".repeat(64),"extra":1}).to_string();
        assert!(db.execute("INSERT INTO auth_sessions VALUES(?1,1000000,1000600,'active',?2,NULL,NULL)",params![Id::default().to_string(),payload]).is_err());
        let profile="$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let payload=serde_json::json!({"csrf":"c".repeat(64)}).to_string();
        assert!(db.execute("INSERT INTO auth_sessions VALUES(?1,1000000,1000600,'active',?2,NULL,1)",params![Id::default().to_string(),payload]).is_err(),"half-present principal projection must reject SQL NULL semantics");
        db.execute("INSERT INTO accounts(login,display_name,state,epoch,verifier) VALUES('owner','Owner','enabled',1,?1)",[profile])?;
        db.execute("INSERT INTO account_owner VALUES(1,1)",[])?;
        assert!(db.execute("UPDATE auth_sessions SET state='revoked',payload=NULL WHERE id=?1",[x.clone()]).is_ok());
        assert!(db.execute("UPDATE auth_sessions SET state='active',payload=json_object('csrf',?1) WHERE id=?2",params!["a".repeat(64),x]).is_err());
        Ok(())
    }).await.unwrap();
    let raw = Connection::open(dir.path().join("db")).unwrap();
    assert!(
        raw.execute(
            "UPDATE auth_sessions SET state='revoked',payload=NULL WHERE id=?1",
            [id]
        )
        .is_err()
    );
    assert!(raw.execute("DELETE FROM auth_sessions", []).is_err());
    let mut authenticated = record(clock.load(Ordering::SeqCst));
    authenticated.expiry_date += time::Duration::seconds(28200);
    authenticated
        .data
        .insert("principal".into(), serde_json::json!({"id":1,"epoch":1}));
    sessions.create(&mut authenticated).await.unwrap();
    sessions.save(&authenticated).await.unwrap();
    store
        .run(|db| {
            db.execute("UPDATE accounts SET epoch=2 WHERE id=1", [])?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(sessions.save(&authenticated).await.is_err());
    let mut raced = authenticated.clone();
    raced.id = Id::default();
    assert!(sessions.create(&mut raced).await.is_err());
}

#[tokio::test]
async fn bounded_capacity_cleanup_and_eight_collision_attempts() {
    let (_dir, store, mut sessions, clock) = fixture().await;
    let mut r = record(clock.load(Ordering::SeqCst));
    sessions.create(&mut r).await.unwrap();
    let original = r.id;
    let attempts = Arc::new(AtomicUsize::new(0));
    let counter = attempts.clone();
    sessions.candidate = Some(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
        original
    }));
    assert!(sessions.create(&mut r.clone()).await.is_err());
    assert_eq!(attempts.load(Ordering::SeqCst), 7);
    sessions.candidate = None;
    let mut collision = r.clone();
    sessions.create(&mut collision).await.unwrap();
    assert_ne!(collision.id, r.id);
    store
        .run(|db| {
            let tx = db.transaction()?;
            let payload = serde_json::json!({"csrf":"a".repeat(64)}).to_string();
            for _ in 2..10000 {
                tx.execute(
                    "INSERT INTO auth_sessions VALUES(?1,1000000,1000600,'active',?2,NULL,NULL)",
                    params![Id::default().to_string(), payload],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(sessions.create(&mut record(1_000_000)).await.is_err());
    clock.store(1_000_600, Ordering::SeqCst);
    let mut next = record(1_000_600);
    sessions.create(&mut next).await.unwrap();
    assert_eq!(
        store
            .run(
                |db| Ok(db.query_row("SELECT count(*) FROM auth_sessions", [], |r| r
                    .get::<_, i64>(0))?)
            )
            .await
            .unwrap(),
        9901
    );
    assert_eq!(sessions.cleanup().await.unwrap(), 100);
    assert!(sessions.save(&r).await.is_err());
}

#[tokio::test]
async fn authority_origin_csrf_forms_normalization_and_readonly_no_mutation() {
    let (_dir, store, sessions, clock) = fixture().await;
    let app = router(state(store.clone()));
    let mut r = record(clock.load(Ordering::SeqCst));
    sessions.create(&mut r).await.unwrap();
    for _ in 0..12 {
        let response = app
            .clone()
            .oneshot(request("/auth/login", Some(r.id)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response.headers().contains_key(header::SET_COOKIE));
    }
    assert_eq!(sessions.load(&r.id).await.unwrap(), Some(r.clone()));
    check_authority_rejections(&app).await;
    for (origin, content, body, code) in [
        (
            None,
            "application/x-www-form-urlencoded",
            "csrf=x",
            StatusCode::FORBIDDEN,
        ),
        (
            Some("http://localhost:3000"),
            "application/x-www-form-urlencoded",
            "csrf=x",
            StatusCode::FORBIDDEN,
        ),
        (
            Some("http://127.0.0.1:3000"),
            "application/json",
            "{}",
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            Some("http://127.0.0.1:3000"),
            "application/x-www-form-urlencoded",
            "csrf=x",
            StatusCode::FORBIDDEN,
        ),
        (
            Some("http://127.0.0.1:3000"),
            "application/x-www-form-urlencoded",
            "csrf=x&csrf=y",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let mut req = Request::builder()
            .method("POST")
            .uri("/auth/logout")
            .header(header::HOST, "127.0.0.1:3000")
            .header(header::CONTENT_TYPE, content)
            .header(header::COOKIE, format!("mimik_session={}", r.id));
        if let Some(origin) = origin {
            req = req.header(header::ORIGIN, origin);
        }
        assert_error(
            app.clone()
                .oneshot(req.body(Body::from(body)).unwrap())
                .await
                .unwrap(),
            code,
        )
        .await;
    }
    let req = Request::builder()
        .method("POST")
        .uri("/auth/login")
        .header(header::HOST, "127.0.0.1:3000")
        .header(header::ORIGIN, "http://127.0.0.1:3000")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("x".repeat(8193)))
        .unwrap();
    assert_error(
        app.clone().oneshot(req).await.unwrap(),
        StatusCode::PAYLOAD_TOO_LARGE,
    )
    .await;
    for path in [
        "/characters/1",
        "/sources/1",
        "/history",
        "/context",
        "/assets/htmx.min.js",
    ] {
        assert_error(
            app.clone()
                .oneshot(request(path, Some(r.id)))
                .await
                .unwrap(),
            StatusCode::FORBIDDEN,
        )
        .await;
    }
}

async fn check_authority_rejections(app: &Router) {
    for host in [None, Some("foreign:3000")] {
        let mut req = Request::builder().uri("/auth/login");
        if let Some(host) = host {
            req = req.header(header::HOST, host);
        }
        assert_error(
            app.clone()
                .oneshot(req.body(Body::empty()).unwrap())
                .await
                .unwrap(),
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    let mut req = request("/auth/login", None);
    req.headers_mut()
        .append(header::HOST, "localhost:3000".parse().unwrap());
    assert_error(
        app.clone().oneshot(req).await.unwrap(),
        StatusCode::BAD_REQUEST,
    )
    .await;
}

#[tokio::test]
async fn corrupt_record_returns_static_500_without_cookie() {
    let (_dir, store, sessions, clock) = fixture().await;
    let app = router(state(store.clone()));
    let mut r = record(clock.load(Ordering::SeqCst));
    sessions.create(&mut r).await.unwrap();
    damage_fixture_payload(&store, r.id, "x".repeat(4097)).await;
    assert_error(
        app.oneshot(request("/auth/login", Some(r.id)))
            .await
            .unwrap(),
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .await;
}

#[tokio::test]
async fn supervised_cleanup_propagates_clock_failure() {
    let (_dir, _store, sessions, clock) = fixture().await;
    clock.store(0, Ordering::SeqCst);
    tokio::time::pause();
    let task = tokio::spawn(cleanup_loop(sessions));
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_mins(1)).await;
    tokio::time::resume();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn known_unknown_sql_flood_keeps_sixteen_worker_permits_after_cancellation() {
    let (_dir, store, sessions, clock) = fixture().await;
    let mut r = record(clock.load(Ordering::SeqCst));
    sessions.create(&mut r).await.unwrap();
    let (held_tx, held_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let holder = store.clone();
    let held = tokio::spawn(async move {
        holder
            .run(move |db| {
                let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                held_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                tx.commit()?;
                Ok(())
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), held_rx)
        .await
        .unwrap()
        .unwrap();
    let mut workers = Vec::new();
    for index in 0..16 {
        let sessions = sessions.clone();
        let id = if index % 2 == 0 { r.id } else { Id::default() };
        workers.push(tokio::spawn(async move { sessions.load(&id).await }));
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while crate::store::AUTH_WORKERS.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let app = router(state(store.clone()));
    for id in [r.id, Id::default()] {
        assert_error(
            app.clone()
                .oneshot(request("/auth/login", Some(id)))
                .await
                .unwrap(),
            StatusCode::SERVICE_UNAVAILABLE,
        )
        .await;
    }
    for worker in workers {
        worker.abort();
        assert!(worker.await.unwrap_err().is_cancelled());
    }
    assert_eq!(crate::store::AUTH_WORKERS.available_permits(), 0);
    assert!(sessions.load(&Id::default()).await.is_err());
    release_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), held)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while crate::store::AUTH_WORKERS.available_permits() != 16 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(sessions.load(&r.id).await.unwrap().is_some());
}

#[tokio::test]
async fn eight_nonqueued_requests_slow_body_timeout_and_recovery() {
    let (_dir, store, _sessions, _clock) = fixture().await;
    let app = router(state(store));
    tokio::time::pause();
    let mut tasks = Vec::new();
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    for _ in 0..8 {
        let started = started_tx.clone();
        let mut announced = false;
        let body = Body::from_stream(futures_util::stream::poll_fn(move |_| {
            if !announced {
                started.send(()).unwrap();
                announced = true;
            }
            std::task::Poll::Pending::<Option<Result<axum::body::Bytes, std::io::Error>>>
        }));
        let req = Request::builder()
            .method("POST")
            .uri("/auth/login")
            .header(header::HOST, "127.0.0.1:3000")
            .header(header::ORIGIN, "http://127.0.0.1:3000")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body)
            .unwrap();
        tasks.push(tokio::spawn(app.clone().oneshot(req)));
    }
    for _ in 0..8 {
        tokio::time::timeout(Duration::from_secs(1), started_rx.recv())
            .await
            .unwrap()
            .unwrap();
    }
    assert_eq!(REQUESTS.available_permits(), 0);
    assert_error(
        app.clone()
            .oneshot(request("/auth/session", None))
            .await
            .unwrap(),
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    tokio::time::advance(Duration::from_secs(5)).await;
    for task in tasks {
        assert_error(task.await.unwrap().unwrap(), StatusCode::REQUEST_TIMEOUT).await;
    }
    tokio::time::resume();
    assert_eq!(REQUESTS.available_permits(), 8);
    assert_error(
        app.oneshot(request("/auth/session", None)).await.unwrap(),
        StatusCode::UNAUTHORIZED,
    )
    .await;
}

#[test]
fn governor_burst_refill_is_nonqueued_and_uses_a_fake_clock() {
    let clock = governor::clock::FakeRelativeClock::default();
    let limiter = RateLimiter::direct_with_clock(
        Quota::with_period(Duration::from_secs(6))
            .unwrap()
            .allow_burst(NonZeroU32::new(10).unwrap()),
        clock.clone(),
    );
    for _ in 0..10 {
        assert!(limiter.check().is_ok());
    }
    assert!(limiter.check().is_err());
    clock.advance(Duration::from_secs(5));
    assert!(limiter.check().is_err());
    clock.advance(Duration::from_secs(1));
    assert!(limiter.check().is_ok());
    assert!(limiter.check().is_err());
}

#[tokio::test]
async fn actual_router_quota_new_csrf_only_existing_reads_bypass() {
    let (_dir, store, _sessions, _clock) = fixture().await;
    let mut state = state(store);
    state.quota = Arc::new(RateLimiter::direct(
        Quota::with_period(Duration::from_secs(6))
            .unwrap()
            .allow_burst(NonZeroU32::new(1).unwrap()),
    ));
    let app = router(state);
    let response = app
        .clone()
        .oneshot(request("/auth/login", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let id = cookie
        .trim_start_matches("mimik_session=")
        .parse::<Id>()
        .unwrap();
    for _ in 0..20 {
        assert_eq!(
            app.clone()
                .oneshot(request("/auth/login", Some(id)))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }
    let denied = app.oneshot(request("/auth/login", None)).await.unwrap();
    assert_eq!(denied.headers()[header::RETRY_AFTER], "6");
    assert_error(denied, StatusCode::TOO_MANY_REQUESTS).await;
}

#[derive(Clone, Copy, Debug)]
enum Fault {
    Create,
    Save,
    Load,
    Delete,
    RevokeBeforeSave,
}
#[derive(Clone, Debug)]
struct FaultStore {
    inner: SqliteSessions,
    fault: Fault,
}
fn injected() -> tower_sessions::session_store::Error {
    tower_sessions::session_store::Error::Backend("Synthetic session failure.".into())
}
#[async_trait::async_trait]
impl SessionStore for FaultStore {
    async fn create(&self, record: &mut Record) -> tower_sessions::session_store::Result<()> {
        if matches!(self.fault, Fault::Create) {
            return Err(injected());
        }
        self.inner.create(record).await
    }
    async fn save(&self, record: &Record) -> tower_sessions::session_store::Result<()> {
        if matches!(self.fault, Fault::Save) {
            return Err(injected());
        }
        if matches!(self.fault, Fault::RevokeBeforeSave) {
            self.inner.delete(&record.id).await?;
        }
        self.inner.save(record).await
    }
    async fn load(&self, id: &Id) -> tower_sessions::session_store::Result<Option<Record>> {
        if matches!(self.fault, Fault::Load) {
            return Err(injected());
        }
        self.inner.load(id).await
    }
    async fn delete(&self, id: &Id) -> tower_sessions::session_store::Result<()> {
        if matches!(self.fault, Fault::Delete) {
            return Err(injected());
        }
        self.inner.delete(id).await
    }
}

#[tokio::test]
async fn initial_create_second_save_revoke_load_and_delete_failures_are_static() {
    let (_dir, store, sessions, clock) = fixture().await;
    for fault in [Fault::Create, Fault::Save, Fault::RevokeBeforeSave] {
        let app = router_with_sessions(
            state(store.clone()),
            FaultStore {
                inner: sessions.clone(),
                fault,
            },
        );
        assert_error(
            app.oneshot(request("/auth/login", None)).await.unwrap(),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .await;
    }
    let counts = store
        .run(|db| {
            Ok((
                db.query_row(
                    "SELECT count(*) FROM auth_sessions WHERE state='active'",
                    [],
                    |r| r.get::<_, i64>(0),
                )?,
                db.query_row(
                    "SELECT count(*) FROM auth_sessions WHERE state='revoked'",
                    [],
                    |r| r.get::<_, i64>(0),
                )?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(counts, (1, 1));
    let mut r = record(clock.load(Ordering::SeqCst));
    sessions.create(&mut r).await.unwrap();
    let app = router_with_sessions(
        state(store.clone()),
        FaultStore {
            inner: sessions.clone(),
            fault: Fault::Load,
        },
    );
    assert_error(
        app.oneshot(request("/auth/login", Some(r.id)))
            .await
            .unwrap(),
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .await;
    let app = router_with_sessions(
        state(store),
        FaultStore {
            inner: sessions.clone(),
            fault: Fault::Delete,
        },
    );
    let req = Request::builder()
        .method("POST")
        .uri("/auth/logout")
        .header(header::HOST, "127.0.0.1:3000")
        .header(header::ORIGIN, "http://127.0.0.1:3000")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("mimik_session={}", r.id))
        .body(Body::from(format!("csrf={}", "a".repeat(64))))
        .unwrap();
    assert_error(
        app.oneshot(req).await.unwrap(),
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .await;
    assert_eq!(sessions.load(&r.id).await.unwrap(), Some(r));
}

#[tokio::test]
async fn missing_account_reconstruction_flushes_without_cached_owner_access() {
    let (dir, store, sessions, clock) = fixture().await;
    store.run(|db| {
        db.execute("INSERT INTO accounts(login,display_name,state,epoch,verifier) VALUES('owner','Owner','enabled',1,?1)",["$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"])?;
        db.execute("INSERT INTO account_owner VALUES(1,1)",[])?;Ok(())
    }).await.unwrap();
    let app = app(store.clone()).await.unwrap();
    let mut r = record(clock.load(Ordering::SeqCst));
    r.data
        .insert("principal".into(), serde_json::json!({"id":1,"epoch":1}));
    sessions.create(&mut r).await.unwrap();
    assert_eq!(
        app.clone()
            .oneshot(request("/", Some(r.id)))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let raw = Connection::open(dir.path().join("db")).unwrap();
    // Synthetic externally damaged state; normal account guards prohibit deletion.
    raw.execute_batch(
        "PRAGMA foreign_keys=OFF; DROP TRIGGER accounts_delete; DELETE FROM accounts WHERE id=1;",
    )
    .unwrap();
    assert_error(
        app.clone()
            .oneshot(request("/auth/session", Some(r.id)))
            .await
            .unwrap(),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    assert!(sessions.load(&r.id).await.unwrap().is_none());
    assert!(sessions.save(&r).await.is_err());
    assert_error(
        app.oneshot(request("/characters/1", Some(r.id)))
            .await
            .unwrap(),
        StatusCode::FORBIDDEN,
    )
    .await;
}
