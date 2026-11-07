use super::*;

fn post(id: Id) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/auth/login")
        .header("host", "127.0.0.1:3000")
        .header("origin", "http://127.0.0.1:3000")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "text/html")
        .header("sec-fetch-mode", "navigate")
        .header("sec-fetch-dest", "document")
        .header("cookie", format!("mimik_session={id}"))
        .body(Body::from(
            serde_urlencoded::to_string([
                ("csrf", "a".repeat(64)),
                ("login", "unknown-player".into()),
                ("password", "abcdefghijkl".into()),
            ])
            .unwrap(),
        ))
        .unwrap()
}
async fn login_retry(response: Response, status: StatusCode, retry: &str) {
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()[header::RETRY_AFTER], retry);
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let html = String::from_utf8(
        to_bytes(response.into_body(), 16384)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("role=\"alert\""));
    assert!(html.contains("value=\"unknown-player\""));
    assert!(!html.contains("abcdefghijkl"));
    assert!(html.contains("name=\"password\""));
}
#[tokio::test]
async fn document_quota_failure_retains_login_without_auth_or_cookie() {
    let (_dir, store, sessions, clock) = fixture().await;
    let mut record = record(clock.load(Ordering::SeqCst));
    sessions.create(&mut record).await.unwrap();
    let mut state = state(store.clone());
    state.quota = Arc::new(RateLimiter::direct(
        Quota::with_period(Duration::from_secs(6)).unwrap(),
    ));
    state.quota.check().unwrap();
    login_retry(
        router(state).oneshot(post(record.id)).await.unwrap(),
        StatusCode::TOO_MANY_REQUESTS,
        "6",
    )
    .await;
    store
        .run(|db| {
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
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn document_password_busy_preserves_the_retry_form_without_account_binding() {
    let (_dir, store, sessions, clock) = fixture().await;
    let mut record = record(clock.load(Ordering::SeqCst));
    sessions.create(&mut record).await.unwrap();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release = Arc::new(std::sync::Mutex::new(release_rx));
    let accounts = Accounts::new(store.clone()).with_auth_test_hooks(
        Arc::new(move || {
            ready_tx.send(()).unwrap();
            release.lock().unwrap().recv().unwrap();
        }),
        Arc::new(|| {}),
    );
    let blocked = tokio::spawn(async move {
        accounts
            .authenticate(
                "unknown-player".into(),
                Zeroizing::new(b"abcdefghijkl".to_vec()),
            )
            .await
    });
    tokio::task::spawn_blocking(move || ready_rx.recv_timeout(Duration::from_secs(5)).unwrap())
        .await
        .unwrap();
    let response = router(state(store.clone()))
        .oneshot(post(record.id))
        .await
        .unwrap();
    release_tx.send(()).unwrap();
    assert!(blocked.await.unwrap().unwrap().is_none());
    login_retry(response, StatusCode::SERVICE_UNAVAILABLE, "1").await;
    store
        .run(|db| {
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
#[tokio::test]
async fn busy_flag_preserves_only_failed_retry_not_success_or_cookie() {
    let _guard = AUTH_TEST_LOCK.clone().lock_owned().await;
    let app = Router::new()
        .route(
            "/auth/login",
            axum::routing::post(|| async {
                mark_storage_busy();
                errors::login(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unknown-player",
                    &"a".repeat(64),
                    "The server is busy.",
                )
                .unwrap()
            }),
        )
        .layer(middleware::from_fn(boundary));
    login_retry(
        app.oneshot(post(Id::default())).await.unwrap(),
        StatusCode::SERVICE_UNAVAILABLE,
        "1",
    )
    .await;
    let app = Router::new()
        .route(
            "/auth/login",
            axum::routing::post(|| async {
                mark_storage_busy();
                let mut response = Redirect::to("/auth/session").into_response();
                response.extensions_mut().insert(errors::LoginRetry);
                response.headers_mut().insert(
                    header::SET_COOKIE,
                    "mimik_session=unsafe; Path=/".parse().unwrap(),
                );
                response
            }),
        )
        .layer(middleware::from_fn(boundary));
    assert_error_document(
        app.oneshot(post(Id::default())).await.unwrap(),
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
}
#[tokio::test]
async fn document_timeout_is_styled_while_plain_control_keeps_short_body() {
    let _guard = AUTH_TEST_LOCK.clone().lock_owned().await;
    tokio::time::pause();
    let app = Router::new()
        .route(
            "/slow",
            axum::routing::get(|| async { std::future::pending::<StatusCode>().await }),
        )
        .layer(middleware::from_fn(boundary));
    assert_error_document(
        app.clone()
            .oneshot(navigation("/slow", None))
            .await
            .unwrap(),
        StatusCode::REQUEST_TIMEOUT,
    )
    .await;
    assert_error(
        app.oneshot(request("/slow", None)).await.unwrap(),
        StatusCode::REQUEST_TIMEOUT,
    )
    .await;
}
#[tokio::test]
async fn document_body_limit_is_styled_and_does_not_enter_authentication() {
    let (_dir, store, _sessions, _clock) = fixture().await;
    let mut request = post(Id::default());
    *request.body_mut() = Body::from("x".repeat(9000));
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let mut state = state(store);
    state.accounts = state.accounts.with_auth_test_hooks(
        Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }),
        Arc::new(|| {}),
    );
    assert_error_document(
        router(state).oneshot(request).await.unwrap(),
        StatusCode::PAYLOAD_TOO_LARGE,
    )
    .await;
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}
#[test]
fn login_marker_cannot_preserve_unapproved_statuses() {
    for code in 100..600 {
        let status = StatusCode::from_u16(code).unwrap();
        let mut response = status.into_response();
        response.extensions_mut().insert(errors::LoginRetry);
        assert_eq!(
            errors::trusted(&response),
            [401, 403, 429, 503].contains(&code)
        );
    }
}

#[tokio::test]
async fn test_deadline_header_lengthens_only_its_own_request() {
    let _guard = AUTH_TEST_LOCK.clone().lock_owned().await;
    tokio::time::pause();
    let app = Router::new()
        .route(
            "/slow",
            axum::routing::get(|| async {
                tokio::time::sleep(Duration::from_secs(10)).await;
                StatusCode::OK
            }),
        )
        .layer(middleware::from_fn(boundary));
    let slow = |header: Option<&str>| {
        let mut builder = axum::http::Request::builder()
            .uri("/slow")
            .header("host", "127.0.0.1:3000");
        if let Some(seconds) = header {
            builder = builder.header(TEST_DEADLINE_HEADER, seconds);
        }
        builder.body(axum::body::Body::empty()).unwrap()
    };
    assert_eq!(
        app.clone().oneshot(slow(None)).await.unwrap().status(),
        StatusCode::REQUEST_TIMEOUT
    );
    assert_eq!(
        app.clone()
            .oneshot(slow(Some("60")))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}
