//! Master view slice (b1): the notification hub, publication after commit at every write path,
//! read-only admission and the finite snapshot probe (CAMP-14).
use super::*;
use crate::hub::Hint;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use tokio::sync::broadcast::{Receiver, error::TryRecvError};

const WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// Every hint published so far on `rx`, in order (a lagged gap is skipped).
fn drain(rx: &mut Receiver<Hint>) -> Vec<Hint> {
    let mut out = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(hint) => out.push(hint),
            Err(TryRecvError::Lagged(_)) => {}
            Err(_) => return out,
        }
    }
}

fn sheet_hint(sheet: i64, revision: i64) -> Hint {
    Hint::Sheet {
        campaign: 1,
        sheet,
        revision,
    }
}

pub(super) fn seen_hint(recipient: i64, sheet: i64) -> Hint {
    Hint::Seen {
        campaign: 1,
        recipient,
        sheet,
    }
}

fn role_hint(account: i64) -> Hint {
    Hint::Role {
        campaign: 1,
        account,
    }
}

// ---------------------------------------------------------------------------------------
// The hub itself

#[test]
fn a_hint_is_visible_only_for_its_campaign_and_a_seen_hint_only_to_its_recipient() {
    let sheet = sheet_hint(1, 2);
    let role = role_hint(9);
    let seen = seen_hint(4, 1);
    for account in [2, 4, 99] {
        assert!(sheet.visible_to(1, account) && role.visible_to(1, account));
        assert!(!sheet.visible_to(2, account) && !role.visible_to(2, account));
        assert_eq!(seen.visible_to(1, account), account == 4, "{account}");
        assert!(!seen.visible_to(2, 4), "another campaign");
    }
}

#[tokio::test]
async fn a_feed_skips_hints_that_are_not_for_it_and_stores_are_isolated() {
    let _locks = lock().await;
    let setup = world().await;
    let mut master_a = setup.store.feed(1, 2);
    let mut master_b = setup.store.feed(1, 4);
    let mut elsewhere = setup.store.feed(2, 4);
    // A clone shares the ring; a separately opened store does not.
    let other_dir = tempfile::tempdir().unwrap();
    let other = crate::Store::open(&other_dir.path().join("other.sqlite3")).unwrap();
    let mut other_rx = other.subscribe();
    setup.store.clone().publish([
        seen_hint(4, 1),
        Hint::Seen {
            campaign: 2,
            recipient: 4,
            sheet: 7,
        },
        sheet_hint(1, 5),
    ]);
    assert_eq!(master_b.recv().await.unwrap(), seen_hint(4, 1));
    assert_eq!(master_b.recv().await.unwrap(), sheet_hint(1, 5));
    // A never sees B's seen hint; the other campaign never sees campaign 1's hints.
    assert_eq!(master_a.recv().await.unwrap(), sheet_hint(1, 5));
    assert_eq!(
        elsewhere.recv().await.unwrap(),
        Hint::Seen {
            campaign: 2,
            recipient: 4,
            sheet: 7
        }
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), elsewhere.recv())
            .await
            .is_err()
    );
    assert_eq!(drain(&mut other_rx), Vec::<Hint>::new());
}

#[tokio::test]
async fn a_slow_subscriber_lags_without_blocking_commits() {
    let _locks = lock().await;
    let setup = world().await;
    let mut slow = setup.store.subscribe();
    // 260 real committing role changes (B: master at revision 2, demote, promote, ...) with a
    // subscriber that never reads: every commit completes, the ring keeps 256 hints.
    let commits = async {
        let mut revision = 2;
        for _ in 0..130 {
            for promote in [false, true] {
                setup
                    .store
                    .change_role(setup.auth[1].clone(), setup.campaign, 4, revision, promote)
                    .await
                    .unwrap();
                revision += 1;
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_mins(2), commits)
        .await
        .expect("commits are never blocked by a slow subscriber");
    assert_eq!(
        slow.recv().await,
        Err(tokio::sync::broadcast::error::RecvError::Lagged(4))
    );
    assert_eq!(slow.recv().await.unwrap(), role_hint(4));
    assert_eq!(drain(&mut slow).len(), 255, "the ring keeps 256 hints");
}

// ---------------------------------------------------------------------------------------
// Publication after commit

#[tokio::test]
async fn every_shared_save_publishes_the_sheet_hint_and_the_seen_hint_only_when_the_mark_moved() {
    let _locks = lock().await;
    for kind in [
        Kind::Name,
        Kind::Field,
        Kind::Cell,
        Kind::NewRow,
        Kind::RemoveRow,
        Kind::Quick,
        Kind::Switch,
        Kind::Undo,
    ] {
        for (marked, moved) in [("2", true), ("1", false)] {
            let setup = world().await;
            two_revisions(&setup).await;
            assert_eq!(
                location(&setup, 3, "/private-sheets/1/seen", seen_form(marked))
                    .await
                    .0,
                StatusCode::SEE_OTHER
            );
            let html = get(&setup, 3, "/private-sheets/1").await;
            let (path, pairs) = submission(kind, &html);
            let mut rx = setup.store.subscribe();
            let (status, answer) = send(&setup, 3, &path, Some(pairs)).await;
            assert_eq!(status, StatusCode::SEE_OTHER, "{kind:?}: {answer}");
            let mut want = vec![sheet_hint(1, 3)];
            if moved {
                want.push(seen_hint(4, 1));
            }
            assert_eq!(drain(&mut rx), want, "{kind:?}, marked {marked}");
        }
    }
}

#[tokio::test]
async fn creations_and_full_form_saves_publish_after_their_commit() {
    let _locks = lock().await;
    let setup = world().await;
    let mut rx = setup.store.subscribe();
    // A Master's creation (player or NPC) acknowledges revision 1: sheet hint, then seen hint.
    assert_eq!(create_as(&setup, 1, false).await, StatusCode::SEE_OTHER);
    assert_eq!(drain(&mut rx), vec![sheet_hint(1, 1), seen_hint(2, 1)]);
    assert_eq!(create_as(&setup, 3, true).await, StatusCode::SEE_OTHER);
    assert_eq!(drain(&mut rx), vec![sheet_hint(2, 1), seen_hint(4, 2)]);
    // A player's creation has no mark to move.
    assert_eq!(create_as(&setup, 2, false).await, StatusCode::SEE_OTHER);
    assert_eq!(drain(&mut rx), vec![sheet_hint(3, 1)]);
    // The full form acknowledges its new revision, also without a prior mark.
    let (status, html) = send(
        &setup,
        3,
        "/private-sheets/3",
        Some(form(1, &[("value-hp", "10")], &[row("r1-1", "1")])),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
    assert_eq!(
        drain(&mut rx),
        vec![sheet_hint(3, 2), seen_hint(4, 3)],
        "no prior mark"
    );
    let (status, html) = send(
        &setup,
        3,
        "/private-sheets/2",
        Some(form(1, &[("value-hp", "10")], &[row("r1-1", "1")])),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
    assert_eq!(
        drain(&mut rx),
        vec![sheet_hint(2, 2), seen_hint(4, 2)],
        "the mark moves 1 to 2"
    );
}

#[tokio::test]
async fn noops_refused_stale_invalid_and_rolled_back_saves_publish_nothing() {
    let _locks = lock().await;
    let setup = world().await;
    two_revisions(&setup).await;
    let mut rx = setup.store.subscribe();
    // A stale base: 409.
    let (status, _) = send(
        &setup,
        1,
        "/private-sheets/1",
        Some(form(1, &[("value-hp", "5")], &[row("r1-1", "1")])),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    // A no-op: the current revision, nothing changed.
    let (status, html) = send(
        &setup,
        1,
        "/private-sheets/1",
        Some(form(2, &[("value-hp", "-15")], &[row("r1-1", "1")])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{html}");
    // Refused: a player who does not hold the sheet, an outsider and the Owner.
    for who in [2, 4, 0] {
        let (status, _) = send(
            &setup,
            who,
            "/private-sheets/1",
            Some(form(2, &[("value-hp", "5")], &[row("r1-1", "1")])),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{who}");
    }
    // An unknown form key.
    let (status, _) = send(
        &setup,
        1,
        "/private-sheets/1",
        Some(form(2, &[("value-unknown", "1")], &[row("r1-1", "1")])),
    )
    .await;
    assert!(status.is_client_error(), "{status}");
    // A rolled-back save (the notes step fails after the sheet and the mark were written).
    *crate::campaigns::notes::BEFORE_INSERT.lock().unwrap() =
        Some(Box::new(|_| Err(crate::AppError::Conflict)));
    let before = seen_of(&setup, 2, 1).await;
    let (status, _) = send(
        &setup,
        1,
        "/private-sheets/1",
        Some(form(2, &[("value-hp", "7")], &[row("r1-1", "1")])),
    )
    .await;
    assert!(status.is_client_error(), "{status}");
    assert_eq!(saved(&setup, 1).await.0, 2, "rolled back");
    assert_eq!(seen_of(&setup, 2, 1).await, before);
    assert_eq!(drain(&mut rx), Vec::<Hint>::new(), "nothing was committed");
    // Seen: a revision the history lacks, and a player.
    assert_eq!(
        location(&setup, 3, "/private-sheets/1/seen", seen_form("99"))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        location(&setup, 2, "/private-sheets/1/seen", seen_form("1"))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(drain(&mut rx), Vec::<Hint>::new());
}

#[tokio::test]
async fn note_only_and_empty_note_edits_publish_and_keep_the_note_hook_behavior() {
    let _locks = lock().await;
    let setup = world().await;
    assert_eq!(create_as(&setup, 1, false).await, StatusCode::SEE_OTHER);
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let record = calls.clone();
    setup
        .store
        .set_note_hook(Arc::new(move |sheet, revision, addresses| {
            record
                .lock()
                .unwrap()
                .push((sheet, revision, addresses.to_vec()));
        }))
        .unwrap();
    let mut rx = setup.store.subscribe();
    // An edit with change notes: the hook runs and the hint is published (the hint is the
    // invalidation of the revision, never a substitute for the hook).
    let (status, html) = send(
        &setup,
        1,
        "/private-sheets/1",
        Some(form(1, &[("value-hp", "50")], &[row("r1-1", "1")])),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
    assert_eq!(drain(&mut rx), vec![sheet_hint(1, 2), seen_hint(2, 1)]);
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert_eq!(calls.lock().unwrap()[0].1, 2);
    // A revision whose note list is empty (a rename; a sheet-notes-only edit): the hook ignores
    // it as before, the hint is still published.
    for (n, change) in [("name", "Renamed"), ("notes", "Sheet notes only")]
        .into_iter()
        .enumerate()
    {
        let revision = 2 + i64::try_from(n).unwrap();
        let (status, html) = send(
            &setup,
            1,
            "/private-sheets/1",
            Some(form(
                revision,
                &[("value-hp", "50"), change],
                &[row("r1-1", "1")],
            )),
        )
        .await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{change:?}: {html}");
        assert_eq!(
            drain(&mut rx),
            vec![sheet_hint(1, revision + 1), seen_hint(2, 1)],
            "{change:?}"
        );
    }
    assert_eq!(
        calls.lock().unwrap().len(),
        1,
        "empty note lists never call the hook"
    );
}

#[tokio::test]
async fn a_departure_publishes_every_freeze_only_after_the_whole_transaction() {
    let _locks = lock().await;
    let setup = world().await;
    assert_eq!(create_as(&setup, 2, false).await, StatusCode::SEE_OTHER);
    assert_eq!(create_as(&setup, 2, false).await, StatusCode::SEE_OTHER);
    // The final freeze fails: nothing is committed and nothing is published.
    setup.store.run(|db| {
        db.execute_batch("CREATE TRIGGER fail_last_history BEFORE INSERT ON private_sheet_history WHEN NEW.sheet_id=2 AND NEW.operation='freeze' BEGIN SELECT RAISE(ABORT,'synthetic disk failure'); END;")?;
        Ok(())
    }).await.unwrap();
    let mut rx = setup.store.subscribe();
    assert!(
        setup
            .store
            .depart(setup.auth[1].clone(), setup.campaign, 3, 1)
            .await
            .is_err()
    );
    assert!(
        drain(&mut rx).is_empty(),
        "a rolled-back departure publishes nothing"
    );
    setup
        .store
        .run(|db| {
            db.execute_batch("DROP TRIGGER fail_last_history")?;
            Ok(())
        })
        .await
        .unwrap();
    // The hints arrive only once both freezes and the membership change are committed.
    let store = setup.store.clone();
    let mut witness = setup.store.subscribe();
    let watcher = tokio::spawn(async move {
        witness.recv().await.unwrap();
        store
            .run(|db| {
                Ok(db.query_row(
                    "SELECT (SELECT count(*) FROM private_sheets WHERE state='frozen'),(SELECT count(*) FROM campaign_members WHERE account_id=3 AND state='inactive')",
                    [],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
                )?)
            })
            .await
            .unwrap()
    });
    setup
        .store
        .depart(setup.auth[1].clone(), setup.campaign, 3, 1)
        .await
        .unwrap();
    assert_eq!(watcher.await.unwrap(), (2, 1));
    assert_eq!(
        drain(&mut rx),
        vec![role_hint(3), sheet_hint(1, 2), sheet_hint(2, 2)]
    );
}

#[tokio::test]
async fn role_changes_publish_a_role_hint_and_refused_ones_do_not() {
    let _locks = lock().await;
    let setup = world().await;
    let mut rx = setup.store.subscribe();
    let store = &setup.store;
    let a = || setup.auth[1].clone();
    // Demotion and promotion (B: master at revision 2).
    store.change_role(a(), 1, 4, 2, false).await.unwrap();
    assert_eq!(drain(&mut rx), vec![role_hint(4)]);
    assert!(
        store.change_role(a(), 1, 4, 2, true).await.is_err(),
        "stale revision"
    );
    assert_eq!(drain(&mut rx), Vec::<Hint>::new());
    store.change_role(a(), 1, 4, 3, true).await.unwrap();
    assert_eq!(drain(&mut rx), vec![role_hint(4)]);
    // A request and a decline change no role; the accepted request is a join.
    store.request_join(setup.auth[4].clone(), 1).await.unwrap();
    store.decide_join(a(), 1, 5, 1, false).await.unwrap();
    store.request_join(setup.auth[4].clone(), 1).await.unwrap();
    assert_eq!(drain(&mut rx), Vec::<Hint>::new());
    store.decide_join(a(), 1, 5, 3, true).await.unwrap();
    assert_eq!(drain(&mut rx), vec![role_hint(5)]);
    assert!(store.decide_join(a(), 1, 5, 3, true).await.is_err());
    assert_eq!(drain(&mut rx), Vec::<Hint>::new());
    // A player leaving.
    store.depart(setup.auth[4].clone(), 1, 5, 1).await.unwrap();
    assert_eq!(drain(&mut rx), vec![role_hint(5)]);
}

#[tokio::test]
async fn the_seen_post_publishes_to_its_recipient_only_when_the_mark_changed() {
    let _locks = lock().await;
    let setup = world().await;
    two_revisions(&setup).await;
    let mut rx = setup.store.subscribe();
    let mut feed_a = setup.store.feed(1, 2);
    let mut feed_b = setup.store.feed(1, 4);
    for (revision, hints) in [
        ("1", vec![seen_hint(4, 1)]),
        ("1", vec![]),
        ("2", vec![seen_hint(4, 1)]),
        ("1", vec![]),
        ("2", vec![]),
    ] {
        assert_eq!(
            location(&setup, 3, "/private-sheets/1/seen", seen_form(revision))
                .await
                .0,
            StatusCode::SEE_OTHER
        );
        assert_eq!(drain(&mut rx), hints, "revision {revision}");
    }
    // A's own later save is the next hint on A's feed: B's seen hints never reached A.
    let (status, html) = send(
        &setup,
        1,
        "/private-sheets/1",
        Some(form(2, &[("value-hp", "1")], &[row("r1-1", "1")])),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
    assert_eq!(feed_a.recv().await.unwrap(), sheet_hint(1, 3));
    assert_eq!(feed_a.recv().await.unwrap(), seen_hint(2, 1));
    assert_eq!(feed_b.recv().await.unwrap(), seen_hint(4, 1));
}

#[tokio::test]
async fn caller_cancellation_after_commit_still_publishes() {
    let _locks = lock().await;
    let setup = world().await;
    assert_eq!(create_as(&setup, 1, false).await, StatusCode::SEE_OTHER);
    let permit = crate::definitions::admit().unwrap();
    let (mut view, permit) = setup
        .store
        .private_read(setup.auth[1].clone(), 1, None, permit)
        .await
        .unwrap();
    view.snapshot.notes = "Changed while cancelled".into();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    *crate::campaigns::sheets::BEFORE_SAVE_RECHECK
        .lock()
        .unwrap() = Some(Box::new(move |_| {
        entered_tx.send(()).unwrap();
        go_rx.recv_timeout(WAIT).unwrap();
        Ok(())
    }));
    let mut rx = setup.store.subscribe();
    let (store, auth) = (setup.store.clone(), setup.auth[1].clone());
    let task = tokio::spawn(async move { store.private_save(auth, view, permit).await });
    tokio::task::spawn_blocking(move || entered_rx.recv_timeout(WAIT).unwrap())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    go_tx.send(()).unwrap();
    // The committing task outlives the cancelled caller and publishes.
    assert_eq!(
        tokio::time::timeout(WAIT, rx.recv())
            .await
            .unwrap()
            .unwrap(),
        sheet_hint(1, 2)
    );
    assert_eq!(saved(&setup, 1).await.0, 2);
}

#[tokio::test]
async fn subscribing_before_the_snapshot_misses_no_commit() {
    let _locks = lock().await;
    let setup = world().await;
    assert_eq!(create_as(&setup, 1, false).await, StatusCode::SEE_OTHER);
    let mut feed = setup.store.feed(1, 2);
    // A commit between the subscription and the snapshot: the snapshot already shows it and the
    // hint is still delivered (a consumer coalesces it).
    let (status, _) = send(
        &setup,
        1,
        "/private-sheets/1",
        Some(form(1, &[("value-hp", "9")], &[row("r1-1", "1")])),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let snapshot = |setup: &Setup| {
        let (store, auth) = (setup.store.clone(), setup.auth[1].clone());
        async move {
            store
                .master_workspace(auth, 1, crate::campaigns::master::MasterPage::default())
                .await
                .unwrap()
                .players
                .rows[0]
                .revision
        }
    };
    assert_eq!(snapshot(&setup).await, 2);
    assert_eq!(feed.recv().await.unwrap(), sheet_hint(1, 2));
    assert_eq!(feed.recv().await.unwrap(), seen_hint(2, 1));
    // A commit after the snapshot is hinted and read back authoritatively.
    let (status, _) = send(
        &setup,
        1,
        "/private-sheets/1",
        Some(form(2, &[("value-hp", "8")], &[row("r1-1", "1")])),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(feed.recv().await.unwrap(), sheet_hint(1, 3));
    assert_eq!(snapshot(&setup).await, 3);
}

// ---------------------------------------------------------------------------------------
// Read-only admission and the snapshot probe

fn snapshot_path(setup: &Setup) -> String {
    format!("/campaigns/{}/master/snapshot", setup.campaign)
}

async fn raw_get(setup: &Setup, path: &str, headers: &[(&str, &str)]) -> axum::response::Response {
    use tower::ServiceExt;
    let mut request = axum::http::Request::builder()
        .uri(path)
        .header("host", "127.0.0.1:3000")
        .header(crate::auth::TEST_DEADLINE_HEADER, "120");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    setup
        .app
        .clone()
        .oneshot(request.body(axum::body::Body::empty()).unwrap())
        .await
        .unwrap()
}

pub(super) async fn snapshot_as(
    setup: &Setup,
    who: usize,
    headers: &[(&str, &str)],
) -> (StatusCode, axum::http::HeaderMap, String) {
    let cookie = setup.auth[who].fixture_cookie();
    let mut all = vec![("cookie", cookie.as_str())];
    all.extend_from_slice(headers);
    let response = raw_get(setup, &snapshot_path(setup), &all).await;
    let (status, headers) = (response.status(), response.headers().clone());
    (
        status,
        headers,
        super::super::formula_tests::body(response).await,
    )
}

/// Every session row, byte for byte.
async fn session_rows(setup: &Setup) -> Vec<String> {
    setup
        .store
        .run(|db| {
            let mut q = db.prepare(
                "SELECT id||'|'||issued_at||'|'||expires_at||'|'||state||'|'||coalesce(payload,'<null>')||'|'||coalesce(account_id,-1)||'|'||coalesce(epoch,-1) FROM auth_sessions ORDER BY id",
            )?;
            Ok(q.query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .await
        .unwrap()
}

/// An anonymous session (no principal) is 401 and a corrupt row 500; neither writes anything.
async fn anonymous_and_corrupt_sessions(setup: &Setup) {
    {
        use tower_sessions::SessionStore as _;
        let mut data = std::collections::HashMap::new();
        data.insert("csrf".to_owned(), json!("a".repeat(64)));
        let mut record = tower_sessions::session::Record {
            id: tower_sessions::session::Id::default(),
            data,
            expiry_date: time::OffsetDateTime::from_unix_timestamp(500).unwrap(),
        };
        crate::auth::SqliteSessions::new(setup.store.clone())
            .create(&mut record)
            .await
            .unwrap();
        let rows = session_rows(setup).await;
        let cookie = format!("mimik_session={}", record.id);
        let response = raw_get(setup, &snapshot_path(setup), &[("cookie", cookie.as_str())]).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "anonymous");
        assert_eq!(session_rows(setup).await, rows, "anonymous session");
        setup
            .store
            .run(|db| {
                // The update trigger refuses such a write; a damaged file is simulated without it.
                db.execute_batch("DROP TRIGGER auth_sessions_update")?;
                db.execute(
                    r#"UPDATE auth_sessions SET payload='{"csrf":5}' WHERE account_id=5"#,
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let rows = session_rows(setup).await;
        let response = snapshot_as(setup, 4, &[]).await;
        assert_eq!(response.0, StatusCode::INTERNAL_SERVER_ERROR, "corrupt row");
        assert_eq!(session_rows(setup).await, rows, "corrupt row");
    }
}

#[tokio::test]
async fn read_only_admission_never_writes_the_session_store() {
    let _locks = lock().await;
    let setup = world().await;
    let clock = Arc::new(AtomicI64::new(100));
    let c = clock.clone();
    setup
        .store
        .run(move |db| {
            crate::store::register_auth_clock(db, Arc::new(move || Ok(c.load(Ordering::SeqCst))))
        })
        .await
        .unwrap();
    let before = session_rows(&setup).await;
    assert_eq!(before.len(), 5);
    // Valid.
    assert_eq!(snapshot_as(&setup, 1, &[]).await.0, StatusCode::OK);
    assert_eq!(session_rows(&setup).await, before, "valid session");
    // Revoked (as logout leaves it).
    setup
        .store
        .run(|db| {
            db.execute(
                "UPDATE auth_sessions SET state='revoked',payload=NULL WHERE account_id=4",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let revoked = session_rows(&setup).await;
    assert_eq!(
        snapshot_as(&setup, 3, &[]).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(session_rows(&setup).await, revoked, "revoked session");
    // A disabled account.
    setup
        .store
        .run(|db| {
            db.execute(
                "UPDATE accounts SET state='disabled',epoch=2 WHERE id=3",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let disabled = session_rows(&setup).await;
    assert_eq!(
        snapshot_as(&setup, 2, &[]).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(session_rows(&setup).await, disabled, "disabled account");
    anonymous_and_corrupt_sessions(&setup).await;
    let disabled = session_rows(&setup).await;
    // Missing, unknown and malformed cookies, and two cookies.
    for cookie in [
        None,
        Some("mimik_session=AAAAAAAAAAAAAAAAAAAAAA".to_owned()),
        Some("mimik_session=not-a-session".to_owned()),
        Some("other=1".to_owned()),
        Some(format!(
            "{}; {}",
            setup.auth[1].fixture_cookie(),
            setup.auth[3].fixture_cookie()
        )),
    ] {
        let headers: Vec<(&str, &str)> = cookie.iter().map(|c| ("cookie", c.as_str())).collect();
        let response = raw_get(&setup, &snapshot_path(&setup), &headers).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{cookie:?}");
        assert_eq!(session_rows(&setup).await, disabled, "{cookie:?}");
    }
    // Expired: the absolute deadline (1000) has passed.
    clock.store(2000, Ordering::SeqCst);
    assert_eq!(
        snapshot_as(&setup, 1, &[]).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(session_rows(&setup).await, disabled, "expired session");
}

#[tokio::test]
async fn the_snapshot_answers_masters_alone_and_everyone_else_uniformly() {
    let _locks = lock().await;
    let setup = world().await;
    assert_eq!(create_as(&setup, 1, false).await, StatusCode::SEE_OTHER);
    assert_eq!(create_as(&setup, 3, true).await, StatusCode::SEE_OTHER);
    // A Master of another campaign only (account 5).
    setup
        .store
        .create_campaign(
            setup.auth[4].clone(),
            "Other".into(),
            "synthetic-game".into(),
            "first".into(),
        )
        .await
        .unwrap();
    // Former Master: B, demoted.
    setup
        .store
        .change_role(setup.auth[1].clone(), setup.campaign, 4, 2, false)
        .await
        .unwrap();
    let mut refused = Vec::new();
    for who in [0, 2, 3, 4] {
        let (status, headers, body) = snapshot_as(&setup, who, &[]).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{who}");
        assert_eq!(headers["cache-control"], "no-store");
        assert!(!body.to_lowercase().contains("npc"), "{who}: {body}");
        nothing_leaks(&body);
        refused.push(body);
    }
    assert!(
        refused.windows(2).all(|pair| pair[0] == pair[1]),
        "uniform: {refused:?}"
    );
    let cookie_a = setup.auth[1].fixture_cookie();
    let missing = raw_get(
        &setup,
        "/campaigns/99/master/snapshot",
        &[("cookie", cookie_a.as_str())],
    )
    .await;
    assert_eq!(missing.status(), StatusCode::FORBIDDEN);
    // No session at all: 401, not a redirect.
    let anonymous = raw_get(&setup, &snapshot_path(&setup), &[]).await;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(anonymous.headers()["cache-control"], "no-store");
    // Master A: JSON, no-store, shaped as documented.
    let (status, headers, body) = snapshot_as(&setup, 1, &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(headers["content-type"], "application/json");
    assert_eq!(headers["cache-control"], "no-store");
    assert!(
        headers
            .keys()
            .all(|name| !name.as_str().starts_with("access-control-"))
    );
    let json: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        json,
        json!({
            "players": {"rows": [{"id":"1","name":"Assisted sheet","custodian":"player2","revision":"1","state":"active","npc":false,"seen":"1"}], "next": null},
            "npcs": {"rows": [{"id":"2","name":"Assisted sheet","custodian":"player4","revision":"1","state":"active","npc":true,"seen":null}], "next": null},
        })
    );
    // The page's cursor rules: nonnegative integers and nothing else.
    for bad in ["players_after=-1", "npcs_after=x", "other=1"] {
        let response = raw_get(
            &setup,
            &format!("{}?{bad}", snapshot_path(&setup)),
            &[("cookie", cookie_a.as_str())],
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{bad}");
    }
}

#[tokio::test]
async fn the_snapshot_pages_each_section_on_its_own_cursor() {
    let _locks = lock().await;
    let setup = world().await;
    setup
        .store
        .request_join(setup.auth[4].clone(), setup.campaign)
        .await
        .unwrap();
    setup
        .store
        .decide_join(setup.auth[1].clone(), setup.campaign, 5, 1, true)
        .await
        .unwrap();
    for n in 0..51 {
        plant(
            &setup,
            if n < 26 { 2 } else { 4 },
            if n < 26 { 3 } else { 5 },
            false,
        )
        .await;
        plant(
            &setup,
            if n < 26 { 1 } else { 3 },
            if n < 26 { 2 } else { 4 },
            true,
        )
        .await;
    }
    let cookie = setup.auth[1].fixture_cookie();
    let fetch = |query: String| {
        let (setup, cookie) = (&setup, cookie.clone());
        async move {
            let response = raw_get(
                setup,
                &format!("{}{query}", snapshot_path(setup)),
                &[("cookie", cookie.as_str())],
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            serde_json::from_str::<Value>(&super::super::formula_tests::body(response).await)
                .unwrap()
        }
    };
    let first = fetch(String::new()).await;
    assert_eq!(first["players"]["rows"].as_array().unwrap().len(), 50);
    assert_eq!(first["npcs"]["rows"].as_array().unwrap().len(), 50);
    let (players_next, npcs_next) = (
        first["players"]["next"].as_str().unwrap().to_owned(),
        first["npcs"]["next"].as_str().unwrap().to_owned(),
    );
    assert_eq!(
        first["players"]["rows"][49]["id"].as_str().unwrap(),
        players_next
    );
    let second = fetch(format!("?players_after={players_next}&npcs_after=0")).await;
    assert_eq!(second["players"]["rows"].as_array().unwrap().len(), 1);
    assert_eq!(second["players"]["next"], Value::Null);
    assert_eq!(second["npcs"], first["npcs"], "the other cursor is kept");
    let third = fetch(format!("?players_after=0&npcs_after={npcs_next}")).await;
    assert_eq!(third["npcs"]["rows"].as_array().unwrap().len(), 1);
    assert_eq!(third["players"], first["players"]);
    let empty = fetch("?players_after=100000&npcs_after=100000".into()).await;
    assert_eq!(empty["players"], json!({"rows": [], "next": null}));
}

#[tokio::test]
async fn the_snapshot_refuses_cross_site_requests_and_foreign_origins() {
    let _locks = lock().await;
    let setup = world().await;
    assert_eq!(create_as(&setup, 1, false).await, StatusCode::SEE_OTHER);
    let ok = |headers: &'static [(&'static str, &'static str)]| {
        let setup = &setup;
        async move { snapshot_as(setup, 1, headers).await }
    };
    for headers in [
        &[("origin", "http://127.0.0.1:3000")][..],
        &[("sec-fetch-site", "same-origin")],
        &[("sec-fetch-site", "none")],
        &[
            ("sec-fetch-site", "same-origin"),
            ("origin", "http://127.0.0.1:3000"),
        ],
    ] {
        let (status, response_headers, body) = ok(headers).await;
        assert_eq!(status, StatusCode::OK, "{headers:?}: {body}");
        assert!(
            response_headers
                .keys()
                .all(|name| !name.as_str().starts_with("access-control-"))
        );
    }
    for headers in [
        &[("origin", "http://evil.example")][..],
        &[("origin", "http://127.0.0.1:3001")],
        &[("origin", "null")],
        &[
            ("origin", "http://127.0.0.1:3000"),
            ("origin", "http://127.0.0.1:3000"),
        ],
        &[("sec-fetch-site", "cross-site")],
        &[("sec-fetch-site", "same-site")],
        &[
            ("sec-fetch-site", "same-origin"),
            ("sec-fetch-site", "same-origin"),
        ],
    ] {
        let (status, response_headers, _) = ok(headers).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{headers:?}");
        assert!(
            response_headers
                .keys()
                .all(|name| !name.as_str().starts_with("access-control-"))
        );
    }
}

#[test]
fn the_snapshot_encoder_uses_decimal_strings_and_rejects_overflow() {
    use crate::campaigns::master::{MasterSheet, MasterWorkspace};
    use crate::campaigns::{Campaign, Listing};
    let row = |id: i64, name: String| MasterSheet {
        id,
        name,
        custodian: "C".into(),
        revision: i64::MAX,
        state: "active".into(),
        npc: true,
        seen: Some(i64::MAX - 1),
    };
    let workspace = |players: Vec<MasterSheet>, npcs: Vec<MasterSheet>| MasterWorkspace {
        campaign: Campaign {
            id: 1,
            creator_id: 1,
            title: String::new(),
            game_id: String::new(),
            edition_id: String::new(),
            creator: String::new(),
        },
        players: Listing {
            rows: players,
            next: Some(i64::MAX),
        },
        npcs: Listing {
            rows: npcs,
            next: None,
        },
    };
    let json: Value = serde_json::from_str(
        &workspace(vec![row(i64::MAX, "x".into())], vec![])
            .snapshot_json()
            .unwrap(),
    )
    .unwrap();
    let first = &json["players"]["rows"][0];
    assert_eq!(first["id"], "9223372036854775807");
    assert_eq!(first["revision"], "9223372036854775807");
    assert_eq!(first["seen"], "9223372036854775806");
    assert_eq!(json["players"]["next"], "9223372036854775807");
    // A row of 2 KiB or more is refused, never truncated.
    assert!(
        workspace(vec![row(1, "n".repeat(2100))], vec![])
            .snapshot_json()
            .is_none()
    );
    assert!(
        workspace(vec![], vec![row(1, "n".repeat(2100))])
            .snapshot_json()
            .is_none()
    );
    assert!(
        workspace(vec![row(1, "n".repeat(1500))], vec![])
            .snapshot_json()
            .is_some()
    );
    // More than 100 rows is refused too. Through the route this cannot happen (two pages of 50);
    // the check is defence in depth, tested here with constructed data.
    let many = |n: i64| (1..=n).map(|id| row(id, "n".into())).collect::<Vec<_>>();
    assert!(workspace(many(50), many(50)).snapshot_json().is_some());
    assert!(workspace(many(51), many(50)).snapshot_json().is_none());
    // The body limit (256 KiB in production, unreachable with rows of at most 2 KiB) is tested
    // with a lowered limit: one small row's body fits its exact length and not one byte less.
    let one = workspace(many(1), vec![]);
    let body = one.snapshot_json().unwrap();
    assert!(one.snapshot_json_within(body.len()).is_some());
    assert!(one.snapshot_json_within(body.len() - 1).is_none());
}
