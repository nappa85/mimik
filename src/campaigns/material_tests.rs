//! Synthetic material acceptance; no user corpus is accessed.
use super::*;
use axum::http::StatusCode;

async fn seed(store: &Store) {
    store.run(|db|{
        db.execute("INSERT INTO sources(id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages) VALUES(1,'fixture','fixture','Material <title>','mork-borg','Fixture edition','en',?1,x'',x'',x'',2)",["a".repeat(64)])?;
        db.execute("INSERT INTO source_pages(id,source_id,pdf_page,png) VALUES(1,1,1,x'89504e47')",[])?;
        for id in 1..=51 {db.execute("INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(?1,1,?2,'Text','[]','<p>Fixture</p><script>bad()</script>','Fixture')",params![id,format!("unit-{id}")])?;}
        // Raw fixture rows get derived search text exactly as a legacy upgrade would.
        crate::sources::search::ensure(db)?;
        Ok(())
    }).await.unwrap();
}
/// Raw HTML is immutable (migration trigger); fixtures replace the unreviewed unit instead.
fn replace_unit(db: &rusqlite::Connection, block: &str, html: &str) -> Result<(), AppError> {
    db.execute(
        "DELETE FROM source_units WHERE id=1 AND state='unreviewed'",
        [],
    )?;
    db.execute("INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(1,1,?1,'Text','[]',?2,'Fixture')", params![block, html])?;
    crate::sources::search::ensure(db)?;
    Ok(())
}
fn decision(revision: i64) -> Vec<(String, String)> {
    vec![
        ("csrf".into(), "a".repeat(64)),
        ("revision".into(), revision.to_string()),
        ("state".into(), "corrected".into()),
        ("correction".into(), "<unsaved correction>".into()),
        ("rationale".into(), "<checked original>".into()),
    ]
}
async fn body(response: axum::response::Response) -> String {
    String::from_utf8(
        axum::body::to_bytes(response.into_body(), 16 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

async fn document_post(
    app: &axum::Router,
    auth: &RequestAuth,
    form: Vec<(String, String)>,
) -> axum::response::Response {
    document_at(app, auth, "/admin/materials/pages/1/units/1", form).await
}
async fn document_at(
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

#[tokio::test]
async fn material_document_conflict_validation_and_nul_retain_escaped_drafts() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    for (revision, state, status) in [
        (0, "corrected", StatusCode::CONFLICT),
        (1, "invalid", StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        let mut form = decision(revision);
        form[2].1 = state.into();
        let response = document_post(&app, &auth[0], form).await;
        assert_eq!(response.status(), status);
        let html = body(response).await;
        assert!(html.contains("Your draft was not saved"), "{html}");
        assert!(html.contains(&format!("Submitted revision {revision}")));
        assert!(!html.contains("<unsaved correction>"));
        assert!(
            html.contains("&#60;unsaved correction&#62;")
                || html.contains("&lt;unsaved correction&gt;")
        );
        assert!(html.contains("checked original"));
    }
    let mut form = decision(1);
    form[3].1 = "\0retained NUL correction".into();
    let response = document_post(&app, &auth[0], form).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = body(response).await;
    assert!(html.contains("retained NUL correction"));
    assert!(html.contains("Submitted revision 1"));
    for revision in [0, 1] {
        let form = vec![
            ("csrf".into(), "a".repeat(64)),
            ("revision".into(), revision.to_string()),
            ("printed_label".into(), "<printed draft>".into()),
            ("rationale".into(), "<whole page rationale>".into()),
        ];
        let response = document_at(&app, &auth[0], "/admin/materials/pages/1", form).await;
        assert_eq!(
            response.status(),
            if revision == 0 {
                StatusCode::CONFLICT
            } else {
                StatusCode::UNPROCESSABLE_ENTITY
            }
        );
        let html = body(response).await;
        assert!(html.contains("Your draft was not saved"));
        assert!(html.contains(&format!("Submitted page revision {revision}")));
        assert!(html.contains("whole page rationale"));
        assert!(!html.contains("value=\"<printed draft>\""));
    }
}

#[tokio::test]
async fn material_completion_checks_offscreen_units_and_invalidates_after_review() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    for id in 1..=50 {
        store
            .material_review_unit(
                auth[0].clone(),
                auth[0].csrf_fixture(),
                1,
                id,
                crate::sources::UnitReview {
                    revision: 1,
                    state: "accepted".into(),
                    correction: String::new(),
                    actor: String::new(),
                    rationale: "Compared synthetic original".into(),
                },
            )
            .await
            .unwrap();
    }
    let complete = |revision| crate::sources::PageReview {
        revision,
        printed_label: "1".into(),
        actor: String::new(),
        rationale: "Whole page omissions checked".into(),
    };
    assert!(matches!(
        store
            .material_complete(auth[0].clone(), auth[0].csrf_fixture(), 1, complete(51))
            .await,
        Err(AppError::Invalid(_))
    ));
    store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            51,
            crate::sources::UnitReview {
                revision: 1,
                state: "rejected".into(),
                correction: String::new(),
                actor: String::new(),
                rationale: "Compared last unit".into(),
            },
        )
        .await
        .unwrap();
    store
        .material_complete(auth[0].clone(), auth[0].csrf_fixture(), 1, complete(52))
        .await
        .unwrap();
    let page = store
        .material_page(auth[0].clone(), 1, 50, 50)
        .await
        .unwrap();
    assert!(page.page.complete);
    assert_eq!(page.page.revision, 53);
    assert_eq!(page.page.events.len(), 2);
    assert_eq!(page.history_next, None);
    store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            1,
            crate::sources::UnitReview {
                revision: 2,
                state: "corrected".into(),
                correction: "Recompared source".into(),
                actor: String::new(),
                rationale: "Changed source decision".into(),
            },
        )
        .await
        .unwrap();
    let page = store.material_page(auth[0].clone(), 1, 0, 0).await.unwrap();
    assert!(!page.page.complete);
    assert_eq!(page.page.revision, 54);
    assert_eq!(page.history_next, Some(50));
}

#[tokio::test]
async fn material_final_admission_failure_rolls_back_unit_event_and_sidecar() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, clock) = fixture().await;
    seed(&store).await;
    store.run(move|db|{db.create_scalar_function("material_test_expire",0,rusqlite::functions::FunctionFlags::SQLITE_UTF8,move |_|{clock.store(1000,Ordering::SeqCst);Ok(1)})?;db.execute_batch("CREATE TRIGGER material_final_expiry AFTER INSERT ON source_web_events BEGIN SELECT material_test_expire(); END;")?;Ok(())}).await.unwrap();
    let result = store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            1,
            crate::sources::UnitReview {
                revision: 1,
                state: "accepted".into(),
                correction: String::new(),
                rationale: "Compared".into(),
                actor: String::new(),
            },
        )
        .await;
    assert!(matches!(result, Err(AppError::NotFound)));
    store
        .run(|db| {
            for table in ["source_review_events", "source_web_events"] {
                assert_eq!(
                    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                        .get::<_, i64>(0))?,
                    0
                );
            }
            assert_eq!(
                db.query_row("SELECT revision FROM source_units WHERE id=1", [], |r| r
                    .get::<_, i64>(0))?,
                1
            );
            assert_eq!(
                db.query_row("SELECT revision FROM source_pages WHERE id=1", [], |r| r
                    .get::<_, i64>(0))?,
                1
            );
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn material_crop_uses_fresh_authority_after_cpu_work() {
    let _lock = crate::auth::test_resource_lock().await;
    let (dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 4, 4);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[0; 48])
            .unwrap();
    }
    store
        .run(move |db| {
            db.execute("UPDATE source_pages SET png=?1 WHERE id=1", [bytes])?;
            db.execute(
                "UPDATE sources SET prepared_json=?1 WHERE id=1",
                [br#"{"pages":[{"pdf_page":1,"width_points":4,"height_points":4}]}"#.to_vec()],
            )?;
            db.execute(
                "UPDATE source_units SET polygon='[[0,0],[4,0],[4,4],[0,4]]' WHERE id=1",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    assert_eq!(
        router_request(
            &app,
            &auth[0],
            "/admin/materials/pages/1/units/1/image",
            None
        )
        .await
        .status(),
        StatusCode::OK
    );
    let path = dir.path().join("private.sqlite3");
    *crate::sources::materials::AFTER_CROP.lock().unwrap() = Some(Box::new(move || {
        let db = rusqlite::Connection::open(&path).unwrap();
        crate::store::register_auth_clock(&db, Arc::new(|| Ok(100))).unwrap();
        db.execute(
            "UPDATE auth_sessions SET state='revoked',payload=NULL WHERE account_id=1",
            [],
        )
        .unwrap();
    }));
    let response = router_request(
        &app,
        &auth[0],
        "/admin/materials/pages/1/units/1/image",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_ne!(
        response
            .headers()
            .get("content-type")
            .map(axum::http::HeaderValue::as_bytes),
        Some(b"image/png".as_slice())
    );
}

#[tokio::test]
async fn material_schema14_upgrade_preserves_source_account_and_private_history() {
    let _lock = crate::auth::test_resource_lock().await;
    let (dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Retained campaign".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    let saved = insert_native(&store, auth[1].clone(), native(campaign, 2)).await;
    crate::accounts::Accounts::new(store.clone())
        .browser_invite(
            auth[0].clone(),
            "retained".into(),
            "Retained account".into(),
            0,
        )
        .await
        .unwrap();
    store
        .review_source_unit(
            1,
            1,
            crate::sources::UnitReview {
                revision: 1,
                state: "corrected".into(),
                correction: "Retained correction".into(),
                actor: "local reviewer".into(),
                rationale: "Retained rationale".into(),
            },
        )
        .await
        .unwrap();
    store.run(|db|{assert_eq!(db.query_row("SELECT count(*) FROM source_web_events",[],|r|r.get::<_,i64>(0))?,0);crate::sources::search::drop_schema(db)?;db.execute_batch("DROP TABLE registry_web_events; DROP TABLE registry_events; DROP TABLE source_web_events; DROP INDEX source_review_page_history; PRAGMA user_version=14;")?;Ok(())}).await.unwrap();
    let reopened = Store::open(&dir.path().join("private.sqlite3")).unwrap();
    reopened
        .run(|db| {
            crate::store::register_auth_clock(db, Arc::new(|| Ok(100)))?;
            assert_eq!(
                db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
                crate::store::SCHEMA_VERSION
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM account_web_events", [], |r| r
                    .get::<_, i64>(0))?,
                1
            );
            assert_eq!(
                db.query_row("SELECT actor FROM source_review_events", [], |r| r
                    .get::<_, String>(0))?,
                "local reviewer"
            );
            Ok(())
        })
        .await
        .unwrap();
    let data = reopened
        .material_page(auth[0].clone(), 1, 0, 0)
        .await
        .unwrap();
    assert_eq!(data.page.units[0].correction, "Retained correction");
    assert_eq!(data.page.units[0].revision, 2);
    let (view, permit) = reopened
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
}

#[tokio::test]
async fn material_directory_cursors_are_independent_and_exhaustive() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    store.run(|db|{
        for id in 2..=51{
            db.execute("INSERT INTO sources SELECT ?1,?2,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages FROM sources WHERE id=1",params![id,format!("synthetic-{id}")])?;
            db.execute("INSERT INTO source_pages(id,source_id,pdf_page,png) VALUES(?1,1,?1,x'')",[id])?;
        }Ok(())
    }).await.unwrap();
    let first = store.material_sources(auth[0].clone(), 0).await.unwrap();
    assert_eq!(first.rows.len(), 50);
    assert_eq!(first.next, Some(50));
    assert_eq!(first.rows[0].imported, 51);
    let last = store.material_sources(auth[0].clone(), 50).await.unwrap();
    assert_eq!(last.rows[0].id, 51);
    assert_eq!(last.next, None);
    let first = store.material_pages(auth[0].clone(), 1, 0).await.unwrap();
    assert_eq!(first.rows.len(), 50);
    assert_eq!(first.next, Some(50));
    let last = store.material_pages(auth[0].clone(), 1, 50).await.unwrap();
    assert_eq!(last.rows[0].id, 51);
    assert_eq!(last.next, None);
}

#[tokio::test]
async fn material_maximum_metadata_and_history_byte_guards() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    store
        .run(|db| {
            db.execute(
                "UPDATE sources SET title=?1,edition=?2 WHERE id=1",
                params!["x".repeat(200), "🦀".repeat(100)],
            )?;
            replace_unit(db, &"x".repeat(512), &"x".repeat(65536))?;
            Ok(())
        })
        .await
        .unwrap();
    let data = store.material_page(auth[0].clone(), 1, 0, 0).await.unwrap();
    assert_eq!(data.page.edition.len(), 400);
    assert_eq!(data.page.units[0].block_id.len(), 512);
    assert!(store.material_sources(auth[0].clone(), 0).await.is_ok());
    store.run(|db|{db.execute("UPDATE source_units SET state='accepted',revision=revision+1 WHERE id=1",[])?;db.execute("INSERT INTO source_review_events(page_id,unit_id,actor,rationale,state,correction,previous_revision) VALUES(1,1,'local',?1,'accepted','',1)",[format!("\0{}","x".repeat(20000))])?;Ok(())}).await.unwrap();
    assert!(store.material_page(auth[0].clone(), 1, 0, 0).await.is_err());
}

#[tokio::test]
async fn material_authenticated_decision_invalidates_immutable_published_evidence() {
    let _lock = crate::auth::test_resource_lock().await;
    let _runtime = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let version = published_fixture(&store).await;
    let before = store.definition_version(version).await.unwrap();
    assert_eq!(
        before.wiring_status,
        crate::definitions::WiringStatus::Current
    );
    store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            1,
            crate::sources::UnitReview {
                revision: 1,
                state: "corrected".into(),
                correction: "Source-compared correction".into(),
                rationale: "Rechecked synthetic original".into(),
                actor: String::new(),
            },
        )
        .await
        .unwrap();
    let after = store.definition_version(version).await.unwrap();
    assert_eq!(after.wiring_status, crate::definitions::WiringStatus::Stale);
    assert_eq!(before.approval_id, after.approval_id);
    assert_eq!(before.content_hash, after.content_hash);
}

#[tokio::test]
async fn material_missing_wrong_and_revoked_csrf_cannot_change_either_review_kind() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    for path in [
        "/admin/materials/pages/1",
        "/admin/materials/pages/1/units/1",
    ] {
        let form = if path.ends_with("units/1") {
            decision(1)
        } else {
            vec![
                ("csrf".into(), "a".repeat(64)),
                ("revision".into(), "1".into()),
                ("printed_label".into(), "1".into()),
                ("rationale".into(), "Compared original".into()),
            ]
        };
        let mut missing = form.clone();
        missing.retain(|r| r.0 != "csrf");
        assert_eq!(
            router_request(&app, &auth[0], path, Some(missing))
                .await
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let mut wrong = form;
        wrong.iter_mut().find(|r| r.0 == "csrf").unwrap().1 = "wrong".into();
        assert_eq!(
            router_request(&app, &auth[0], path, Some(wrong))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
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
    assert_eq!(
        router_request(
            &app,
            &auth[0],
            "/admin/materials/pages/1/units/1",
            Some(decision(1))
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    store
        .run(|db| {
            assert_eq!(
                db.query_row("SELECT count(*) FROM source_review_events", [], |r| r
                    .get::<_, i64>(0))?,
                0
            );
            assert_eq!(
                db.query_row("SELECT revision FROM source_pages WHERE id=1", [], |r| r
                    .get::<_, i64>(0))?,
                1
            );
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn material_router_owner_matrix_unicode_limit_and_atomic_attribution() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture_with_owner(101).await;
    seed(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    for path in [
        "/admin/materials",
        "/admin/materials/sources/1",
        "/admin/materials/pages/1",
        "/admin/materials/pages/1/image",
        "/admin/materials/pages/1/units/1/image",
    ] {
        assert_eq!(
            router_request(&app, &auth[1], path, None).await.status(),
            StatusCode::FORBIDDEN
        );
    }
    let page = body(router_request(&app, &auth[0], "/admin/materials/pages/1", None).await).await;
    assert!(page.contains("Material &#60;title&#62;") || page.contains("Material &lt;title&gt;"));
    assert!(!page.contains("<script>bad()"));
    assert!(page.contains("Next units"));
    let mut large = decision(1);
    large[3].1 = "🦀".repeat(20000);
    large[4].1 = "🦀".repeat(5000);
    assert_eq!(
        router_request(
            &app,
            &auth[0],
            "/admin/materials/pages/1/units/1",
            Some(large)
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    store
        .run(|db| {
            assert_eq!(
                db.query_row(
                    "SELECT actor FROM source_review_events WHERE id=1",
                    [],
                    |r| r.get::<_, String>(0)
                )?,
                "account:101"
            );
            assert_eq!(
                db.query_row(
                    "SELECT actor_id FROM source_web_events WHERE event_id=1",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                101
            );
            for sql in [
                "DELETE FROM source_web_events",
                "UPDATE source_web_events SET actor_epoch=2",
                "INSERT OR REPLACE INTO source_web_events SELECT * FROM source_web_events",
            ] {
                assert!(db.execute(sql, []).is_err());
            }
            Ok(())
        })
        .await
        .unwrap();
    let mut hostile = decision(2);
    hostile.push(("actor".into(), "someone else".into()));
    assert_eq!(
        router_request(
            &app,
            &auth[0],
            "/admin/materials/pages/1/units/1",
            Some(hostile)
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut excessive = decision(2);
    excessive[3].1 = "x".repeat(330_000);
    assert_eq!(
        router_request(
            &app,
            &auth[0],
            "/admin/materials/pages/1/units/1",
            Some(excessive)
        )
        .await
        .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_only_first_material_decision_was_saved(&store).await;
}

async fn assert_only_first_material_decision_was_saved(store: &Store) {
    store
        .run(|db| {
            assert_eq!(
                db.query_row("SELECT revision FROM source_units WHERE id=1", [], |r| r
                    .get::<_, i64>(0))?,
                2
            );
            assert_eq!(
                db.query_row("SELECT revision FROM source_pages WHERE id=1", [], |r| r
                    .get::<_, i64>(0))?,
                2
            );
            for table in ["source_review_events", "source_web_events"] {
                assert_eq!(
                    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                        .get::<_, i64>(0))?,
                    1
                );
            }
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn material_queued_expiry_and_revocation_reject_both_mutation_kinds() {
    let _lock = crate::auth::test_resource_lock().await;
    for complete in [false, true] {
        for revoke in [false, true] {
            let (_dir, store, auth, clock) = fixture().await;
            seed(&store).await;
            let (entered, waiting) = tokio::sync::oneshot::channel();
            let (release, released) = std::sync::mpsc::channel();
            let locking = store.clone();
            let lock = tokio::spawn(async move {
                locking.run(move|db|{entered.send(()).unwrap();released.recv_timeout(std::time::Duration::from_secs(5)).unwrap();if revoke{db.execute("UPDATE auth_sessions SET state='revoked',payload=NULL WHERE account_id=1",[])?;}Ok(())}).await
            });
            waiting.await.unwrap();
            let saving = store.clone();
            let captured = auth[0].clone();
            let write = tokio::spawn(async move {
                if complete {
                    saving
                        .material_complete(
                            captured.clone(),
                            captured.csrf_fixture(),
                            1,
                            crate::sources::PageReview {
                                revision: 1,
                                printed_label: "1".into(),
                                rationale: "Checked".into(),
                                actor: String::new(),
                            },
                        )
                        .await
                } else {
                    saving
                        .material_review_unit(
                            captured.clone(),
                            captured.csrf_fixture(),
                            1,
                            1,
                            crate::sources::UnitReview {
                                revision: 1,
                                state: "accepted".into(),
                                correction: String::new(),
                                rationale: "Checked".into(),
                                actor: String::new(),
                            },
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
            store
                .run(|db| {
                    assert_eq!(
                        db.query_row("SELECT revision FROM source_pages WHERE id=1", [], |r| r
                            .get::<_, i64>(0))?,
                        1
                    );
                    assert_eq!(
                        db.query_row("SELECT count(*) FROM source_web_events", [], |r| r
                            .get::<_, i64>(0))?,
                        0
                    );
                    Ok(())
                })
                .await
                .unwrap();
        }
    }
}

#[tokio::test]
async fn material_registry_ids_are_byte_bounded_including_nul() {
    let _lock = crate::auth::test_resource_lock().await;
    for (game, edition) in [
        ("\0".to_owned() + &"g".repeat(65), "edition".into()),
        ("game".into(), "\0".to_owned() + &"e".repeat(65)),
    ] {
        let (_dir, store, auth, _clock) = fixture().await;
        seed(&store).await;
        store
            .run(move |db| {
                db.execute("INSERT INTO registry_games VALUES(?1,'Synthetic')", [&game])?;
                db.execute(
                    "INSERT INTO registry_editions VALUES(?1,?2,'Synthetic',0)",
                    params![game, edition],
                )?;
                db.execute(
                    "INSERT INTO source_rulesets VALUES(1,?1,?2)",
                    params![game, edition],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(
            store.material_sources(auth[0].clone(), 0).await.is_err(),
            "oversized present registry identity must fail"
        );
    }
}

#[tokio::test]
async fn material_nul_validation_is_structured_and_does_not_mutate() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    for nul in ["\0content", "con\0tent"] {
        for field in 0..2 {
            let review = crate::sources::UnitReview {
                revision: 1,
                state: "corrected".into(),
                correction: if field == 0 {
                    nul.into()
                } else {
                    "good".into()
                },
                rationale: if field == 1 {
                    nul.into()
                } else {
                    "checked".into()
                },
                actor: String::new(),
            };
            assert!(matches!(
                store
                    .material_review_unit(auth[0].clone(), auth[0].csrf_fixture(), 1, 1, review)
                    .await,
                Err(AppError::Invalid(_))
            ));
        }
        assert!(matches!(
            store
                .material_complete(
                    auth[0].clone(),
                    auth[0].csrf_fixture(),
                    1,
                    crate::sources::PageReview {
                        revision: 1,
                        printed_label: nul.into(),
                        rationale: "checked".into(),
                        actor: String::new()
                    }
                )
                .await,
            Err(AppError::Invalid(_))
        ));
    }
    store
        .run(|db| {
            assert_eq!(
                db.query_row("SELECT revision FROM source_units WHERE id=1", [], |r| r
                    .get::<_, i64>(0))?,
                1
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM source_review_events", [], |r| r
                    .get::<_, i64>(0))?,
                0
            );
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn material_region_guards_report_storage_type_and_polygon_overflow() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    for sql in [
        "UPDATE source_pages SET png='text' WHERE id=1",
        "UPDATE sources SET prepared_json='text' WHERE id=1",
    ] {
        store
            .run(move |db| {
                db.execute(sql, [])?;
                Ok(())
            })
            .await
            .unwrap();
        let result = store
            .material_region_data(
                auth[0].clone(),
                1,
                1,
                crate::sources::region::admit().unwrap(),
            )
            .await;
        assert!(
            matches!(result, Err(AppError::Invalid(_))),
            "invalid stored region type must fail before crop"
        );
        store
            .run(|db| {
                db.execute("UPDATE source_pages SET png=x'89504e47' WHERE id=1", [])?;
                db.execute("UPDATE sources SET prepared_json=x'' WHERE id=1", [])?;
                Ok(())
            })
            .await
            .unwrap();
    }
    store
        .run(|db| {
            db.execute(
                "UPDATE source_units SET polygon=?1 WHERE id=1",
                [format!("\0{}", "x".repeat(1024))],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(matches!(
        store
            .material_region_data(
                auth[0].clone(),
                1,
                1,
                crate::sources::region::admit().unwrap()
            )
            .await,
        Err(AppError::Invalid(_))
    ));
}

#[tokio::test]
async fn material_pagination_completeness_guard_and_audit_rollback() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let data = store.material_page(auth[0].clone(), 1, 0, 0).await.unwrap();
    assert_eq!(data.page.units.len(), 50);
    assert_eq!(data.units_next, Some(50));
    let data = store
        .material_page(auth[0].clone(), 1, 50, 0)
        .await
        .unwrap();
    assert_eq!(data.page.units[0].id, 51);
    assert_eq!(data.units_next, None);
    assert!(
        store
            .material_page(auth[0].clone(), 1, -1, 0)
            .await
            .is_err()
    );
    assert!(
        store
            .material_complete(
                auth[0].clone(),
                auth[0].csrf_fixture(),
                1,
                crate::sources::PageReview {
                    revision: 1,
                    printed_label: "1".into(),
                    rationale: "Full page checked".into(),
                    actor: String::new()
                }
            )
            .await
            .is_err()
    );
    store.run(|db|{db.execute_batch("CREATE TRIGGER material_audit_fault BEFORE INSERT ON source_web_events BEGIN SELECT RAISE(ABORT,'fixture fault'); END;")?;Ok(())}).await.unwrap();
    let review = crate::sources::UnitReview {
        revision: 1,
        state: "accepted".into(),
        correction: String::new(),
        rationale: "Compared original".into(),
        actor: "forged".into(),
    };
    assert!(
        store
            .material_review_unit(auth[0].clone(), auth[0].csrf_fixture(), 1, 1, review)
            .await
            .is_err()
    );
    store
        .run(|db| {
            assert_eq!(
                db.query_row("SELECT revision FROM source_units WHERE id=1", [], |r| r
                    .get::<_, i64>(0))?,
                1
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM source_review_events", [], |r| r
                    .get::<_, i64>(0))?,
                0
            );
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn material_byte_guards_csrf_and_revocation_fail_closed() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, clock) = fixture().await;
    seed(&store).await;
    store
        .run(|db| {
            replace_unit(db, "unit-1", &format!("\0{}", "x".repeat(65536)))?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(store.material_page(auth[0].clone(), 1, 0, 0).await.is_err());
    store
        .run(|db| {
            replace_unit(db, "unit-1", &"x".repeat(65536))?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(store.material_page(auth[0].clone(), 1, 0, 0).await.is_ok());
    let app = crate::auth::app(store.clone()).await.unwrap();
    let mut form = decision(1);
    form[0].1 = "wrong".into();
    assert_eq!(
        router_request(
            &app,
            &auth[0],
            "/admin/materials/pages/1/units/1",
            Some(form)
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    clock.store(1000, Ordering::SeqCst);
    assert!(store.material_image(auth[0].clone(), 1).await.is_err());
    assert!(store.material_recheck(auth[0].clone()).await.is_err());
}

#[tokio::test]
async fn material_review_path_maintains_the_search_index() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let fts = |store: &Store| {
        let store = store.clone();
        async move {
            store
                .run(|db| {
                    let mut query = db.prepare(
                        "SELECT rowid,body FROM source_search_fts WHERE rowid=1 ORDER BY rowid",
                    )?;
                    let rows = query
                        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(rows)
                })
                .await
                .unwrap()
        }
    };
    assert!(fts(&store).await.is_empty());
    for (revision, state, correction, expected) in [
        (1, "accepted", "", Some("Fixture\n")),
        (2, "corrected", "Owner correction", Some("Owner correction")),
        (3, "rejected", "", None),
    ] {
        store
            .material_review_unit(
                auth[0].clone(),
                auth[0].csrf_fixture(),
                1,
                1,
                crate::sources::UnitReview {
                    revision,
                    state: state.into(),
                    correction: correction.into(),
                    actor: String::new(),
                    rationale: "Compared synthetic original".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            fts(&store).await,
            expected
                .map(|body| vec![(1, body.to_owned())])
                .unwrap_or_default()
        );
    }
}
