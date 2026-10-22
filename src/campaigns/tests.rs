use super::{
    snapshots::{self, Pin, PrivateSnapshot},
    values,
};
use crate::{
    AppError, Store,
    auth::{RequestAuth, SqliteSessions},
};
use rusqlite::params;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
};
use tower_sessions::{
    SessionStore,
    session::{Id, Record},
};
#[path = "administration_tests.rs"]
mod administration_tests;
#[path = "assist_tests.rs"]
mod assist_tests;
#[path = "assistant_admin_tests.rs"]
mod assistant_admin_tests;
#[path = "catalogue_tests.rs"]
mod catalogue_tests;
#[path = "cell_tests.rs"]
mod cell_tests;
#[path = "entry_save_tests.rs"]
mod entry_save_tests;
#[path = "formula_tests.rs"]
mod formula_tests;
#[path = "interpret_route_tests.rs"]
mod interpret_route_tests;
#[path = "list_tests.rs"]
pub(super) mod list_tests;
#[path = "list_values_tests.rs"]
mod list_values_tests;
#[path = "material_tests.rs"]
mod material_tests;
#[path = "notes_tests.rs"]
mod notes_tests;
#[path = "quick_tests.rs"]
mod quick_tests;
#[path = "registry_tests.rs"]
mod registry_tests;
#[path = "script_tests.rs"]
mod script_tests;
#[path = "unit_tests.rs"]
mod unit_tests;
#[path = "unknown_tests.rs"]
mod unknown_tests;
#[path = "validity_tests.rs"]
mod validity_tests;

async fn fixture() -> (tempfile::TempDir, Store, Vec<RequestAuth>, Arc<AtomicI64>) {
    fixture_with_owner(1).await
}

async fn fixture_with_owner(
    owner: i64,
) -> (tempfile::TempDir, Store, Vec<RequestAuth>, Arc<AtomicI64>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("private.sqlite3")).unwrap();
    let clock = Arc::new(AtomicI64::new(100));
    let c = clock.clone();
    store.run(move|db|{
        crate::store::register_auth_clock(db,Arc::new(move||Ok(c.load(Ordering::SeqCst))))?;
        for id in 1..=5 {db.execute("INSERT INTO accounts(id,login,display_name,state,epoch,verifier) VALUES(?1,?2,?2,'enabled',1,?3)",params![id,format!("player{id}"),"$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"])?;}
        if owner!=1 {db.execute("INSERT INTO accounts(id,login,display_name,state,epoch,verifier) SELECT ?1,'high-owner','High Owner',state,epoch,verifier FROM accounts WHERE id=1",[owner])?;}
        db.execute("INSERT INTO account_owner VALUES(1,?1)",[owner])?;Ok(())
    }).await.unwrap();
    let sessions = SqliteSessions::new(store.clone());
    let mut auths = Vec::new();
    for account in [owner, 2, 3, 4, 5] {
        let mut data = HashMap::new();
        data.insert("csrf".into(), serde_json::json!("a".repeat(64)));
        data.insert(
            "principal".into(),
            serde_json::json!({"id":account,"epoch":1}),
        );
        let mut record = Record {
            id: Id::default(),
            data,
            expiry_date: time::OffsetDateTime::from_unix_timestamp(1000).unwrap(),
        };
        sessions.create(&mut record).await.unwrap();
        auths.push(RequestAuth::fixture(account, record.id, 1000));
    }
    (dir, store, auths, clock)
}

fn native(campaign: i64, owner: i64) -> PrivateSnapshot {
    PrivateSnapshot {
        encoding: 1,
        sheet_id: 1,
        campaign_id: campaign,
        game_id: "mork-borg".into(),
        edition_id: "it-2025".into(),
        custodian_id: owner,
        kind: "native".into(),
        pin: Pin::Native {
            adapter: "mork-borg-it-2025".into(),
            schema: 1,
            calculation_version: 1,
        },
        revision: 0,
        state: "active".into(),
        name: "Private <name>".into(),
        notes: "Retain exact notes".into(),
        data: serde_json::to_value(crate::mork_borg::Sheet::default()).unwrap(),
    }
}

async fn insert_native(
    store: &Store,
    auth: RequestAuth,
    snapshot: PrivateSnapshot,
) -> PrivateSnapshot {
    store
        .run_campaign(auth.clone(), move |db| {
            let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let result = snapshots::write(&tx, &auth, snapshot, "create", None)?;
            tx.commit()?;
            Ok(result)
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn privacy_roles_noop_history_freeze_and_sticky_rejoin() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let id = store
        .create_campaign(
            auth[1].clone(),
            "Campaign".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    store.request_join(auth[2].clone(), id).await.unwrap();
    store
        .decide_join(auth[1].clone(), id, 3, 1, true)
        .await
        .unwrap();
    let saved = insert_native(&store, auth[2].clone(), native(id, 3)).await;
    assert_eq!(saved.revision, 1);
    assert!(store.list().await.unwrap().is_empty());
    assert_outsiders_denied(&store, &[auth[0].clone(), auth[3].clone(), auth[4].clone()]).await;
    let (view, permit) = store
        .private_read(
            auth[2].clone(),
            1,
            None,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    store
        .private_save(auth[2].clone(), view, permit)
        .await
        .unwrap();
    assert_eq!(
        store
            .private_history(auth[1].clone(), 1, 0)
            .await
            .unwrap()
            .len(),
        2
    );
    store
        .change_role(auth[1].clone(), id, 3, 1, true)
        .await
        .unwrap();
    assert!(
        store
            .change_role(auth[2].clone(), id, 2, 1, false)
            .await
            .is_err()
    );
    assert!(
        store.depart(auth[1].clone(), id, 3, 2).await.is_err(),
        "additional Master must be demoted before removal"
    );
    store.depart(auth[2].clone(), id, 3, 2).await.unwrap();
    let (view, permit) = store
        .private_read(
            auth[2].clone(),
            1,
            None,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    drop(permit);
    assert!(!view.editable);
    assert_eq!(view.snapshot.state, "frozen");
    assert_eq!(view.snapshot.revision, 3);
    assert_eq!(view.snapshot.notes, saved.notes);
    store.request_join(auth[2].clone(), id).await.unwrap();
    store
        .decide_join(auth[1].clone(), id, 3, 3, true)
        .await
        .unwrap();
    assert_eq!(
        store
            .campaign_detail(auth[2].clone(), id, 0)
            .await
            .unwrap()
            .role,
        "player"
    );
    let (view, permit) = store
        .private_read(
            auth[2].clone(),
            1,
            None,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    drop(permit);
    assert!(!view.editable);
    assert!(store.depart(auth[1].clone(), id, 2, 1).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn regression_private_read_rechecks_grants_after_validation_gap() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Read boundary".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    for account in [3, 4] {
        store
            .request_join(
                auth[usize::try_from(account - 1).unwrap()].clone(),
                campaign,
            )
            .await
            .unwrap();
        store
            .decide_join(auth[1].clone(), campaign, account, 1, true)
            .await
            .unwrap();
    }
    store
        .change_role(auth[1].clone(), campaign, 3, 1, true)
        .await
        .unwrap();
    insert_native(&store, auth[3].clone(), native(campaign, 4)).await;
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let entered = std::sync::Mutex::new(Some(entered));
    let released = std::sync::Mutex::new(released);
    *super::sheets::AFTER_PRIVATE_VALIDATION.lock().unwrap() = Some(Arc::new(move || {
        entered.lock().unwrap().take().unwrap().send(()).unwrap();
        released
            .lock()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
    }));
    let reading_store = store.clone();
    let reading_auth = auth[2].clone();
    let permit = crate::definitions::admit().unwrap();
    let reading = tokio::spawn(async move {
        reading_store
            .private_read(reading_auth, 1, None, permit)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), waiting)
        .await
        .unwrap()
        .unwrap();
    store
        .change_role(auth[1].clone(), campaign, 3, 2, false)
        .await
        .unwrap();
    release.send(()).unwrap();
    let result = reading.await.unwrap();
    assert!(
        matches!(result, Err(AppError::NotFound)),
        "A demoted noncustodian retained a mixed-snapshot private view"
    );
}

#[tokio::test]
async fn regression_private_predecessor_is_bounded_before_json_processing() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "History bounds".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    let saved = insert_native(&store, auth[1].clone(), native(campaign, 2)).await;
    let (view, permit) = store
        .private_read(
            auth[1].clone(),
            1,
            None,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    store
        .private_save(auth[1].clone(), view, permit)
        .await
        .unwrap();
    let mut oversized = saved;
    oversized.notes = "x".repeat(snapshots::MAX_BYTES);
    for predecessor in [
        serde_json::to_string(&oversized).unwrap(),
        "{invalid predecessor".into(),
    ] {
        store.run(move |db| {
            let original: String = db.query_row("SELECT sql FROM sqlite_schema WHERE type='trigger' AND name='private_sheet_history_update'", [], |row|row.get(0))?;
            db.execute_batch("DROP TRIGGER private_sheet_history_update; PRAGMA ignore_check_constraints=ON;")?;
            db.execute("UPDATE private_sheet_history SET snapshot=?1 WHERE sheet_id=1 AND revision=1", [predecessor])?;
            db.execute_batch("PRAGMA ignore_check_constraints=OFF;")?;
            db.execute_batch(&original)?;
            Ok(())
        }).await.unwrap();
        assert!(
            store
                .private_read(
                    auth[1].clone(),
                    1,
                    None,
                    crate::definitions::admit().unwrap()
                )
                .await
                .is_err(),
            "Corrupt over-cap predecessor was accepted by a bounded private read"
        );
    }
}

async fn assert_outsiders_denied(store: &Store, outsiders: &[RequestAuth]) {
    for outsider in outsiders {
        assert!(matches!(
            store
                .private_read(
                    outsider.clone(),
                    1,
                    None,
                    crate::definitions::admit().unwrap()
                )
                .await,
            Err(AppError::NotFound)
        ));
        assert!(store.private_history(outsider.clone(), 1, 0).await.is_err());
    }
}

#[tokio::test]
async fn missing_history_commit_fails_and_last_freeze_failure_rolls_back() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let id = store
        .create_campaign(
            auth[1].clone(),
            "Campaign".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    store.request_join(auth[2].clone(), id).await.unwrap();
    store
        .decide_join(auth[1].clone(), id, 3, 1, true)
        .await
        .unwrap();
    insert_native(&store, auth[2].clone(), native(id, 3)).await;
    let a = auth[2].clone();
    assert!(
        store
            .run_campaign(a.clone(), move |db| {
                let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                a.recheck(&tx)?;
                tx.execute("UPDATE private_sheets SET revision=2 WHERE id=1", [])?;
                tx.commit()?;
                Ok(())
            })
            .await
            .is_err()
    );
    let mut second = native(id, 3);
    second.sheet_id = 2;
    insert_native(&store, auth[2].clone(), second).await;
    store.run(|db|{db.execute_batch("CREATE TRIGGER fail_last_history BEFORE INSERT ON private_sheet_history WHEN NEW.sheet_id=2 AND NEW.operation='freeze' BEGIN SELECT RAISE(ABORT,'synthetic disk failure'); END;")?;Ok(())}).await.unwrap();
    assert!(store.depart(auth[2].clone(), id, 3, 1).await.is_err());
    assert_eq!(
        store
            .campaign_detail(auth[2].clone(), id, 0)
            .await
            .unwrap()
            .role,
        "player"
    );
    for sheet in 1..=2 {
        let (view, permit) = store
            .private_read(
                auth[2].clone(),
                sheet,
                None,
                crate::definitions::admit().unwrap(),
            )
            .await
            .unwrap();
        drop(permit);
        assert_eq!(view.snapshot.revision, 1);
        assert_eq!(view.snapshot.state, "active");
    }
}

#[tokio::test]
async fn domain_clock_epoch_and_default_sql_guard_are_fail_closed() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, clock) = fixture().await;
    assert!(
        store
            .run(|db| {
                db.execute(
                    "INSERT INTO campaigns VALUES(1,1,'mork-borg','it-2025','Unauthorized',1)",
                    [],
                )?;
                Ok(())
            })
            .await
            .is_err()
    );
    clock.store(1000, Ordering::SeqCst);
    assert!(
        store
            .create_campaign(
                auth[1].clone(),
                "Expired".into(),
                "mork-borg".into(),
                "it-2025".into()
            )
            .await
            .is_err()
    );
    assert!(store.workspace(auth[1].clone(), 0).await.is_err());
    clock.store(100, Ordering::SeqCst);
    store
        .run(|db| {
            db.execute("UPDATE accounts SET epoch=2 WHERE id=2", [])?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(
        store
            .create_campaign(
                auth[1].clone(),
                "Old epoch".into(),
                "mork-borg".into(),
                "it-2025".into()
            )
            .await
            .is_err()
    );
}

#[test]
fn contextual_decoder_keeps_256_reserved_ids_and_old_definition_limits() {
    let mut object = serde_json::Map::new();
    for n in 0..253 {
        object.insert(format!("field-{n}"), serde_json::json!("v".repeat(8000)));
    }
    for key in ["title", "help", "label"] {
        object.insert(key.into(), serde_json::json!("v".repeat(8000)));
    }
    // Independently bounded body: use 1,000 scalars per field to fit the byte cap.
    for value in object.values_mut() {
        *value = serde_json::json!("v".repeat(1000));
    }
    let bytes = serde_json::to_vec(&serde_json::json!({"data":object})).unwrap();
    assert!(crate::definitions::decode::parse_sheet(&bytes).is_ok());
    assert!(
        crate::definitions::decode::parse_sheet(br#"{"data":{"title":"a","\u0074itle":"b"}}"#)
            .is_err()
    );
    for raw in ["+1", "01", "1e2", "1.0", "9223372036854775808"] {
        assert!(values::integer(raw).is_err());
    }
    for number in [i64::MIN, i64::MAX, 9_007_199_254_740_993] {
        assert_eq!(values::integer(&number.to_string()).unwrap(), number);
    }
}

#[tokio::test]
async fn regression_current_history_operation_corruption_is_rejected() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let id = store
        .create_campaign(
            auth[1].clone(),
            "Campaign".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    insert_native(&store, auth[1].clone(), native(id, 2)).await;
    store
        .run(|db| {
            let sql: String = db.query_row(
                "SELECT sql FROM sqlite_schema WHERE name='private_sheet_history_update'",
                [],
                |r| r.get(0),
            )?;
            db.execute_batch("DROP TRIGGER private_sheet_history_update;")?;
            db.execute(
                "UPDATE private_sheet_history SET operation='edit' WHERE sheet_id=1",
                [],
            )?;
            db.execute_batch(&sql)?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(
        store
            .private_read(
                auth[1].clone(),
                1,
                None,
                crate::definitions::admit().unwrap()
            )
            .await
            .is_err(),
        "Digest alone cannot establish event semantics"
    );
}

async fn router_request(
    app: &axum::Router,
    auth: &RequestAuth,
    path: &str,
    form: Option<Vec<(String, String)>>,
) -> axum::response::Response {
    use tower::ServiceExt;
    let mut req = axum::http::Request::builder()
        .uri(path)
        .header("host", "127.0.0.1:3000")
        .header("cookie", auth.fixture_cookie());
    let body = if let Some(form) = form {
        req = req
            .method("POST")
            .header("origin", "http://127.0.0.1:3000")
            .header("content-type", "application/x-www-form-urlencoded");
        serde_urlencoded::to_string(form).unwrap()
    } else {
        String::new()
    };
    app.clone()
        .oneshot(req.body(axum::body::Body::from(body)).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn actual_router_campaign_native_full_replace_escaped_drafts_private_legacy_denials() {
    use axum::http::StatusCode;
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    store.run(|db|{db.execute("INSERT INTO sources(bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages) VALUES('native-fixture','fixture','Fixture','mork-borg','Italian fixture','it','fixture',x'',x'',x'',1)",[])?;db.execute("INSERT INTO source_rulesets VALUES(1,'mork-borg','it-2025')",[])?;Ok(())}).await.unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    let create = vec![
        ("csrf".into(), "a".repeat(64)),
        ("title".into(), "Router campaign".into()),
        ("game_id".into(), "mork-borg".into()),
        ("edition_id".into(), "it-2025".into()),
    ];
    assert_eq!(
        router_request(&app, &auth[1], "/campaigns", Some(create))
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        router_request(
            &app,
            &auth[2],
            "/campaigns/1/join",
            Some(vec![("csrf".into(), "a".repeat(64))])
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        router_request(
            &app,
            &auth[1],
            "/campaigns/1/requests/3",
            Some(vec![
                ("csrf".into(), "a".repeat(64)),
                ("revision".into(), "1".into()),
                ("action".into(), "accept".into())
            ])
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let response = router_request(&app, &auth[2], "/campaigns/1/sheets/new", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut form = native_form();
    assert_eq!(
        router_request(
            &app,
            &auth[2],
            "/campaigns/1/sheets/new",
            Some(form.clone())
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_native_page_has_no_formula_markers(&app, &auth[2]).await;
    // An edit that changes something (an unchanged save writes nothing, D-U), so the
    // re-post below is stale.
    for pair in &mut form {
        match pair.0.as_str() {
            "revision" => pair.1 = "1".into(),
            "notes" => pair.1 = "Edited private notes".into(),
            _ => {}
        }
    }
    assert_eq!(
        router_request(&app, &auth[2], "/private-sheets/1", Some(form.clone()))
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let response = router_request(&app, &auth[2], "/private-sheets/1", Some(form.clone())).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(!response.headers().contains_key("set-cookie"));
    let html = String::from_utf8(
        axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("&#60;script&#62;name") || html.contains("&lt;script&gt;name"));
    assert!(!html.contains("<script>name"));
    assert!(html.contains("name=\"revision\" value=\"1\""));
    assert!(!html.contains("/sources"));
    assert_owner_router_denials(&app, &auth[0]).await;
    form.retain(|(key, _)| key != "description");
    assert_eq!(
        router_request(&app, &auth[2], "/private-sheets/1", Some(form))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert!(store.list().await.unwrap().is_empty());
}

/// Slice 1.5a (Sonnet m1): the per-field routes refuse native sheets (400, nothing written);
/// the native editor keeps its one full form.
#[tokio::test]
async fn native_sheets_refuse_unit_routes() {
    use axum::http::StatusCode;
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    store.run(|db|{db.execute("INSERT INTO sources(bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages) VALUES('native-fixture','fixture','Fixture','mork-borg','Italian fixture','it','fixture',x'',x'',x'',1)",[])?;db.execute("INSERT INTO source_rulesets VALUES(1,'mork-borg','it-2025')",[])?;Ok(())}).await.unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    let campaign = vec![
        ("csrf".into(), "a".repeat(64)),
        ("title".into(), "Native campaign".into()),
        ("game_id".into(), "mork-borg".into()),
        ("edition_id".into(), "it-2025".into()),
    ];
    let response = router_request(&app, &auth[1], "/campaigns", Some(campaign)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let response = router_request(
        &app,
        &auth[1],
        "/campaigns/1/sheets/new",
        Some(native_form()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let page = router_request(&app, &auth[1], "/private-sheets/1", None).await;
    let html = String::from_utf8(
        axum::body::to_bytes(page.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(
        html.contains("Save full private sheet"),
        "the full form stays"
    );
    assert!(!html.contains("/meta/name") && !html.contains("/fields/"));
    assert!(!html.contains("/cells/") && !html.contains("/lists/"));
    for path in [
        "/private-sheets/1/meta/name",
        "/private-sheets/1/meta/notes",
        "/private-sheets/1/fields/hp",
        // Slice 1.5b: a native sheet has no runtime lists either.
        "/private-sheets/1/cells/0/0/0",
    ] {
        let unit = vec![
            ("csrf".into(), "a".repeat(64)),
            ("base".into(), "1".into()),
            ("value".into(), "7".into()),
        ];
        let response = router_request(&app, &auth[1], path, Some(unit)).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
    }
    let revision: i64 = store
        .run(|db| {
            Ok(
                db.query_row("SELECT revision FROM private_sheets WHERE id=1", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .await
        .unwrap();
    assert_eq!(revision, 1, "nothing written");
}

/// Native sheets have no immutable pin, so nothing is evaluated or shown as advisory.
async fn assert_native_page_has_no_formula_markers(app: &axum::Router, auth: &RequestAuth) {
    use axum::http::StatusCode;
    let native_page = router_request(app, auth, "/private-sheets/1", None).await;
    assert_eq!(native_page.status(), StatusCode::OK);
    let native_html = String::from_utf8(
        axum::body::to_bytes(native_page.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    for marker in [
        "dvisory",
        "Computed",
        "formula-results",
        "How this is computed",
    ] {
        assert!(!native_html.contains(marker), "native page shows {marker}");
    }
    assert!(native_html.contains("<fieldset class=\"sheet-fieldset\">"));
    // No definition checks cover a native pin: never shown as validated (RR-3, ASSIST-4).
    assert!(native_html.contains(
        "<p id=\"validity\" class=\"validity\"><strong>Not validated</strong> (saved revision 1): no rule checks cover its interpretation pin. This flag never blocks saving.</p>"
    ));
}

async fn published_fixture(store: &Store) -> i64 {
    use serde_json::json;
    store.run(|db|{
        db.execute("INSERT INTO registry_games VALUES('synthetic-game','Synthetic')",[])?;db.execute("INSERT INTO registry_editions VALUES('synthetic-game','first','First',0)",[])?;
        let zeros="0".repeat(64);let prepared=json!({"contract_version":1,"extraction_sha256":zeros}).to_string();
        db.execute("INSERT INTO sources(bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages) VALUES(?1,'fixture','Fixture','synthetic-game','First','en',?1,x'',x'',?2,1)",params![zeros,prepared.as_bytes()])?;
        db.execute("INSERT INTO source_rulesets VALUES(1,'synthetic-game','first')",[])?;
        db.execute("INSERT INTO source_pages(id,source_id,pdf_page,png) VALUES(1,1,1,x'')",[])?;
        db.execute("INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(1,1,'block','text','[[0,0],[1,0],[1,1],[0,1]]','','')",[])?;
        crate::sources::search::ensure(db)?;
        db.execute("INSERT INTO source_page_hashes VALUES(1,1,?1)",[&zeros])?;db.execute("INSERT INTO source_finalizations VALUES(1,?1,?1)",[&zeros])?;Ok(())
    }).await.unwrap();
    let controls = [
        ("label", "text", false),
        ("large", "integer", false),
        ("flag", "boolean", true),
        ("story", "multiline", true),
    ];
    let fields=controls.iter().map(|(id,control,nullable)|json!({"id":id,"label":id,"control":control,"nullable":nullable,"mapping":{"state":"mapped","regions":[{"source_id":1,"page_id":1,"unit_id":1}]}})).collect::<Vec<_>>();
    let definition = json!({"contract_version":1,"game_id":"synthetic-game","edition_id":"first","definition_id":"sheet","title":"Generic <title>","sources":[{"source_id":1,"pdf_sha256":"0".repeat(64),"roles":["sheet"]}],"fields":fields,"groups":[{"id":"main","label":"Main","fields":["label","large","flag","story"],"column_hint":"main"}]});
    let draft = store
        .save_definition_draft(serde_json::to_vec(&definition).unwrap(), 0)
        .await
        .unwrap();
    let comparison = store.compare_definition(draft.id, false).await.unwrap();
    store
        .approve_definition(
            draft.id,
            false,
            crate::definitions::ApprovalRequest {
                comparison_hash: comparison.comparison_hash,
                actor: "Synthetic test".into(),
                rationale: "Synthetic original compared".into(),
                originals_compared: true,
            },
        )
        .await
        .unwrap()
        .id
}

fn native_form() -> Vec<(String, String)> {
    let mut form = serde_json::to_value(crate::mork_borg::Sheet::default())
        .unwrap()
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                match value {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Null => String::new(),
                    v => v.to_string(),
                },
            )
        })
        .collect::<Vec<_>>();
    form.extend([
        ("csrf".into(), "a".repeat(64)),
        ("name".into(), "Private <script>name</script>".into()),
        ("notes".into(), "Exact private notes".into()),
    ]);
    for (key, value) in &mut form {
        if key == "description" {
            *value = "Long native text ".repeat(600);
        }
    }
    form
}

async fn assert_owner_router_denials(app: &axum::Router, owner: &RequestAuth) {
    for path in [
        "/private-sheets/1",
        "/private-sheets/1/history",
        "/private-sheets/1/history/1",
        "/characters/1",
        "/characters/1/mork-borg",
        "/sources",
        "/sources/search",
    ] {
        assert_eq!(
            router_request(app, owner, path, None).await.status(),
            axum::http::StatusCode::FORBIDDEN,
            "{path}"
        );
    }
}

async fn maximum_definition(store: &Store) -> i64 {
    published_fixture(store).await;
    let draft = store.definition_draft(1).await.unwrap();
    let mut definition = serde_json::to_value(draft.definition).unwrap();
    definition["fields"]=serde_json::Value::Array((0..64).map(|n|serde_json::json!({"id":format!("field-{n}"),"label":format!("Field {n}"),"control":"multiline","nullable":false,"mapping":{"state":"mapped","regions":[{"source_id":1,"page_id":1,"unit_id":1}]}})).collect());
    definition["groups"][0]["fields"] =
        serde_json::json!((0..64).map(|n| format!("field-{n}")).collect::<Vec<_>>());
    let draft = store
        .save_definition_draft(serde_json::to_vec(&definition).unwrap(), 1)
        .await
        .unwrap();
    let comparison = store.compare_definition(draft.id, false).await.unwrap();
    store
        .approve_definition(
            draft.id,
            false,
            crate::definitions::ApprovalRequest {
                comparison_hash: comparison.comparison_hash,
                actor: "Synthetic capacity test".into(),
                rationale: "Synthetic image comparison".into(),
                originals_compared: true,
            },
        )
        .await
        .unwrap()
        .id
}

fn maximum_values(snapshot: &PrivateSnapshot) -> serde_json::Value {
    let mut reserve = snapshot.clone();
    reserve.revision = i64::MAX;
    reserve.state = "frozen".into();
    reserve.data = serde_json::json!(
        (0..64)
            .map(|n| (format!("field-{n}"), String::new()))
            .collect::<std::collections::BTreeMap<_, _>>()
    );
    let overhead = serde_json::to_vec(&reserve).unwrap().len();
    let mut remaining = snapshots::MAX_BYTES - overhead;
    for value in reserve.data.as_object_mut().unwrap().values_mut() {
        let length = remaining.min(20_000);
        *value = serde_json::json!("x".repeat(length));
        remaining -= length;
    }
    assert_eq!(remaining, 0);
    assert_eq!(
        serde_json::to_vec(&reserve).unwrap().len(),
        snapshots::MAX_BYTES
    );
    reserve.data
}

async fn prepare_maximum_sheet(
    store: &Store,
    auth: &RequestAuth,
    campaign: i64,
    version: i64,
) -> (i64, String) {
    let (mut view, permit) = store
        .private_create_context(
            auth.clone(),
            campaign,
            version,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    view.snapshot.name = "Capacity sheet".into();
    view.snapshot.data = serde_json::json!(
        (0..64)
            .map(|n| (format!("field-{n}"), String::new()))
            .collect::<std::collections::BTreeMap<_, _>>()
    );
    let id = store
        .private_save(auth.clone(), view, permit)
        .await
        .unwrap();
    for _ in 1..8 {
        let (view, permit) = store
            .private_read(auth.clone(), id, None, crate::definitions::admit().unwrap())
            .await
            .unwrap();
        store
            .private_save(auth.clone(), view, permit)
            .await
            .unwrap();
    }
    let (mut view, permit) = store
        .private_read(auth.clone(), id, None, crate::definitions::admit().unwrap())
        .await
        .unwrap();
    view.snapshot.data = maximum_values(&view.snapshot);
    let values = serde_json::to_string(&view.snapshot.data).unwrap();
    store
        .private_save(auth.clone(), view, permit)
        .await
        .unwrap();
    (id, values)
}

#[tokio::test]
async fn maximum_32_retained_sheets_digit_growth_headroom_and_atomic_freeze() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let version = maximum_definition(&store).await;
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Capacity".into(),
            "synthetic-game".into(),
            "first".into(),
        )
        .await
        .unwrap();
    store.request_join(auth[2].clone(), campaign).await.unwrap();
    store
        .decide_join(auth[1].clone(), campaign, 3, 1, true)
        .await
        .unwrap();
    let mut expected = Vec::new();
    for _ in 0..32 {
        expected.push(prepare_maximum_sheet(&store, &auth[2], campaign, version).await);
    }
    let (mut over_budget, permit) = store
        .private_read(
            auth[2].clone(),
            1,
            None,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    over_budget.snapshot.notes = "Consumes reserved canonical headroom".into();
    assert!(
        store
            .private_save(auth[2].clone(), over_budget, permit)
            .await
            .is_err()
    );
    store.run(|db|{db.execute_batch("CREATE TRIGGER fail_maximum_last BEFORE INSERT ON private_sheet_history WHEN NEW.sheet_id=32 AND NEW.operation='freeze' BEGIN SELECT RAISE(ABORT,'synthetic last-row disk failure'); END;")?;Ok(())}).await.unwrap();
    let rollback = std::time::Instant::now();
    assert!(store.depart(auth[2].clone(), campaign, 3, 1).await.is_err());
    println!(
        "StageC actual Rust maximum last-row failure/rollback: {:?}",
        rollback.elapsed()
    );
    store
        .run(|db| {
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM private_sheets WHERE revision=9 AND state='active'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                32
            );
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM private_sheet_history WHERE revision=10",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                0
            );
            db.execute_batch("DROP TRIGGER fail_maximum_last;")?;
            Ok(())
        })
        .await
        .unwrap();
    let (mut view, permit) = store
        .private_create_context(
            auth[2].clone(),
            campaign,
            version,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    view.snapshot.name = "Over capacity".into();
    view.snapshot.data = serde_json::json!(
        (0..64)
            .map(|n| (format!("field-{n}"), String::new()))
            .collect::<std::collections::BTreeMap<_, _>>()
    );
    assert!(
        store
            .private_save(auth[2].clone(), view, permit)
            .await
            .is_err()
    );
    let before = std::time::Instant::now();
    store.depart(auth[2].clone(), campaign, 3, 1).await.unwrap();
    println!(
        "StageC actual Rust 32 × nearly 1 MiB freeze, revision9→10: {:?}",
        before.elapsed()
    );
    assert_maximum_frozen_values(&store, &auth[2], expected).await;
}

async fn assert_maximum_frozen_values(
    store: &Store,
    auth: &RequestAuth,
    expected: Vec<(i64, String)>,
) {
    for (id, data) in expected {
        let (view, permit) = store
            .private_read(auth.clone(), id, None, crate::definitions::admit().unwrap())
            .await
            .unwrap();
        drop(permit);
        assert_eq!(view.snapshot.revision, 10);
        assert_eq!(view.snapshot.state, "frozen");
        assert_eq!(serde_json::to_string(&view.snapshot.data).unwrap(), data);
        assert_eq!(
            store
                .private_history(auth.clone(), id, 0)
                .await
                .unwrap()
                .len(),
            10
        );
    }
}

#[test]
fn private_encoding_one_has_an_independent_sha256_vector() {
    let snapshot = PrivateSnapshot {
        encoding: 1,
        sheet_id: 1,
        campaign_id: 2,
        game_id: "synthetic-game".into(),
        edition_id: "first".into(),
        custodian_id: 3,
        kind: "runtime".into(),
        pin: Pin::Runtime {
            version_id: 4,
            version: 1,
            content_hash: "0".repeat(64),
            schema: 1,
        },
        revision: 1,
        state: "active".into(),
        name: "N".into(),
        notes: String::new(),
        data: serde_json::json!({"flag":false,"large":9_007_199_254_740_993_i64,"unknown":null}),
    };
    let bytes = serde_json::to_string(&snapshot).unwrap();
    assert_eq!(
        bytes,
        r#"{"encoding":1,"sheet_id":1,"campaign_id":2,"game_id":"synthetic-game","edition_id":"first","custodian_id":3,"kind":"runtime","pin":{"type":"runtime","version_id":4,"version":1,"content_hash":"0000000000000000000000000000000000000000000000000000000000000000","schema":1},"revision":1,"state":"active","name":"N","notes":"","data":{"flag":false,"large":9007199254740993,"unknown":null}}"#
    );
    assert_eq!(
        snapshots::hash(bytes.as_bytes()),
        "96fb06a26f43b51c2efce54e9d22f6d9be194f2b14382fffabddd492ebfa6314"
    );
}

#[test]
fn url_transport_rejects_duplicate_unknown_missing_and_excess_pairs_before_map() {
    use super::transport::Pairs;
    assert!(Pairs::parse(b"csrf=a&%63srf=b").is_err());
    assert!(
        Pairs::parse(b"csrf=a&unknown=x")
            .unwrap()
            .admit(&["csrf", "name"].into_iter().collect())
            .is_err()
    );
    assert!(
        Pairs::parse(b"csrf=a")
            .unwrap()
            .admit(&["csrf", "name"].into_iter().collect())
            .is_err()
    );
    let keys = (0..516).map(|n| format!("field-{n}")).collect::<Vec<_>>();
    let encoded =
        serde_urlencoded::to_string(keys.iter().map(|key| (key, "0")).collect::<Vec<_>>()).unwrap();
    assert_eq!(
        Pairs::parse(encoded.as_bytes())
            .unwrap()
            .admit(&keys.iter().map(String::as_str).collect())
            .unwrap()
            .len(),
        516
    );
    assert!(Pairs::parse(format!("{encoded}&extra=0").as_bytes()).is_err());
}

#[tokio::test]
async fn captured_editor_loses_demoted_master_and_reset_account_authority() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Race".into(),
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
    insert_native(&store, auth[1].clone(), native(campaign, 2)).await;
    store
        .change_role(auth[1].clone(), campaign, 3, 1, true)
        .await
        .unwrap();
    let (view, permit) = store
        .private_read(
            auth[2].clone(),
            1,
            None,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    assert!(view.editable);
    store
        .change_role(auth[1].clone(), campaign, 3, 2, false)
        .await
        .unwrap();
    assert!(
        store
            .private_save(auth[2].clone(), view, permit)
            .await
            .is_err()
    );
    let (view, permit) = store
        .private_read(
            auth[1].clone(),
            1,
            None,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    store
        .run(|db| {
            db.execute("UPDATE accounts SET epoch=2 WHERE id=2", [])?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(
        store
            .private_save(auth[1].clone(), view, permit)
            .await
            .is_err()
    );
    assert_eq!(
        store
            .run(|db| Ok(db
                .query_row("SELECT count(*) FROM private_sheet_history", [], |r| r
                    .get::<_, i64>(0))?))
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn cancellation_does_not_release_private_read_runtime_before_sql_worker_finishes() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Cancel".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    insert_native(&store, auth[1].clone(), native(campaign, 2)).await;
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let holder = store.clone();
    let held = tokio::spawn(async move {
        holder
            .run(move |_| {
                entered_tx.send(()).unwrap();
                release_rx
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap();
                Ok(())
            })
            .await
    });
    tokio::task::spawn_blocking(move || {
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
    })
    .await
    .unwrap();
    let reader = store.clone();
    let actor = auth[1].clone();
    let permit = crate::definitions::admit().unwrap();
    let task = tokio::spawn(async move { reader.private_read(actor, 1, None, permit).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while crate::store::AUTH_WORKERS.available_permits() == 16 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(matches!(task.await,Err(error) if error.is_cancelled()));
    assert!(crate::definitions::admit().is_err());
    release_tx.send(()).unwrap();
    held.await.unwrap().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while crate::store::AUTH_WORKERS.available_permits() != 16 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // The SQL slot is released before the abandoned blocking-task result is
    // dropped. Wait for the runtime permit's own completion boundary.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Ok(permit) = crate::definitions::admit() {
                drop(permit);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[path = "../../tests/support/schema_fixture.rs"]
mod schema_fixture;

#[tokio::test]
async fn populated_campaign_fixture_rejects_before_any_drop() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    store
        .create_campaign(
            auth[1].clone(),
            "Retained".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    store
        .run(|db| {
            let before: String = db.query_row(
                "SELECT group_concat(name,'|') FROM (SELECT name FROM sqlite_schema ORDER BY name)",
                [],
                |r| r.get(0),
            )?;
            for mode in 0..3 {
                let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if mode == 2 {
                        schema_fixture::remove_empty_account_tables(db);
                    } else {
                        schema_fixture::pre_finalization(db, mode == 1);
                    }
                }));
                assert!(rejected.is_err());
            }
            let after: String = db.query_row(
                "SELECT group_concat(name,'|') FROM (SELECT name FROM sqlite_schema ORDER BY name)",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(before, after);
            assert_eq!(
                db.query_row("SELECT title FROM campaigns", [], |r| r.get::<_, String>(0))?,
                "Retained"
            );
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn regression_disabled_additional_master_can_be_demoted_then_removed_and_frozen() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Departure".into(),
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
    insert_native(&store, auth[2].clone(), native(campaign, 3)).await;
    store
        .change_role(auth[1].clone(), campaign, 3, 1, true)
        .await
        .unwrap();
    store
        .run(|db| {
            db.execute(
                "UPDATE accounts SET epoch=2,state='disabled' WHERE id=3",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(
        store
            .change_role(auth[1].clone(), campaign, 3, 2, false)
            .await
            .is_ok(),
        "Revoking a disabled target's historical grant must remain possible"
    );
    store.depart(auth[1].clone(), campaign, 3, 3).await.unwrap();
    let (view, permit) = store
        .private_read(
            auth[1].clone(),
            1,
            None,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    drop(permit);
    assert_eq!(view.snapshot.state, "frozen");
    assert_eq!(view.snapshot.revision, 2);
}

#[tokio::test]
async fn regression_missing_finalization_does_not_hide_present_corrupt_source_metadata() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let version = published_fixture(&store).await;
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Metadata".into(),
            "synthetic-game".into(),
            "first".into(),
        )
        .await
        .unwrap();
    let (mut view, permit) = store
        .private_create_context(
            auth[1].clone(),
            campaign,
            version,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    view.snapshot.name = "Sheet".into();
    view.snapshot.data = serde_json::json!({"label":"Text","large":1,"flag":null,"story":null});
    store
        .private_save(auth[1].clone(), view, permit)
        .await
        .unwrap();
    store
        .run(|db| {
            let sql: String = db.query_row(
                "SELECT sql FROM sqlite_schema WHERE name='source_finalize_delete'",
                [],
                |r| r.get(0),
            )?;
            db.execute_batch("DROP TRIGGER source_finalize_delete;")?;
            db.execute("DELETE FROM source_finalizations", [])?;
            db.execute_batch(&sql)?;
            db.execute("UPDATE sources SET title=?1", ["x".repeat(300)])?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(
        store
            .private_read(
                auth[1].clone(),
                1,
                None,
                crate::definitions::admit().unwrap()
            )
            .await
            .is_err(),
        "Missing evidence cannot turn present metadata corruption into a warning"
    );
}

#[tokio::test]
async fn regression_invalid_boolean_and_nullable_state_survive_the_complete_private_draft() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let version = published_fixture(&store).await;
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Draft".into(),
            "synthetic-game".into(),
            "first".into(),
        )
        .await
        .unwrap();
    let app = crate::auth::app(store).await.unwrap();
    for (raw_value, raw_state) in [("invalid-bool", "known"), ("false", "invalid-state")] {
        let form = vec![
            ("csrf".into(), "a".repeat(64)),
            ("revision".into(), "0".into()),
            ("name".into(), "Draft".into()),
            ("notes".into(), String::new()),
            ("value-label".into(), "Text".into()),
            ("value-large".into(), "1".into()),
            ("value-flag".into(), raw_value.into()),
            ("null-flag".into(), raw_state.into()),
            ("value-story".into(), String::new()),
            ("null-story".into(), "unknown".into()),
        ];
        let response = router_request(
            &app,
            &auth[1],
            &format!("/campaigns/{campaign}/sheets/new?version={version}"),
            Some(form),
        )
        .await;
        assert_eq!(
            response.status(),
            axum::http::StatusCode::UNPROCESSABLE_ENTITY
        );
        let html = String::from_utf8(
            axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(
            html.contains(&format!("value=\"{raw_value}\"")),
            "raw boolean value lost"
        );
        assert!(
            html.contains(&format!("value=\"{raw_state}\"")),
            "raw nullable state lost"
        );
        assert!(html.contains("name=\"revision\" value=\"0\""));
    }
}

#[tokio::test]
async fn regression_future_calculation_pin_is_readonly_and_freeze_mirrors_known_native_schema_revision()
 {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Future".into(),
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
    insert_native(&store, auth[2].clone(), native(campaign, 3)).await;
    store
        .run(|db| {
            let tx = db.transaction()?;
            let mut future = snapshots::load(&tx, 1)?;
            future.pin = Pin::Native {
                adapter: "mork-borg-it-2025".into(),
                schema: 1,
                calculation_version: 2,
            };
            let snapshot = serde_json::to_string(&future)?;
            let hash = snapshots::hash(snapshot.as_bytes());
            let pin = serde_json::to_string(&future.pin)?;
            let triggers = ["private_sheets_update", "private_sheet_history_update"].map(|name| {
                tx.query_row("SELECT sql FROM sqlite_schema WHERE name=?1", [name], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap()
            });
            tx.execute_batch(
                "DROP TRIGGER private_sheets_update; DROP TRIGGER private_sheet_history_update;",
            )?;
            tx.execute(
                "UPDATE private_sheets SET pin=?1,snapshot=?2,hash=?3",
                params![pin, snapshot, hash],
            )?;
            tx.execute(
                "UPDATE private_sheet_history SET snapshot=?1,hash=?2",
                params![snapshot, hash],
            )?;
            for sql in triggers {
                tx.execute_batch(&sql)?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap();
    let (view, permit) = store
        .private_read(
            auth[2].clone(),
            1,
            None,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    drop(permit);
    assert!(!view.editable);
    assert!(view.warning.contains("Unsupported"));
    store.depart(auth[2].clone(), campaign, 3, 1).await.unwrap();
    let (view, permit) = store
        .private_read(
            auth[2].clone(),
            1,
            None,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    drop(permit);
    assert_eq!(view.snapshot.revision, 2);
    assert_eq!(view.snapshot.data["revision"], 2);
    assert_eq!(view.snapshot.state, "frozen");
}

#[tokio::test]
async fn actual_router_runtime_i64_nullable_stale_pin_edit_and_missing_evidence_history() {
    use axum::http::StatusCode;
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let version = published_fixture(&store).await;
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Generic".into(),
            "synthetic-game".into(),
            "first".into(),
        )
        .await
        .unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    let path = format!("/campaigns/{campaign}/sheets/new?version={version}");
    assert_eq!(
        router_request(&app, &auth[1], &path, None).await.status(),
        StatusCode::OK
    );
    let form = vec![
        ("csrf".into(), "a".repeat(64)),
        ("revision".into(), "0".into()),
        ("name".into(), "Runtime sheet".into()),
        ("notes".into(), "Exact".into()),
        ("value-label".into(), "Text".into()),
        ("value-large".into(), "9007199254740993".into()),
        ("value-flag".into(), "false".into()),
        ("null-flag".into(), "known".into()),
        ("value-story".into(), String::new()),
        ("null-story".into(), "unknown".into()),
    ];
    assert_eq!(
        router_request(&app, &auth[1], &path, Some(form.clone()))
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    store
        .run(|db| {
            let sql: String = db.query_row(
                "SELECT sql FROM sqlite_schema WHERE name='source_finalize_delete'",
                [],
                |r| r.get(0),
            )?;
            db.execute_batch("DROP TRIGGER source_finalize_delete;")?;
            db.execute("DELETE FROM source_finalizations WHERE source_id=1", [])?;
            db.execute_batch(&sql)?;
            Ok(())
        })
        .await
        .unwrap();
    let response = router_request(&app, &auth[1], "/private-sheets/1", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = String::from_utf8(
        axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("9007199254740993"));
    assert!(html.contains("evidence is unavailable"));
    // The edit changes the notes: an unchanged save would write nothing (D-U).
    let mut edited = form;
    for pair in &mut edited {
        match pair.0.as_str() {
            "revision" => pair.1 = "1".into(),
            "notes" => pair.1 = "Edited".into(),
            _ => {}
        }
    }
    assert_eq!(
        router_request(&app, &auth[1], "/private-sheets/1", Some(edited))
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        router_request(&app, &auth[1], "/private-sheets/1/history/1", None)
            .await
            .status(),
        StatusCode::OK
    );
    let (view, permit) = store
        .private_read(
            auth[1].clone(),
            1,
            None,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    drop(permit);
    assert_eq!(
        view.snapshot.data["large"].as_i64(),
        Some(9_007_199_254_740_993)
    );
    assert_eq!(view.snapshot.data["flag"], false);
    assert!(view.snapshot.data["story"].is_null());
}

async fn published_256_fields(store: &Store) -> (i64, Vec<String>) {
    published_fixture(store).await;
    let draft = store.definition_draft(1).await.unwrap();
    let mut definition = serde_json::to_value(draft.definition).unwrap();
    let ids = (0..253)
        .map(|n| format!("field-{n}"))
        .chain(["title".into(), "label".into(), "help".into()])
        .collect::<Vec<_>>();
    definition["fields"] = serde_json::json!(ids.iter().map(|id|serde_json::json!({"id":id,"label":id,"control":"text","nullable":true,"mapping":{"state":"mapped","regions":[{"source_id":1,"page_id":1,"unit_id":1}]}})).collect::<Vec<_>>());
    definition["groups"][0]["fields"] = serde_json::json!(ids);
    let draft = store
        .save_definition_draft(serde_json::to_vec(&definition).unwrap(), 1)
        .await
        .unwrap();
    let comparison = store.compare_definition(draft.id, false).await.unwrap();
    let version = store
        .approve_definition(
            draft.id,
            false,
            crate::definitions::ApprovalRequest {
                comparison_hash: comparison.comparison_hash,
                actor: "Synthetic".into(),
                rationale: "Synthetic capacity schema".into(),
                originals_compared: true,
            },
        )
        .await
        .unwrap()
        .id;
    (version, ids)
}

fn complete_nullable_form(ids: &[String]) -> Vec<(String, String)> {
    let mut form = vec![
        ("csrf".into(), "a".repeat(64)),
        ("revision".into(), "0".into()),
        ("name".into(), "Complete values".into()),
        ("notes".into(), String::new()),
    ];
    for id in ids {
        form.push((
            format!("value-{id}"),
            if id == "field-0" {
                String::new()
            } else {
                "v".repeat(1000)
            },
        ));
        form.push((
            format!("null-{id}"),
            if id == "title" { "unknown" } else { "known" }.into(),
        ));
    }
    form
}

#[tokio::test]
async fn complete_256_field_editor_and_transactional_creation_readiness() {
    use axum::http::StatusCode;
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let (version, ids) = published_256_fields(&store).await;
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "256 fields".into(),
            "synthetic-game".into(),
            "first".into(),
        )
        .await
        .unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    let path = format!("/campaigns/{campaign}/sheets/new?version={version}");
    assert_eq!(
        router_request(&app, &auth[1], &path, None).await.status(),
        StatusCode::OK
    );
    let form = complete_nullable_form(&ids);
    assert_eq!(form.len(), 516);
    assert_eq!(
        router_request(&app, &auth[1], &path, Some(form))
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let (view, permit) = store
        .private_read(
            auth[1].clone(),
            1,
            Some(1),
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(view.snapshot.data.as_object().unwrap().len(), 256);
    assert_eq!(view.snapshot.data["field-0"], "");
    assert!(view.snapshot.data["title"].is_null());
    assert_eq!(view.snapshot.data["label"].as_str().unwrap().len(), 1000);
    assert_eq!(view.snapshot.data["help"].as_str().unwrap().len(), 1000);
    drop(permit);
    let (mut pending, permit) = store
        .private_create_context(
            auth[1].clone(),
            campaign,
            version,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    pending.snapshot.name = "Readiness race".into();
    pending.snapshot.data = serde_json::json!(
        ids.iter()
            .map(|id| (id.clone(), serde_json::Value::Null))
            .collect::<std::collections::BTreeMap<_, _>>()
    );
    store.run(move|db|{
        db.execute("INSERT INTO definition_withdrawals(approval_id,version_id,actor,rationale) SELECT id,version_id,'Synthetic','Readiness changed after admission' FROM definition_approvals WHERE version_id=?1 ORDER BY id DESC LIMIT 1",[version])?;
        Ok(())
    }).await.unwrap();
    assert!(
        store
            .private_save(auth[1].clone(), pending, permit)
            .await
            .is_err()
    );
    assert_eq!(
        store
            .private_list(auth[1].clone(), Some(campaign), 0)
            .await
            .unwrap()
            .len(),
        1
    );
    let (mut existing, permit) = store
        .private_read(
            auth[1].clone(),
            1,
            None,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    existing.snapshot.notes = "Still structurally editable after withdrawal".into();
    store
        .private_save(auth[1].clone(), existing, permit)
        .await
        .unwrap();
    assert_eq!(
        store
            .private_history(auth[1].clone(), 1, 0)
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn captured_campaign_authority_rejects_logout_rotation_and_expiry() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    for revocation in ["logout", "rotation", "expiry"] {
        let (_dir, store, auth, clock) = fixture().await;
        let campaign = store
            .create_campaign(
                auth[1].clone(),
                "Boundary".into(),
                "mork-borg".into(),
                "it-2025".into(),
            )
            .await
            .unwrap();
        insert_native(&store, auth[1].clone(), native(campaign, 2)).await;
        let (mut view, permit) = store
            .private_read(
                auth[1].clone(),
                1,
                None,
                crate::definitions::admit().unwrap(),
            )
            .await
            .unwrap();
        view.snapshot.notes = "Must not commit".into();
        if revocation == "expiry" {
            clock.store(1000, Ordering::SeqCst);
        } else {
            let id = store
                .run(|db| {
                    Ok(db.query_row(
                        "SELECT id FROM auth_sessions WHERE account_id=2 AND state='active'",
                        [],
                        |row| row.get::<_, String>(0),
                    )?)
                })
                .await
                .unwrap();
            SqliteSessions::new(store.clone())
                .delete(&id.parse().unwrap())
                .await
                .unwrap();
            if revocation == "rotation" {
                let mut data = HashMap::new();
                data.insert("csrf".into(), serde_json::json!("b".repeat(64)));
                data.insert("principal".into(), serde_json::json!({"id":2,"epoch":1}));
                let mut fresh = Record {
                    id: Id::default(),
                    data,
                    expiry_date: time::OffsetDateTime::from_unix_timestamp(1000).unwrap(),
                };
                SqliteSessions::new(store.clone())
                    .create(&mut fresh)
                    .await
                    .unwrap();
            }
        }
        assert!(
            store
                .private_save(auth[1].clone(), view, permit)
                .await
                .is_err(),
            "Captured actor survived {revocation}"
        );
        assert!(store.request_join(auth[1].clone(), campaign).await.is_err());
        store
            .run(|db| {
                assert_eq!(
                    db.query_row(
                        "SELECT revision FROM private_sheets WHERE id=1",
                        [],
                        |row| row.get::<_, i64>(0)
                    )?,
                    1
                );
                assert_eq!(
                    db.query_row(
                        "SELECT count(*) FROM private_sheet_history WHERE sheet_id=1",
                        [],
                        |row| row.get::<_, i64>(0)
                    )?,
                    1
                );
                Ok(())
            })
            .await
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn private_mutation_rechecks_expiry_after_waiting_for_database_worker() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, clock) = fixture().await;
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Queued expiry".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    insert_native(&store, auth[1].clone(), native(campaign, 2)).await;
    let (view, permit) = store
        .private_read(
            auth[1].clone(),
            1,
            None,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let locking = store.clone();
    let lock = tokio::spawn(async move {
        locking
            .run(move |_| {
                entered.send(()).unwrap();
                released
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                Ok(())
            })
            .await
    });
    waiting.await.unwrap();
    let saving = store.clone();
    let captured = auth[1].clone();
    let write = tokio::spawn(async move { saving.private_save(captured, view, permit).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while crate::store::AUTH_WORKERS.available_permits() == 16 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    clock.store(1000, Ordering::SeqCst);
    release.send(()).unwrap();
    lock.await.unwrap().unwrap();
    assert!(write.await.unwrap().is_err());
    assert_private_counts(&store, 1).await;
}

#[tokio::test]
async fn bounded_private_reads_reject_corrupt_snapshot_pin_and_definition() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    for damaged in ["snapshot", "pin", "definition"] {
        let (_dir, store, auth, _clock) = fixture().await;
        let version = published_fixture(&store).await;
        let campaign = store
            .create_campaign(
                auth[1].clone(),
                "Corruption".into(),
                "synthetic-game".into(),
                "first".into(),
            )
            .await
            .unwrap();
        let (mut view, permit) = store
            .private_create_context(
                auth[1].clone(),
                campaign,
                version,
                crate::definitions::admit().unwrap(),
            )
            .await
            .unwrap();
        view.snapshot.name = "Retained".into();
        view.snapshot.data =
            serde_json::json!({"label":"value","large":0,"flag":null,"story":null});
        store
            .private_save(auth[1].clone(), view, permit)
            .await
            .unwrap();
        store
            .run(move |db| {
                let (trigger, statement, bytes) = match damaged {
                    "snapshot" => (
                        "private_sheet_history_update",
                        "UPDATE private_sheet_history SET snapshot=?1 WHERE sheet_id=1",
                        snapshots::MAX_BYTES + 1,
                    ),
                    "pin" => (
                        "private_sheets_update",
                        "UPDATE private_sheets SET pin=?1 WHERE id=1",
                        4097,
                    ),
                    _ => (
                        "definition_versions_update",
                        "UPDATE definition_versions SET content=?1 WHERE id=?2",
                        crate::definitions::MAX_BYTES + 1,
                    ),
                };
                let original: String = db.query_row(
                    "SELECT sql FROM sqlite_schema WHERE name=?1",
                    [trigger],
                    |row| row.get(0),
                )?;
                db.execute_batch(&format!(
                    "DROP TRIGGER {trigger}; PRAGMA ignore_check_constraints=ON;"
                ))?;
                let content = serde_json::to_string(&"x".repeat(bytes))?;
                if damaged == "definition" {
                    db.execute(statement, params![content, version])?;
                } else {
                    db.execute(statement, [content])?;
                }
                db.execute_batch("PRAGMA ignore_check_constraints=OFF;")?;
                db.execute_batch(&original)?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(
            store
                .private_read(
                    auth[1].clone(),
                    1,
                    None,
                    crate::definitions::admit().unwrap()
                )
                .await
                .is_err(),
            "Stored {damaged} escaped bounded admission"
        );
    }
}

#[tokio::test]
async fn final_representable_revision_remains_available_for_mandatory_freeze() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Revision boundary".into(),
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
    insert_native(&store, auth[2].clone(), native(campaign, 3)).await;
    let actor = auth[2].clone();
    store.run_campaign(actor.clone(),move|db|{
        // Seed an unreachable counter boundary in a synthetic fixture; the
        // normal transition guards are restored before testing real operations.
        let tx=db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut snapshot=snapshots::load(&tx,1)?;
        let triggers=["private_sheets_update","private_sheet_history_insert"].map(|name|tx.query_row("SELECT sql FROM sqlite_schema WHERE name=?1",[name],|row|row.get::<_,String>(0)).unwrap());
        tx.execute_batch("DROP TRIGGER private_sheets_update; DROP TRIGGER private_sheet_history_insert;")?;
        for revision in [i64::MAX-2,i64::MAX-1] {
            snapshot.revision=revision;
            snapshot.data["revision"]=revision.into();
            let content=serde_json::to_string(&snapshot)?;
            let hash=snapshots::hash(content.as_bytes());
            tx.execute("INSERT INTO private_sheet_history(sheet_id,revision,hash,snapshot,operation,previous_revision,actor_id,actor_epoch,session_digest,occurred_at) VALUES(1,?1,?2,?3,'edit',?4,3,1,?5,100)",params![revision,hash,content,revision-1,actor.digest()])?;
            if revision==i64::MAX-1 { tx.execute("UPDATE private_sheets SET revision=?1,snapshot=?2,hash=?3 WHERE id=1",params![revision,content,hash])?; }
        }
        for trigger in triggers {tx.execute_batch(&trigger)?;}
        tx.commit()?;
        Ok(())
    }).await.unwrap();
    let (view, permit) = store
        .private_read(
            auth[2].clone(),
            1,
            None,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    assert!(
        store
            .private_save(auth[2].clone(), view, permit)
            .await
            .is_err()
    );
    store.depart(auth[2].clone(), campaign, 3, 1).await.unwrap();
    let (view, permit) = store
        .private_read(
            auth[2].clone(),
            1,
            None,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(view.snapshot.revision, i64::MAX);
    assert_eq!(view.snapshot.data["revision"], i64::MAX);
    assert_eq!(view.snapshot.state, "frozen");
    drop(permit);
}

async fn capacity_accounts(store: &Store) -> Vec<RequestAuth> {
    store.run(|db|{
        for account in 10..=73 {
            db.execute("INSERT INTO accounts(id,login,display_name,state,epoch,verifier) VALUES(?1,?2,?2,'enabled',1,?3)",params![account,format!("capacity{account}"),"$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"])?;
        }
        Ok(())
    }).await.unwrap();
    let sessions = SqliteSessions::new(store.clone());
    let mut accounts = Vec::new();
    for account in 10..=73 {
        let mut data = HashMap::new();
        data.insert("csrf".into(), serde_json::json!("a".repeat(64)));
        data.insert(
            "principal".into(),
            serde_json::json!({"id":account,"epoch":1}),
        );
        let mut record = Record {
            id: Id::default(),
            data,
            expiry_date: time::OffsetDateTime::from_unix_timestamp(1000).unwrap(),
        };
        sessions.create(&mut record).await.unwrap();
        accounts.push(RequestAuth::fixture(account, record.id, 1000));
    }
    accounts
}

#[tokio::test]
async fn distinct_64_reservations_and_global_private_sheet_cap_are_enforced() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let extra = capacity_accounts(&store).await;
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Capacity".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    for account in extra.iter().take(63) {
        store.request_join(account.clone(), campaign).await.unwrap();
    }
    assert!(
        store
            .request_join(extra[63].clone(), campaign)
            .await
            .is_err()
    );
    for (offset, account) in extra.iter().take(63).enumerate() {
        store
            .decide_join(
                auth[1].clone(),
                campaign,
                i64::try_from(offset).unwrap() + 10,
                1,
                true,
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .campaign_detail(account.clone(), campaign, 0)
                .await
                .unwrap()
                .role,
            "player"
        );
    }
    for (offset, account) in std::iter::once(&auth[1])
        .chain(extra.iter().take(63))
        .enumerate()
    {
        let account = account.clone();
        store
            .run_campaign(account.clone(), move |db| {
                let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                for local in 0..32 {
                    let mut snapshot = native(campaign, account.account_id());
                    snapshot.sheet_id = i64::try_from(offset * 32 + local + 1).unwrap();
                    snapshots::write(&tx, &account, snapshot, "create", None)?;
                }
                tx.commit()?;
                Ok(())
            })
            .await
            .unwrap();
    }
    let second = store
        .create_campaign(
            auth[1].clone(),
            "Global limit".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    let account = auth[1].clone();
    assert!(
        store
            .run_campaign(account.clone(), move |db| {
                let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                let mut snapshot = native(second, account.account_id());
                snapshot.sheet_id = 2049;
                snapshots::write(&tx, &account, snapshot, "create", None)?;
                tx.commit()?;
                Ok(())
            })
            .await
            .is_err()
    );
    assert_private_counts(&store, 2048).await;
}

#[tokio::test]
async fn regression_roster_next_link_requires_more_rows_and_preserves_sheet_section() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    for count in [0, 48, 49, 50] {
        let (_dir, store, auth, _clock) = fixture().await;
        let accounts = capacity_accounts(&store).await;
        let campaign = store
            .create_campaign(
                auth[1].clone(),
                "Pagination".into(),
                "mork-borg".into(),
                "it-2025".into(),
            )
            .await
            .unwrap();
        for account in accounts.iter().take(count) {
            store.request_join(account.clone(), campaign).await.unwrap();
            store
                .decide_join(auth[1].clone(), campaign, account.account_id(), 1, true)
                .await
                .unwrap();
        }
        let mut sheet = native(campaign, 2);
        sheet.name = "Preserved private section".into();
        insert_native(&store, auth[1].clone(), sheet).await;
        let app = crate::auth::app(store).await.unwrap();
        let response = router_request(
            &app,
            &auth[1],
            &format!("/campaigns/{campaign}?requests_after=123&versions_after=456"),
            None,
        )
        .await;
        let html = String::from_utf8(
            axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert_eq!(
            html.contains("Next roster page"),
            count == 50,
            "False next-page link for {} members",
            count + 1
        );
        assert!(html.contains("Preserved private section"));
        if count == 50 {
            assert!(html.contains("requests_after=123&amp;versions_after=456&amp;sheets_after=0"));
            let response = router_request(
                &app,
                &auth[1],
                &format!("/campaigns/{campaign}?members_after=58"),
                None,
            )
            .await;
            assert_eq!(response.status(), axum::http::StatusCode::OK);
            let html = String::from_utf8(
                axum::body::to_bytes(response.into_body(), 1024 * 1024)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap();
            assert!(html.contains("capacity59"));
            assert!(html.contains("Preserved private section"));
            assert!(!html.contains("Next roster page"));
        }
    }
}

#[test]
fn shared_listing_next_cursor_uses_last_rendered_row_not_lookahead() {
    for count in [0, 49, 50, 51] {
        let page = super::Listing::new((1..=count).collect::<Vec<i64>>(), |id| *id);
        assert_eq!(page.rows.len(), usize::try_from(count.min(50)).unwrap());
        assert_eq!(page.next, if count == 51 { Some(50) } else { None });
    }
}

#[tokio::test]
async fn regression_production_owner_is_administration_only() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    assert!(
        store
            .create_campaign(
                auth[0].clone(),
                "Owner must not play".into(),
                "mork-borg".into(),
                "it-2025".into()
            )
            .await
            .is_err()
    );
    assert!(store.workspace(auth[0].clone(), 0).await.is_err());
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Player positive".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    assert!(
        store
            .campaign_detail(auth[1].clone(), campaign, 0)
            .await
            .is_ok()
    );
    assert!(store.request_join(auth[0].clone(), campaign).await.is_err());
}

#[tokio::test]
async fn owner_development_records_are_preserved_but_do_not_grant_production_authority() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let development = auth[0].clone().development_fixture();
    let campaign = store
        .create_campaign(
            development.clone(),
            "Retained development campaign".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    store.request_join(auth[2].clone(), campaign).await.unwrap();
    store
        .decide_join(development.clone(), campaign, 3, 1, true)
        .await
        .unwrap();
    insert_native(&store, development.clone(), native(campaign, 1)).await;
    assert!(
        store
            .private_read(
                auth[0].clone(),
                1,
                None,
                crate::definitions::admit().unwrap()
            )
            .await
            .is_err()
    );
    assert!(store.private_history(auth[0].clone(), 1, 0).await.is_err());
    let (view, permit) = store
        .private_read(
            development.clone(),
            1,
            None,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    assert!(view.editable);
    drop(permit);
    let player = auth[2].clone();
    store
        .run_campaign(player.clone(), move |db| {
            assert!(super::snapshots::authorize_edit(db, &player, &native(campaign, 3)).is_err());
            let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let mut sheet = native(campaign, 3);
            sheet.sheet_id = 2;
            assert!(snapshots::write(&tx, &player, sheet, "create", None).is_err());
            Ok(())
        })
        .await
        .unwrap();
    assert_private_counts(&store, 1).await;
    assert!(store.run(|db|{
        db.execute("INSERT INTO campaigns VALUES(99,1,'mork-borg','it-2025','No leaked development context',1)",[])?;
        Ok(())
    }).await.is_err());
}

#[tokio::test]
async fn owner_admin_landing_and_explicit_development_router_enforce_their_scopes() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let owner = auth[0].clone().development_fixture();
    let campaign = store
        .create_campaign(
            owner,
            "Development only".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    let normal = crate::auth::app(store.clone()).await.unwrap();
    let status = router_request(&normal, &auth[0], "/auth/session", None).await;
    let html = String::from_utf8(
        axum::body::to_bytes(status.into_body(), 8192)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("Open administration"));
    assert!(!html.contains("Open campaign workspace"));
    assert_eq!(
        router_request(&normal, &auth[0], "/admin", None)
            .await
            .status(),
        axum::http::StatusCode::OK
    );
    assert_eq!(
        router_request(&normal, &auth[1], "/admin", None)
            .await
            .status(),
        axum::http::StatusCode::FORBIDDEN
    );
    for path in [
        "/campaigns".into(),
        format!("/campaigns/{campaign}"),
        "/campaigns?development_owner_play=true".into(),
    ] {
        assert!(
            router_request(&normal, &auth[0], &path, None)
                .await
                .status()
                .is_client_error()
        );
    }
    let development = crate::auth::app_with_options(
        store,
        crate::auth::AuthOptions {
            development_owner_play: true,
            ..crate::auth::AuthOptions::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        router_request(
            &development,
            &auth[0],
            &format!("/campaigns/{campaign}"),
            None
        )
        .await
        .status(),
        axum::http::StatusCode::OK
    );
    assert_eq!(
        router_request(&normal, &auth[0], &format!("/campaigns/{campaign}"), None)
            .await
            .status(),
        axum::http::StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn populated_schema12_upgrade_preserves_owner_development_records_and_sessions() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (directory, store, auth, clock) = fixture().await;
    let development = auth[0].clone().development_fixture();
    let campaign = store
        .create_campaign(
            development.clone(),
            "Retained schema12".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    let original = insert_native(&store, development.clone(), native(campaign, 1)).await;
    store
        .run(|db| {
            assert_eq!(
                db.query_row("SELECT count(*) FROM account_web_events", [], |row| row
                    .get::<_, i64>(0))?,
                0
            );
            assert_eq!(db.query_row("SELECT count(*) FROM source_web_events",[],|r|r.get::<_,i64>(0))?,0);
            db.execute_batch("DROP TABLE registry_web_events; DROP TABLE registry_events; DROP TABLE source_web_events; DROP INDEX source_review_page_history; DROP TABLE account_web_events; DROP VIEW account_web_request;")?;
            for trigger in [
                "private_owner_supervision_insert",
                "private_owner_supervision_update",
                "campaign_owner_membership_insert",
                "campaign_owner_membership_update",
            ] {
                db.execute_batch(&format!("DROP TRIGGER {trigger};"))?;
            }
            db.execute_batch("DROP VIEW campaign_request_auth;")?;
            db.execute_batch(
                include_str!("../../migrations/012_campaigns.sql")
                    .lines()
                    .next()
                    .unwrap(),
            )?;
            crate::sources::search::drop_schema(db)?;
            db.execute_batch(crate::catalogues::DROP_SCHEMA)?;db.execute_batch(crate::campaigns::DROP_NOTES_SCHEMA)?;
            db.execute_batch("PRAGMA user_version=12;")?;
            Ok(())
        })
        .await
        .unwrap();
    let reopened = Store::open(&directory.path().join("private.sqlite3")).unwrap();
    reopened
        .run(move |db| {
            crate::store::register_auth_clock(
                db,
                Arc::new(move || Ok(clock.load(Ordering::SeqCst))),
            )
        })
        .await
        .unwrap();
    assert!(
        reopened
            .private_read(
                auth[0].clone(),
                1,
                None,
                crate::definitions::admit().unwrap()
            )
            .await
            .is_err()
    );
    let (view, permit) = reopened
        .private_read(development, 1, None, crate::definitions::admit().unwrap())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_string(&view.snapshot).unwrap(),
        serde_json::to_string(&original).unwrap()
    );
    drop(permit);
    assert_private_counts(&reopened, 1).await;
    reopened
        .run(|db| {
            assert_eq!(
                db.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?,
                crate::store::SCHEMA_VERSION
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM auth_sessions", [], |row| row
                    .get::<_, i64>(0))?,
                5
            );
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn schema13_guards_reject_owner_insert_regrant_and_owner_only_supervised_update() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let developer = auth[0].clone().development_fixture();
    let legacy = store
        .create_campaign(
            developer.clone(),
            "Owner only Master".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    store.request_join(auth[2].clone(), legacy).await.unwrap();
    store
        .decide_join(developer, legacy, 3, 1, true)
        .await
        .unwrap();
    insert_native(
        &store,
        auth[2].clone().development_fixture(),
        native(legacy, 3),
    )
    .await;
    let player = auth[2].clone();
    store.run_campaign(player,move|db|{
        let result=db.execute("UPDATE private_sheets SET revision=2 WHERE id=1",[]);
        assert!(matches!(result,Err(rusqlite::Error::SqliteFailure(_,Some(ref message))) if message=="enabled gameplay Master required"));
        Ok(())
    }).await.unwrap();
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Normal campaign".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    store
        .request_join(auth[0].clone().development_fixture(), campaign)
        .await
        .unwrap();
    assert!(
        store
            .campaign_detail(auth[1].clone(), campaign, 0)
            .await
            .unwrap()
            .requests
            .is_empty()
    );
    let development_requests = store
        .campaign_detail(auth[1].clone().development_fixture(), campaign, 0)
        .await
        .unwrap();
    assert_eq!(development_requests.requests.len(), 1);
    assert_eq!(development_requests.requests[0].account_id, 1);
    assert!(matches!(
        store
            .decide_join(auth[1].clone(), campaign, 1, 1, true)
            .await,
        Err(AppError::Invalid(_))
    ));
    store
        .decide_join(auth[1].clone().development_fixture(), campaign, 1, 1, true)
        .await
        .unwrap();
    assert!(
        store
            .change_role(auth[1].clone(), campaign, 1, 1, true)
            .await
            .is_err()
    );
    let owner = auth[0].clone();
    assert!(store.run_campaign(owner,|db|{
        db.execute("INSERT INTO campaigns VALUES(99,1,'mork-borg','it-2025','False flag denial',1)",[])?;
        Ok(())
    }).await.is_err());
    assert_private_counts(&store, 1).await;
}

#[tokio::test]
async fn directory_requests_sheets_and_history_paginate_authorized_rows_at_boundaries() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let accounts = capacity_accounts(&store).await;
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Pagination".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    for index in 0..51 {
        store
            .request_join(accounts[index].clone(), campaign)
            .await
            .unwrap();
        if index > 0 {
            store
                .create_campaign(
                    accounts[index].clone(),
                    format!("Directory {index}"),
                    "mork-borg".into(),
                    "it-2025".into(),
                )
                .await
                .unwrap();
        }
        let count = index + 1;
        if [1, 49, 50, 51].contains(&count) {
            let directory = store.workspace(auth[1].clone(), 0).await.unwrap();
            assert_eq!(directory.campaigns.len(), count.min(50));
            assert_eq!(directory.next, if count == 51 { Some(50) } else { None });
            let detail = store
                .campaign_detail_page(
                    auth[1].clone(),
                    campaign,
                    super::domain::DetailPage {
                        members: 1,
                        versions: 17,
                        sheets: 23,
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            assert_eq!(detail.requests.len(), count.min(50));
            assert_eq!(
                detail.requests_next,
                if count == 51 {
                    Some(accounts[49].account_id())
                } else {
                    None
                }
            );
            assert_eq!(detail.members.len(), 1);
        }
    }
    let app = crate::auth::app(store.clone()).await.unwrap();
    let response = router_request(
        &app,
        &auth[1],
        &format!("/campaigns/{campaign}?members_after=1&versions_after=17&sheets_after=23"),
        None,
    )
    .await;
    let html = String::from_utf8(
        axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains(&format!(
        "requests_after={}&amp;members_after=1&amp;versions_after=17&amp;sheets_after=23",
        accounts[49].account_id()
    )));
    let second = store
        .campaign_detail_page(
            auth[1].clone(),
            campaign,
            super::domain::DetailPage {
                requests: accounts[49].account_id(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(second.requests.len(), 1);
    assert_eq!(second.requests[0].account_id, accounts[50].account_id());
    assert_eq!(second.requests_next, None);
    let second_directory = store.workspace(auth[1].clone(), 50).await.unwrap();
    assert_eq!(second_directory.campaigns[0].id, 51);
    assert_eq!(second_directory.next, None);
    verify_sheet_and_history_pagination(&store, &auth[1], &accounts, campaign, &app).await;
}

async fn verify_sheet_and_history_pagination(
    store: &Store,
    master: &RequestAuth,
    accounts: &[RequestAuth],
    campaign: i64,
    app: &axum::Router,
) {
    for (index, account) in accounts.iter().take(51).enumerate() {
        store
            .decide_join(master.clone(), campaign, account.account_id(), 1, true)
            .await
            .unwrap();
        let mut sheet = native(campaign, account.account_id());
        sheet.sheet_id = i64::try_from(index + 1).unwrap();
        insert_native(store, account.clone(), sheet).await;
        let count = index + 1;
        if [1, 49, 50, 51].contains(&count) {
            let page = store
                .private_list_page(master.clone(), Some(campaign), 0)
                .await
                .unwrap();
            assert_eq!(page.rows.len(), count.min(50));
            assert_eq!(page.next, if count == 51 { Some(50) } else { None });
        }
    }
    // Personal listings must apply the lookahead after authorization, not count
    // other players' sheets as evidence of a further page.
    let personal = store
        .private_list_page(accounts[0].clone(), None, 0)
        .await
        .unwrap();
    assert_eq!(personal.rows.len(), 1);
    assert_eq!(personal.next, None);
    let second = store
        .private_list_page(master.clone(), Some(campaign), 50)
        .await
        .unwrap();
    assert_eq!(second.rows[0].id, 51);
    assert_eq!(second.next, None);
    let response = router_request(
        app,
        master,
        &format!("/campaigns/{campaign}?members_after=1&requests_after=7&versions_after=17"),
        None,
    )
    .await;
    let html = String::from_utf8(
        axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains(
        "sheets_after=50&amp;members_after=1&amp;requests_after=7&amp;versions_after=17"
    ));
    let actor = accounts[0].clone();
    store
        .run_campaign(actor.clone(), move |db| {
            let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let mut sheet = snapshots::load(&tx, 1)?;
            for revision in 2..=51 {
                sheet = snapshots::write(&tx, &actor, sheet, "edit", None)?;
                if [49, 50, 51].contains(&revision) {
                    assert_eq!(sheet.revision, revision);
                }
            }
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap();
    for (after, count, next) in [
        (0, 50, Some(50)),
        (1, 50, None),
        (2, 49, None),
        (50, 1, None),
    ] {
        let page = store
            .private_history_page(accounts[0].clone(), 1, after)
            .await
            .unwrap();
        assert_eq!(page.rows.len(), count);
        assert_eq!(page.next, next);
        assert_eq!(page.rows[0].revision, after + 1);
    }
}

#[tokio::test]
async fn approved_versions_paginate_without_resetting_other_sections() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    published_fixture(&store).await;
    let draft = store.definition_draft(1).await.unwrap();
    let mut definition = serde_json::to_value(draft.definition).unwrap();
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Versions".into(),
            "synthetic-game".into(),
            "first".into(),
        )
        .await
        .unwrap();
    for count in 1..=51 {
        if count > 1 {
            definition["title"] = serde_json::json!(format!("Version {count}"));
            let draft = store
                .save_definition_draft(serde_json::to_vec(&definition).unwrap(), count - 1)
                .await
                .unwrap();
            let comparison = store.compare_definition(draft.id, false).await.unwrap();
            store
                .approve_definition(
                    draft.id,
                    false,
                    crate::definitions::ApprovalRequest {
                        comparison_hash: comparison.comparison_hash,
                        actor: "Synthetic test".into(),
                        rationale: "Synthetic original compared".into(),
                        originals_compared: true,
                    },
                )
                .await
                .unwrap();
        }
        if [1, 49, 50, 51].contains(&count) {
            let page = store
                .campaign_detail(auth[1].clone(), campaign, 0)
                .await
                .unwrap();
            assert_eq!(page.versions.len(), usize::try_from(count.min(50)).unwrap());
            assert_eq!(
                page.versions_next,
                if count == 51 { Some(50) } else { None }
            );
        }
    }
    let page = store
        .campaign_detail_page(
            auth[1].clone(),
            campaign,
            super::domain::DetailPage {
                versions: 50,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(page.versions.len(), 1);
    assert_eq!(page.versions[0].0, 51);
    assert_eq!(page.versions_next, None);
    let app = crate::auth::app(store).await.unwrap();
    let response = router_request(
        &app,
        &auth[1],
        &format!("/campaigns/{campaign}?members_after=1&requests_after=7&sheets_after=23"),
        None,
    )
    .await;
    let html = String::from_utf8(
        axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains(
        "versions_after=50&amp;members_after=1&amp;requests_after=7&amp;sheets_after=23"
    ));
}

#[tokio::test]
async fn regression_retained_owner_join_request_is_not_offered_or_reported_as_server_failure() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Retained request".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    store
        .request_join(auth[0].clone().development_fixture(), campaign)
        .await
        .unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    let response = router_request(
        &app,
        &auth[1],
        &format!("/campaigns/{campaign}/requests/1"),
        Some(vec![
            ("csrf".into(), "a".repeat(64)),
            ("revision".into(), "1".into()),
            ("action".into(), "accept".into()),
        ]),
    )
    .await;
    assert_eq!(
        response.status(),
        axum::http::StatusCode::UNPROCESSABLE_ENTITY
    );
    assert!(!response.headers().contains_key("set-cookie"));
    let normal = store
        .campaign_detail(auth[1].clone(), campaign, 0)
        .await
        .unwrap();
    assert!(normal.requests.is_empty());
    assert_eq!(normal.requests_next, None);
    let development = store
        .campaign_detail(auth[1].clone().development_fixture(), campaign, 0)
        .await
        .unwrap();
    assert_eq!(development.requests.len(), 1);
    assert_eq!(development.requests[0].revision, 1);
}

async fn assert_private_counts(store: &Store, expected: i64) {
    store
        .run(move |db| {
            for table in ["private_sheets", "private_sheet_history"] {
                let count: i64 =
                    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                        row.get(0)
                    })?;
                assert_eq!(count, expected, "Unexpected {table} row count");
            }
            Ok(())
        })
        .await
        .unwrap();
}
