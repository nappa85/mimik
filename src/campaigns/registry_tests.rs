//! Owner browser ruleset registration; synthetic identities only.
use super::*;
use crate::registry::{RegisterAlias, RegisterRuleset, RulesetKey};
use axum::http::StatusCode;

fn ruleset_form(game: &str, game_label: &str, edition: &str, label: &str) -> Vec<(String, String)> {
    vec![
        ("csrf".into(), "a".repeat(64)),
        ("game_id".into(), game.into()),
        ("game_label".into(), game_label.into()),
        ("edition_id".into(), edition.into()),
        ("edition_label".into(), label.into()),
    ]
}
fn alias_form(game: &str, edition: &str, legacy: &str) -> Vec<(String, String)> {
    vec![
        ("csrf".into(), "a".repeat(64)),
        ("game_id".into(), game.into()),
        ("edition_id".into(), edition.into()),
        ("legacy_edition".into(), legacy.into()),
    ]
}
fn ruleset(game: &str, game_label: &str, edition: &str, label: &str) -> RegisterRuleset {
    RegisterRuleset {
        game_id: game.into(),
        game_label: game_label.into(),
        edition_id: edition.into(),
        edition_label: label.into(),
    }
}
async fn text(response: axum::response::Response) -> String {
    String::from_utf8(
        axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
async fn document(
    app: &axum::Router,
    auth: &RequestAuth,
    path: &str,
    form: Vec<(String, String)>,
) -> axum::response::Response {
    use tower::ServiceExt;
    app.clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri(path)
                .header("host", "127.0.0.1:3000")
                .header("origin", "http://127.0.0.1:3000")
                .header("accept", "text/html")
                .header("sec-fetch-mode", "navigate")
                .header("sec-fetch-dest", "document")
                .header("cookie", auth.fixture_cookie())
                .header("content-type", "application/x-www-form-urlencoded")
                .body(axum::body::Body::from(
                    serde_urlencoded::to_string(form).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}
async fn counts(store: &Store) -> [i64; 5] {
    store
        .run(|db| {
            let mut out = [0; 5];
            for (slot, table) in out.iter_mut().zip([
                "registry_games",
                "registry_editions",
                "legacy_ruleset_aliases",
                "registry_events",
                "registry_web_events",
            ]) {
                *slot = db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?;
            }
            Ok(out)
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn ruleset_router_owner_matrix_and_escaped_listing() {
    use tower::ServiceExt;
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture_with_owner(101).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let anonymous = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/admin/rulesets")
                .header("host", "127.0.0.1:3000")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    for player in [&auth[1], &auth[1].clone().development_fixture()] {
        assert_eq!(
            router_request(&app, player, "/admin/rulesets", None)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        let form = ruleset_form("player-game", "Player game", "first", "First");
        assert_eq!(
            router_request(&app, player, "/admin/rulesets", Some(form))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        let form = alias_form("mork-borg", "it-2025", "Player alias");
        assert_eq!(
            router_request(&app, player, "/admin/rulesets/aliases", Some(form))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(counts(&store).await, [1, 1, 1, 0, 0]);
    let page = text(router_request(&app, &auth[0], "/admin/rulesets", None).await).await;
    assert!(page.contains("MÖRK BORG — IT 2025"), "{page}");
    assert!(page.contains(&"a".repeat(64)));
    let hostile = ruleset_form(
        "script-game",
        "Game <script>alert(1)</script>",
        "first",
        "<script>edition()</script>",
    );
    let saved = router_request(&app, &auth[0], "/admin/rulesets", Some(hostile)).await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    let page = text(router_request(&app, &auth[0], "/admin/rulesets", None).await).await;
    assert!(!page.contains("<script>"), "{page}");
    assert!(page.contains("&#60;script&#62;alert(1)") || page.contains("&lt;script&gt;alert(1)"));
    let admin = text(router_request(&app, &auth[0], "/admin", None).await).await;
    assert!(admin.contains("href=\"/admin/rulesets\""), "{admin}");
}

#[tokio::test]
async fn ruleset_missing_wrong_revoked_csrf_and_foreign_origin_do_not_mutate() {
    use tower::ServiceExt;
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    for (path, form) in [
        (
            "/admin/rulesets",
            ruleset_form("csrf-game", "CSRF game", "first", "First"),
        ),
        (
            "/admin/rulesets/aliases",
            alias_form("mork-borg", "it-2025", "Other label"),
        ),
    ] {
        let mut missing = form.clone();
        missing.retain(|field| field.0 != "csrf");
        assert_eq!(
            router_request(&app, &auth[0], path, Some(missing))
                .await
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let mut wrong = form.clone();
        wrong[0].1 = "b".repeat(64);
        assert_eq!(
            router_request(&app, &auth[0], path, Some(wrong))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        let foreign = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("host", "127.0.0.1:3000")
                    .header("origin", "http://evil.example")
                    .header("cookie", auth[0].fixture_cookie())
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(axum::body::Body::from(
                        serde_urlencoded::to_string(form).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
    }
    let mut hostile = ruleset_form("actor-game", "Actor game", "first", "First");
    hostile.push(("actor".into(), "someone else".into()));
    assert_eq!(
        router_request(&app, &auth[0], "/admin/rulesets", Some(hostile))
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut large = ruleset_form("large-game", "Large game", "first", "First");
    large[2].1 = "x".repeat(9000);
    assert_eq!(
        router_request(&app, &auth[0], "/admin/rulesets", Some(large))
            .await
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    store
        .run(|db| {
            db.execute(
                "UPDATE auth_sessions SET state='revoked',payload=NULL WHERE account_id=1",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let form = ruleset_form("revoked-game", "Revoked game", "first", "First");
    assert_eq!(
        router_request(&app, &auth[0], "/admin/rulesets", Some(form))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(counts(&store).await, [1, 1, 1, 0, 0]);
}

#[tokio::test]
async fn ruleset_events_are_idempotent_append_only_and_attributed_only_for_browser() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture_with_owner(101).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let form = ruleset_form("web-game", "Web game", "first", "First edition");
    for _ in 0..2 {
        let response = router_request(&app, &auth[0], "/admin/rulesets", Some(form.clone())).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
    }
    // Idempotent browser and CLI repeats write no event and keep the existing state.
    store
        .register_ruleset(ruleset("web-game", "Web game", "first", "First edition"))
        .await
        .unwrap();
    assert_eq!(counts(&store).await, [2, 2, 1, 1, 1]);
    let unchanged = store
        .owner_register_ruleset(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            ruleset("web-game", "Web game", "first", "First edition"),
        )
        .await
        .unwrap();
    assert!(!unchanged);
    store
        .register_ruleset(ruleset("web-game", "Web game", "second", "Second edition"))
        .await
        .unwrap();
    assert_eq!(counts(&store).await, [2, 3, 1, 2, 1]);
    let alias = alias_form("web-game", "first", "Old first label");
    for _ in 0..2 {
        let response = router_request(
            &app,
            &auth[0],
            "/admin/rulesets/aliases",
            Some(alias.clone()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
    }
    assert_eq!(counts(&store).await, [2, 3, 2, 3, 2]);
    store
        .run(|db| {
            let rows = db
                .prepare("SELECT e.kind,e.game_id,e.edition_id,e.game_label,e.label,w.actor_id FROM registry_events e LEFT JOIN registry_web_events w ON w.event_id=e.id ORDER BY e.id")?
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<i64>>(5)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            assert_eq!(
                rows,
                vec![
                    ("ruleset".into(), "web-game".into(), "first".into(), Some("Web game".into()), "First edition".into(), Some(101)),
                    ("ruleset".into(), "web-game".into(), "second".into(), Some("Web game".into()), "Second edition".into(), None),
                    ("alias".into(), "web-game".into(), "first".into(), None, "Old first label".into(), Some(101)),
                ]
            );
            for sql in [
                "DELETE FROM registry_events",
                "UPDATE registry_events SET label='changed'",
                "INSERT OR REPLACE INTO registry_events SELECT * FROM registry_events",
                "INSERT INTO registry_events(kind,game_id,edition_id,game_label,label) VALUES('ruleset','web-game','first','Web game','First edition')",
                "INSERT INTO registry_events(kind,game_id,edition_id,label) VALUES('alias','web-game','first','Not an alias')",
                "DELETE FROM registry_web_events",
                "UPDATE registry_web_events SET actor_epoch=2",
                "INSERT OR REPLACE INTO registry_web_events SELECT * FROM registry_web_events",
                "INSERT INTO registry_web_events VALUES(2,101,1,'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa')",
            ] {
                assert!(db.execute(sql, []).is_err(), "{sql}");
            }
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn ruleset_collisions_and_native_alias_retain_escaped_input_without_writes() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    store
        .register_ruleset(ruleset("kept-game", "Kept game", "first", "First"))
        .await
        .unwrap();
    store
        .register_ruleset(ruleset("kept-game", "Kept game", "second", "Second"))
        .await
        .unwrap();
    let before = counts(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    for (form, retained) in [
        (
            ruleset_form("kept-game", "<script>relabel()</script>", "third", "Third"),
            "relabel()",
        ),
        (
            ruleset_form(
                "kept-game",
                "Kept game",
                "first",
                "<script>renamed()</script>",
            ),
            "renamed()",
        ),
        (
            ruleset_form("kept-game", "Kept game", "third", "Second"),
            "value=\"third\"",
        ),
        (
            ruleset_form("Bad ID", "Kept game", "third", "Third"),
            "value=\"Bad ID\"",
        ),
    ] {
        let response = document(&app, &auth[0], "/admin/rulesets", form).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let html = text(response).await;
        assert!(html.contains("was not saved"), "{html}");
        assert!(html.contains(retained), "{html}");
        assert!(!html.contains("<script>"), "{html}");
    }
    for (form, retained) in [
        (
            alias_form("mork-borg", "it-2025", "<script>native()</script>"),
            "native()",
        ),
        (
            alias_form("kept-game", "first", "Second"),
            "value=\"Second\"",
        ),
        (
            alias_form("kept-game", "missing", "Missing alias"),
            "Missing alias",
        ),
    ] {
        let response = document(&app, &auth[0], "/admin/rulesets/aliases", form).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let html = text(response).await;
        assert!(html.contains("was not saved"), "{html}");
        assert!(html.contains(retained), "{html}");
        assert!(!html.contains("<script>"), "{html}");
    }
    store
        .register_ruleset_alias(RegisterAlias {
            game_id: "kept-game".into(),
            edition_id: "first".into(),
            legacy_edition: "Shared alias".into(),
        })
        .await
        .unwrap();
    let after_cli = counts(&store).await;
    let response = document(
        &app,
        &auth[0],
        "/admin/rulesets/aliases",
        alias_form("kept-game", "second", "Shared alias"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(counts(&store).await, after_cli);
    assert_eq!(after_cli[..3], [before[0], before[1], before[2] + 1]);
}

#[tokio::test]
async fn ruleset_listing_composite_cursor_covers_more_than_fifty_editions() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    for (game, editions) in [("aa-game", 49), ("zz-game", 11)] {
        for n in 0..editions {
            store
                .register_ruleset(ruleset(
                    game,
                    game,
                    &format!("e{n:02}"),
                    &format!("Edition {n}"),
                ))
                .await
                .unwrap();
        }
    }
    let first = store.owner_rulesets(auth[0].clone(), None).await.unwrap();
    assert_eq!(first.rows.len(), 50);
    assert_eq!(first.rows[0].game_id, "aa-game");
    let next = first.next.clone().unwrap();
    assert_eq!(
        next,
        RulesetKey {
            game_id: "mork-borg".into(),
            edition_id: "it-2025".into()
        }
    );
    let second = store
        .owner_rulesets(auth[0].clone(), Some(next))
        .await
        .unwrap();
    assert_eq!(second.rows.len(), 11);
    assert_eq!(second.next, None);
    let mut seen = first
        .rows
        .iter()
        .chain(&second.rows)
        .map(|row| (row.game_id.clone(), row.edition_id.clone()))
        .collect::<Vec<_>>();
    let total = seen.len();
    seen.dedup();
    assert_eq!((total, seen.len()), (61, 61));
    let app = crate::auth::app(store.clone()).await.unwrap();
    let page = text(router_request(&app, &auth[0], "/admin/rulesets", None).await).await;
    assert!(
        page.contains("/admin/rulesets?after_game=mork-borg&amp;after_edition=it-2025"),
        "{page}"
    );
    let page = text(
        router_request(
            &app,
            &auth[0],
            "/admin/rulesets?after_game=mork-borg&after_edition=it-2025",
            None,
        )
        .await,
    )
    .await;
    assert!(page.contains("zz-game") && !page.contains("aa-game"));
    for query in [
        "after_game=mork-borg",
        "after_edition=it-2025",
        "after_game=Bad&after_edition=it-2025",
        "after_game=mork-borg&after_edition=",
        "after=1",
    ] {
        assert_ne!(
            router_request(&app, &auth[0], &format!("/admin/rulesets?{query}"), None)
                .await
                .status(),
            StatusCode::OK,
            "{query}"
        );
    }
}

#[tokio::test]
async fn ruleset_queued_expiry_and_revocation_reject_registration() {
    let _lock = crate::auth::test_resource_lock().await;
    for alias in [false, true] {
        for revoke in [false, true] {
            let (_dir, store, auth, clock) = fixture().await;
            let (entered, waiting) = tokio::sync::oneshot::channel();
            let (release, released) = std::sync::mpsc::channel();
            let locking = store.clone();
            let lock = tokio::spawn(async move {
                locking
                    .run(move |db| {
                        entered.send(()).unwrap();
                        released
                            .recv_timeout(std::time::Duration::from_secs(5))
                            .unwrap();
                        if revoke {
                            db.execute(
                                "UPDATE auth_sessions SET state='revoked',payload=NULL WHERE account_id=1",
                                [],
                            )?;
                        }
                        Ok(())
                    })
                    .await
            });
            waiting.await.unwrap();
            let saving = store.clone();
            let captured = auth[0].clone();
            let write = tokio::spawn(async move {
                if alias {
                    saving
                        .owner_register_alias(
                            captured.clone(),
                            captured.csrf_fixture(),
                            RegisterAlias {
                                game_id: "mork-borg".into(),
                                edition_id: "it-2025".into(),
                                legacy_edition: "MÖRK BORG — IT 2025".into(),
                            },
                        )
                        .await
                } else {
                    saving
                        .owner_register_ruleset(
                            captured.clone(),
                            captured.csrf_fixture(),
                            ruleset("queued-game", "Queued", "first", "First"),
                        )
                        .await
                }
            });
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while crate::store::AUTH_WORKERS.available_permits() == 16 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            if !revoke {
                clock.store(1000, Ordering::SeqCst);
            }
            release.send(()).unwrap();
            lock.await.unwrap().unwrap();
            assert!(matches!(write.await.unwrap(), Err(AppError::NotFound)));
            assert_eq!(counts(&store).await, [1, 1, 1, 0, 0]);
        }
    }
}

#[tokio::test]
async fn ruleset_audit_failure_and_final_recheck_roll_back_registration() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, clock) = fixture().await;
    store
        .run(|db| {
            db.execute_batch("CREATE TRIGGER registry_audit_fault BEFORE INSERT ON registry_web_events BEGIN SELECT RAISE(ABORT,'fixture fault'); END;")?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(
        store
            .owner_register_ruleset(
                auth[0].clone(),
                auth[0].csrf_fixture(),
                ruleset("fault-game", "Fault", "first", "First"),
            )
            .await
            .is_err()
    );
    assert_eq!(counts(&store).await, [1, 1, 1, 0, 0]);
    store
        .run(move |db| {
            db.execute_batch("DROP TRIGGER registry_audit_fault;")?;
            db.create_scalar_function(
                "registry_test_expire",
                0,
                rusqlite::functions::FunctionFlags::SQLITE_UTF8,
                move |_| {
                    clock.store(1000, Ordering::SeqCst);
                    Ok(1)
                },
            )?;
            db.execute_batch("CREATE TRIGGER registry_final_expiry AFTER INSERT ON registry_web_events BEGIN SELECT registry_test_expire(); END;")?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(matches!(
        store
            .owner_register_ruleset(
                auth[0].clone(),
                auth[0].csrf_fixture(),
                ruleset("expired-game", "Expired", "first", "First"),
            )
            .await,
        Err(AppError::NotFound)
    ));
    assert_eq!(counts(&store).await, [1, 1, 1, 0, 0]);
}

#[tokio::test]
async fn populated_schema15_upgrade_preserves_registry_and_adds_unattributed_history() {
    let _lock = crate::auth::test_resource_lock().await;
    let (dir, store, auth, _clock) = fixture().await;
    store
        .register_ruleset(ruleset("retained-game", "Retained", "first", "First"))
        .await
        .unwrap();
    store
        .register_ruleset_alias(RegisterAlias {
            game_id: "retained-game".into(),
            edition_id: "first".into(),
            legacy_edition: "Retained old label".into(),
        })
        .await
        .unwrap();
    store
        .run(|db| {
            // Rewind exactly to before the registry-history migration, whatever its number;
            // later (derived search) migrations are dropped and re-applied too.
            let registry = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"))
                .map_err(AppError::from)?
                .filter_map(Result::ok)
                .filter_map(|entry| entry.file_name().into_string().ok())
                .find(|name| name.ends_with("_registry_administration.sql"))
                .and_then(|name| name[..3].parse::<i64>().ok())
                .ok_or(AppError::Invalid("registry migration missing"))?;
            crate::sources::search::drop_schema(db)?;
            db.execute_batch(&format!(
                "DROP TABLE registry_web_events; DROP TABLE registry_events; PRAGMA user_version={};",
                registry - 1
            ))?;
            Ok(())
        })
        .await
        .unwrap();
    let reopened = Store::open(&dir.path().join("private.sqlite3")).unwrap();
    reopened
        .run(|db| crate::store::register_auth_clock(db, Arc::new(|| Ok(100))))
        .await
        .unwrap();
    assert_eq!(counts(&reopened).await, [2, 2, 2, 0, 0]);
    let listing = reopened
        .owner_rulesets(auth[0].clone(), None)
        .await
        .unwrap();
    assert!(
        listing
            .rows
            .iter()
            .any(|row| row.game_id == "retained-game")
    );
    assert!(
        !reopened
            .owner_register_ruleset(
                auth[0].clone(),
                auth[0].csrf_fixture(),
                ruleset("retained-game", "Retained", "first", "First"),
            )
            .await
            .unwrap()
    );
    assert!(
        reopened
            .owner_register_ruleset(
                auth[0].clone(),
                auth[0].csrf_fixture(),
                ruleset("retained-game", "Retained", "second", "Second"),
            )
            .await
            .unwrap()
    );
    assert_eq!(counts(&reopened).await, [2, 3, 2, 1, 1]);
    reopened
        .run(|db| {
            assert_eq!(
                db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
                crate::store::SCHEMA_VERSION
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
                    .get::<_, i64>(
                    0
                ))?,
                0
            );
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn ruleset_web_attribution_rejects_non_latest_event_even_for_valid_owner_request() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    for edition in ["first", "second"] {
        store
            .register_ruleset(ruleset("cli-game", "CLI game", edition, edition))
            .await
            .unwrap();
    }
    let owner = auth[0].clone();
    store
        .run(move |db| {
            let insert = "INSERT INTO registry_web_events(event_id,actor_id,actor_epoch,session_digest) VALUES(?1,?2,?3,?4)";
            // Only the latest-event clause separates an earlier CLI event from the latest one.
            let stale = crate::auth::with_domain_request(owner.clone(), || {
                db.execute(insert, params![1, owner.account_id(), owner.epoch(), owner.digest()])
            });
            assert!(stale.is_err(), "earlier CLI event gained browser attribution");
            let latest = crate::auth::with_domain_request(owner.clone(), || {
                db.execute(insert, params![2, owner.account_id(), owner.epoch(), owner.digest()])
            });
            assert_eq!(latest?, 1);
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(counts(&store).await, [2, 3, 1, 2, 1]);
}

#[tokio::test]
async fn ruleset_stale_csrf_admission_rejects_registration_with_valid_owner() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    store
        .register_ruleset(ruleset("csrf-game", "CSRF game", "first", "First"))
        .await
        .unwrap();
    let before = counts(&store).await;
    // The Owner binding stays valid; only the in-transaction CSRF recheck can refuse.
    let owner = auth[0].clone();
    store
        .run(move |db| owner.recheck_owner(db).map(|_| ()))
        .await
        .unwrap();
    let ruleset_result = store
        .owner_register_ruleset(
            auth[0].clone(),
            auth[0].stale_csrf_fixture(),
            ruleset("csrf-game", "CSRF game", "second", "Second"),
        )
        .await;
    assert!(matches!(ruleset_result, Err(AppError::NotFound)));
    let alias_result = store
        .owner_register_alias(
            auth[0].clone(),
            auth[0].stale_csrf_fixture(),
            RegisterAlias {
                game_id: "csrf-game".into(),
                edition_id: "first".into(),
                legacy_edition: "Stale alias".into(),
            },
        )
        .await;
    assert!(matches!(alias_result, Err(AppError::NotFound)));
    assert_eq!(counts(&store).await, before);
}
