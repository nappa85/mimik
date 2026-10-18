//! Account browser acceptance uses the same fresh synthetic sessions as workspace tests.
use super::*;
use crate::accounts::Accounts;
use axum::http::StatusCode;
use std::os::unix::fs::PermissionsExt;

fn invite_form(login: &str, epoch: i64) -> Vec<(String, String)> {
    vec![
        ("csrf".into(), "a".repeat(64)),
        ("login".into(), login.into()),
        ("display_name".into(), "Browser <player>".into()),
        ("epoch".into(), epoch.to_string()),
    ]
}
fn redeem_form(token: &[u8], password: &str) -> Vec<(String, String)> {
    vec![
        ("csrf".into(), "a".repeat(64)),
        ("token".into(), std::str::from_utf8(token).unwrap().into()),
        ("password".into(), password.into()),
        ("confirmation".into(), password.into()),
    ]
}
async fn html(response: axum::response::Response) -> String {
    String::from_utf8(
        axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
async fn browser_admin_issuance_redemption_reset_disable_and_attribution_are_atomic() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let accounts = Accounts::new(store.clone());
    let app = crate::auth::app(store.clone()).await.unwrap();
    let response = router_request(
        &app,
        &auth[0],
        "/admin/invite",
        Some(invite_form("browser-player", 0)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    for (name, value) in [
        ("cache-control", "no-store"),
        ("x-frame-options", "DENY"),
        ("content-security-policy", "frame-ancestors 'none'"),
        ("referrer-policy", "same-origin"),
    ] {
        assert_eq!(response.headers()[name], value);
    }
    assert!(!response.headers().contains_key("set-cookie"));
    let body = html(response).await;
    let token = body
        .split("id=\"credential-link\" readonly type=\"text\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .split("#token=")
        .nth(1)
        .unwrap()
        .as_bytes()
        .to_vec();
    assert_eq!(token.len(), 64);
    assert_eq!(
        router_request(
            &app,
            &auth[0],
            "/admin/invite",
            Some(invite_form("browser-player", 0))
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert!(
        !html(router_request(&app, &auth[0], "/admin", None).await)
            .await
            .contains(std::str::from_utf8(&token).unwrap())
    );
    let mut bad = redeem_form(&token, "abcdefghijkl");
    bad[3].1 = "different1234".into();
    assert_eq!(
        router_request(&app, &auth[4], "/auth/enroll", Some(bad))
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        router_request(
            &app,
            &auth[4],
            "/auth/reset",
            Some(redeem_form(&token, "abcdefghijkl"))
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let response = router_request(
        &app,
        &auth[4],
        "/auth/enroll",
        Some(redeem_form(&token, "abcdefghijkl")),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()["location"], "/auth/login");
    assert!(!response.headers().contains_key("set-cookie"));
    assert_eq!(
        router_request(
            &app,
            &auth[4],
            "/auth/enroll",
            Some(redeem_form(&token, "abcdefghijkl"))
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    verify_reset_disable(&store, &accounts, &app, &auth).await;
}
async fn verify_reset_disable(
    store: &Store,
    accounts: &Accounts,
    app: &axum::Router,
    auth: &[RequestAuth],
) {
    let reset = accounts.browser_reset(auth[0].clone(), 6, 2).await.unwrap();
    assert_eq!(reset.account.epoch, 3);
    assert!(accounts.browser_reset(auth[0].clone(), 6, 2).await.is_err());
    assert_eq!(
        router_request(
            app,
            &auth[4],
            "/auth/reset",
            Some(redeem_form(&reset.token, "mnopqrstuvwx"))
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    accounts
        .browser_disable(auth[0].clone(), 6, 4)
        .await
        .unwrap();
    assert!(accounts.browser_reset(auth[0].clone(), 6, 5).await.is_err());
    assert!(
        accounts
            .browser_disable(auth[0].clone(), 1, 1)
            .await
            .is_err()
    );
    assert!(accounts.browser_reset(auth[0].clone(), 1, 1).await.is_err());
    store.run(|db|{
        assert_eq!(db.query_row("SELECT count(*) FROM account_web_events",[],|r|r.get::<_,i64>(0))?,3);
        assert_eq!(db.query_row("SELECT state FROM accounts WHERE id=6",[],|r|r.get::<_,String>(0))?,"disabled");
        assert_eq!(db.query_row("SELECT count(*) FROM account_events",[],|r|r.get::<_,i64>(0))?,5);
        for sql in ["DELETE FROM account_web_events","UPDATE account_web_events SET actor_epoch=2","INSERT OR REPLACE INTO account_web_events SELECT * FROM account_web_events LIMIT 1"] {assert!(db.execute(sql,[]).is_err());}
        Ok(())
    }).await.unwrap();
}

#[tokio::test]
async fn admin_authority_pagination_stale_epochs_and_audit_failure_preserve_state() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, clock) = fixture().await;
    let accounts = Accounts::new(store.clone());
    let app = crate::auth::app(store.clone()).await.unwrap();
    for actor in [&auth[1], &auth[1].clone().development_fixture()] {
        assert!(
            accounts
                .browser_invite(actor.clone(), "denied".into(), "Denied".into(), 0)
                .await
                .is_err()
        );
        assert_eq!(
            router_request(&app, actor, "/admin/invite", Some(invite_form("denied", 0)))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    for n in 0..46 {
        accounts
            .browser_invite(
                auth[0].clone(),
                format!("reserved{n}"),
                "Reserved".into(),
                0,
            )
            .await
            .unwrap();
    }
    let first = accounts.browser_list(auth[0].clone(), 0).await.unwrap();
    assert_eq!(first.rows.len(), 50);
    assert_eq!(first.next, Some(50));
    let last = accounts.browser_list(auth[0].clone(), 50).await.unwrap();
    assert_eq!(last.rows.len(), 1);
    assert_eq!(last.next, None);
    assert!(accounts.browser_list(auth[0].clone(), -1).await.is_err());
    let original = accounts
        .browser_invite(auth[0].clone(), "reissue".into(), "Reissue".into(), 0)
        .await
        .unwrap();
    let reissued = accounts
        .browser_invite(auth[0].clone(), "reissue".into(), "Reissue".into(), 1)
        .await
        .unwrap();
    assert_ne!(*original.token, *reissued.token);
    assert!(
        accounts
            .browser_invite(auth[0].clone(), "reissue".into(), "Reissue".into(), 1)
            .await
            .is_err()
    );
    store.run(|db|{db.execute_batch("CREATE TRIGGER synthetic_audit_failure BEFORE INSERT ON account_web_events BEGIN SELECT RAISE(ABORT,'synthetic audit failure'); END;")?;Ok(())}).await.unwrap();
    assert!(
        accounts
            .browser_invite(auth[0].clone(), "rollback".into(), "Rollback".into(), 0)
            .await
            .is_err()
    );
    store
        .run(|db| {
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM accounts WHERE login='rollback'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                0
            );
            Ok(())
        })
        .await
        .unwrap();
    clock.store(1000, Ordering::SeqCst);
    assert!(accounts.browser_list(auth[0].clone(), 0).await.is_err());
    assert!(
        accounts
            .browser_disable(auth[0].clone(), 2, 1)
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn browser_redemption_rechecks_captured_csrf_after_password_work() {
    let _lock = crate::auth::test_resource_lock().await;
    for revoke in [false, true] {
        let (dir, store, auth, clock) = fixture().await;
        let accounts = Accounts::new(store.clone());
        let issued = accounts
            .browser_invite(auth[0].clone(), "race-player".into(), "Race".into(), 0)
            .await
            .unwrap();
        let path = dir.path().join("private.sqlite3");
        let clock_for_hook = clock.clone();
        let session = auth[4]
            .fixture_cookie()
            .split('=')
            .nth(1)
            .unwrap()
            .to_owned();
        let accounts = accounts.with_auth_test_hooks(
            Arc::new(move || {
                if revoke {
                    let db = rusqlite::Connection::open(&path).unwrap();
                    crate::store::register_auth_clock(&db, Arc::new(|| Ok(100))).unwrap();
                    db.execute(
                        "UPDATE auth_sessions SET state='revoked',payload=NULL WHERE id=?1",
                        [&session],
                    )
                    .unwrap();
                } else {
                    clock_for_hook.store(1000, Ordering::SeqCst);
                }
            }),
            Arc::new(|| {}),
        );
        assert!(matches!(
            accounts
                .redeem_bytes(
                    "enroll",
                    crate::accounts::digest_bytes(&issued.token).unwrap(),
                    zeroize::Zeroizing::new(b"abcdefghijkl".to_vec()),
                    Some(auth[4].csrf_fixture())
                )
                .await,
            Err(AppError::NotFound)
        ));
        store
            .run(|db| {
                assert_eq!(
                    db.query_row(
                        "SELECT state FROM accounts WHERE login='race-player'",
                        [],
                        |r| r.get::<_, String>(0)
                    )?,
                    "pending"
                );
                assert_eq!(
                    db.query_row(
                        "SELECT count(*) FROM account_credentials WHERE state='active'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )?,
                    1
                );
                Ok(())
            })
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn cli_owner_reset_can_complete_in_browser_and_existing_session_forms_are_readonly() {
    let _lock = crate::auth::test_resource_lock().await;
    let (dir, store, auth, _clock) = fixture().await;
    let accounts = Accounts::new(store.clone()).with_test_clock(Arc::new(|| Ok(100)));
    let path = dir.path().join("owner-reset.token");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    accounts.issue_reset(1, 1, &path).await.unwrap();
    let token = std::fs::read(&path).unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    for route in ["/auth/enroll", "/auth/reset"] {
        let response = router_request(&app, &auth[4], route, None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response.headers().contains_key("set-cookie"));
    }
    assert_eq!(
        router_request(
            &app,
            &auth[4],
            "/auth/reset",
            Some(redeem_form(&token, "abcdefghijklmn"))
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        router_request(
            &app,
            &auth[4],
            "/auth/reset",
            Some(redeem_form(&token, "abcdefghijklmno"))
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    store
        .run(|db| {
            assert_eq!(
                db.query_row("SELECT epoch FROM accounts WHERE id=1", [], |r| r
                    .get::<_, i64>(0))?,
                3
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM account_web_events", [], |r| r
                    .get::<_, i64>(0))?,
                0
            );
            assert_eq!(
                db.query_row(
                    "SELECT expires_at FROM auth_sessions WHERE account_id=5",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                1000
            );
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn populated_schema13_upgrade_and_exact_large_account_ids_are_preserved() {
    let _lock = crate::auth::test_resource_lock().await;
    let (dir, store, auth, _clock) = fixture().await;
    store.run(|db|{
        assert_eq!(db.query_row("SELECT count(*) FROM account_web_events",[],|r|r.get::<_,i64>(0))?,0);
        assert_eq!(db.query_row("SELECT count(*) FROM source_web_events",[],|r|r.get::<_,i64>(0))?,0);
        crate::sources::search::drop_schema(db)?;
        db.execute_batch("DROP TABLE registry_web_events; DROP TABLE registry_events; DROP TABLE source_web_events; DROP INDEX source_review_page_history; DROP TABLE account_web_events; DROP VIEW account_web_request; DROP TABLE catalogue_drafts; PRAGMA user_version=13;")?;
        db.execute("INSERT INTO account_events(account_id,epoch,occurred_at,actor,action) VALUES(1,1,100,'local_operator','bootstrap')",[])?;
        db.execute("INSERT INTO accounts(id,login,display_name,state,epoch) VALUES(9007199254740993,'large-account','Exact large ID','pending',1)",[])?;
        Ok(())
    }).await.unwrap();
    let reopened = Store::open(&dir.path().join("private.sqlite3")).unwrap();
    reopened
        .run(|db| crate::store::register_auth_clock(db, Arc::new(|| Ok(100))))
        .await
        .unwrap();
    let accounts = Accounts::new(reopened.clone());
    let page = accounts
        .browser_list(auth[0].clone(), 9_007_199_254_740_992)
        .await
        .unwrap();
    assert_eq!(page.rows[0].id, 9_007_199_254_740_993);
    assert_eq!(page.next, None);
    let app = crate::auth::app(reopened.clone()).await.unwrap();
    assert!(
        html(router_request(&app, &auth[0], "/admin?after=9007199254740992", None).await)
            .await
            .contains("9007199254740993")
    );
    reopened
        .run(|db| {
            assert_eq!(
                db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
                crate::store::SCHEMA_VERSION
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM auth_sessions", [], |r| r
                    .get::<_, i64>(0))?,
                5
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM account_events", [], |r| r
                    .get::<_, i64>(0))?,
                1
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM account_web_events", [], |r| r
                    .get::<_, i64>(0))?,
                0
            );
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn owner_disable_revokes_player_session_without_altering_private_custody_history() {
    let _lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Retain custody".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    store.request_join(auth[2].clone(), campaign).await.unwrap();
    store
        .decide_join(auth[1].clone(), campaign, 3, 1, true)
        .await
        .unwrap();
    let saved = insert_native(&store, auth[2].clone(), native(campaign, 3)).await;
    let accounts = Accounts::new(store.clone());
    accounts
        .browser_disable(auth[0].clone(), 3, 1)
        .await
        .unwrap();
    assert!(store.private_list(auth[2].clone(), None, 0).await.is_err());
    let (view, permit) = store
        .private_read(
            auth[1].clone(),
            1,
            None,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_string(&view.snapshot).unwrap(),
        serde_json::to_string(&saved).unwrap()
    );
    drop(permit);
    assert_private_counts(&store, 1).await;
}

#[tokio::test]
async fn anonymous_enrollment_uses_durable_csrf_and_never_echoes_credentials() {
    use tower::ServiceExt;
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let accounts = Accounts::new(store.clone());
    let issued = accounts
        .browser_invite(
            auth[0].clone(),
            "anonymous-player".into(),
            "Anonymous".into(),
            0,
        )
        .await
        .unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/auth/enroll")
                .header("host", "127.0.0.1:3000")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let body = html(response).await;
    let csrf = body
        .split("name=\"csrf\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned();
    let mut form = redeem_form(&issued.token, "abcdefghijkl");
    form[0].1 = csrf;
    let encoded = serde_urlencoded::to_string(&form).unwrap();
    for (body, origin, status) in [
        (
            format!("{encoded}&password=abcdefghijkl"),
            "http://127.0.0.1:3000",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            encoded.clone(),
            "http://localhost:3000",
            StatusCode::FORBIDDEN,
        ),
        (
            format!("{encoded}&extra=x"),
            "http://127.0.0.1:3000",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/auth/enroll")
                    .header("host", "127.0.0.1:3000")
                    .header("origin", origin)
                    .header("cookie", &cookie)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert!(!response.headers().contains_key("set-cookie"));
        let response = html(response).await;
        assert!(!response.contains("abcdefghijkl"));
        assert!(!response.contains(std::str::from_utf8(&issued.token).unwrap()));
    }
    let response = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/auth/enroll")
                .header("host", "127.0.0.1:3000")
                .header("origin", "http://127.0.0.1:3000")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(axum::body::Body::from(encoded))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(!response.headers().contains_key("set-cookie"));
}

#[tokio::test]
async fn regression_revoked_reset_form_preserves_redemption_destination() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    Accounts::new(store.clone())
        .browser_reset(auth[0].clone(), 5, 1)
        .await
        .unwrap();
    let app = crate::auth::app(store).await.unwrap();
    let response = router_request(&app, &auth[4], "/auth/reset", None).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()["location"], "/auth/reset");
}

#[tokio::test]
async fn nondefault_owner_captured_authority_loses_logout_reset_disable_and_expiry() {
    let _lock = crate::auth::test_resource_lock().await;
    for transition in ["logout", "reset", "disable", "expiry"] {
        let (dir, store, auth, clock) = fixture_with_owner(101).await;
        let accounts = Accounts::new(store.clone()).with_test_clock(Arc::new(|| Ok(100)));
        let issued = accounts
            .browser_invite(
                auth[0].clone(),
                "initial-invite".into(),
                "Initial".into(),
                0,
            )
            .await
            .unwrap();
        assert_eq!(issued.account.id, 102);
        let app = crate::auth::app(store.clone()).await.unwrap();
        assert_eq!(
            router_request(&app, &auth[0], "/admin", None)
                .await
                .status(),
            StatusCode::OK
        );
        match transition {
            "logout" => {
                let id: Id = auth[0]
                    .fixture_cookie()
                    .split('=')
                    .nth(1)
                    .unwrap()
                    .parse()
                    .unwrap();
                SqliteSessions::new(store.clone())
                    .delete(&id)
                    .await
                    .unwrap();
            }
            "reset" => {
                std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
                    .unwrap();
                accounts
                    .issue_reset(101, 1, &dir.path().join("owner.token"))
                    .await
                    .unwrap();
            }
            "disable" => {
                accounts.disable(101, 1).await.unwrap();
            }
            _ => {
                clock.store(1000, Ordering::SeqCst);
            }
        }
        assert!(matches!(
            accounts
                .browser_invite(auth[0].clone(), "must-not-exist".into(), "Denied".into(), 0)
                .await,
            Err(AppError::NotFound)
        ));
        assert!(accounts.browser_list(auth[0].clone(), 0).await.is_err());
        store
            .run(|db| {
                assert_eq!(
                    db.query_row(
                        "SELECT count(*) FROM accounts WHERE login='must-not-exist'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )?,
                    0
                );
                assert_eq!(
                    db.query_row("SELECT actor_id FROM account_web_events", [], |r| r
                        .get::<_, i64>(0))?,
                    101
                );
                Ok(())
            })
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn credential_retry_keeps_token_clears_password_and_does_not_consume() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let issued = Accounts::new(store.clone())
        .browser_invite(auth[0].clone(), "retry-player".into(), "Retry".into(), 0)
        .await
        .unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    let mut form = redeem_form(&issued.token, "abcdefghijkl");
    form[3].1 = "different1234".into();
    let response = router_request(&app, &auth[4], "/auth/enroll", Some(form)).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!response.headers().contains_key("set-cookie"));
    let body = html(response).await;
    assert!(body.contains(&format!(
        "name=\"token\" value=\"{}\"",
        std::str::from_utf8(&issued.token).unwrap()
    )));
    assert!(!body.contains("abcdefghijkl"));
    assert!(!body.contains("different1234"));
    assert!(body.contains("passwords do not match"));
    store
        .run(|db| {
            assert_eq!(
                db.query_row(
                    "SELECT state FROM accounts WHERE login='retry-player'",
                    [],
                    |r| r.get::<_, String>(0)
                )?,
                "pending"
            );
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM account_credentials WHERE state='active'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                1
            );
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        router_request(
            &app,
            &auth[4],
            "/auth/enroll",
            Some(redeem_form(&issued.token, "abcdefghijkl"))
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
}

#[tokio::test]
async fn configured_https_origin_rejects_crossed_origins_and_emits_secure_cookie() {
    use tower::ServiceExt;
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, _auth, _clock) = fixture().await;
    let external =
        crate::network::external_url("https://mimik.example", crate::network::Network::Loopback)
            .unwrap();
    let app = crate::auth::app_with_options(
        store,
        crate::auth::AuthOptions {
            external_url: Some(external),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/auth/login")
                .header("host", "mimik.example")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()["set-cookie"].to_str().unwrap();
    assert!(cookie.contains("Secure"));
    let cookie = cookie.split(';').next().unwrap().to_owned();
    let body = html(response).await;
    let csrf = body
        .split("name=\"csrf\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned();
    for (host, origin) in [
        ("mimik.example", "http://mimik.example"),
        ("mimik.example", "http://localhost:3000"),
        ("localhost:3000", "https://mimik.example"),
    ] {
        let form = serde_urlencoded::to_string([
            ("csrf", csrf.as_str()),
            ("login", "unknown-player"),
            ("password", "abcdefghijkl"),
        ])
        .unwrap();
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/auth/login")
                    .header("host", host)
                    .header("origin", origin)
                    .header("cookie", &cookie)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("x-forwarded-proto", "https")
                    .body(axum::body::Body::from(form))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(!response.headers().contains_key("set-cookie"));
    }
}
