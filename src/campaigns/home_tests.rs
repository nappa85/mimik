//! The home page (`docs/auth-and-sessions.md`, Home page): each section for the Owner and for a
//! player and Master on seeded databases, the row bound, the mark, and what a role never reads.
use super::formula_tests::body;
use super::{fixture, insert_sheet, published_fixture, router_request, sheet};
use crate::{Store, auth::RequestAuth};
use axum::http::StatusCode;
use rusqlite::params;
use std::sync::{Arc, Mutex, atomic::AtomicI64, atomic::Ordering};
use tower::ServiceExt;

const CSRF: &str = "a";

fn csrf() -> String {
    CSRF.repeat(64)
}

async fn home(app: &axum::Router, auth: &RequestAuth, owner: bool) -> String {
    let response = router_request(app, auth, "/", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body(response).await;
    crate::auth::nav::check::assert_header(&html, owner, None, false, "home");
    assert!(html.contains("<a class=\"logo\" href=\"/\" aria-current=\"page\">"));
    html
}

/// The fixture clock's start: the second the home was rendered in the tests that never move it.
const NOW: i64 = 100;

async fn mark(
    app: &axum::Router,
    auth: &RequestAuth,
    token: &str,
    seen: i64,
) -> axum::response::Response {
    mark_at(app, auth, token, seen, NOW).await
}

async fn mark_at(
    app: &axum::Router,
    auth: &RequestAuth,
    token: &str,
    seen: i64,
    shown_at: i64,
) -> axum::response::Response {
    router_request(
        app,
        auth,
        "/home/seen",
        Some(vec![
            ("csrf".into(), token.into()),
            ("seen".into(), seen.to_string()),
            ("shown_at".into(), shown_at.to_string()),
        ]),
    )
    .await
}

async fn stored_mark(store: &Store, account: i64) -> Option<i64> {
    store
        .run(move |db| {
            Ok(db
                .query_row(
                    "SELECT seen_at FROM account_home_marks WHERE account_id=?1",
                    [account],
                    |row| row.get(0),
                )
                .ok())
        })
        .await
        .unwrap()
}

/// Tables read by the statements `work` prepares and runs (a SQLite authorizer on the store's
/// connection).
async fn tables_read<F, Fut>(store: &Store, work: F) -> Vec<String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = seen.clone();
    store
        .run(move |db| {
            db.authorizer(Some(move |ctx: rusqlite::hooks::AuthContext<'_>| {
                if let rusqlite::hooks::AuthAction::Read { table_name, .. } = ctx.action {
                    sink.lock().unwrap().push(table_name.to_owned());
                }
                rusqlite::hooks::Authorization::Allow
            }))
            .unwrap();
            Ok(())
        })
        .await
        .unwrap();
    work().await;
    store
        .run(|db| {
            db.authorizer(
                None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>,
            )
            .unwrap();
            Ok(())
        })
        .await
        .unwrap();
    let mut tables = seen.lock().unwrap().clone();
    tables.sort();
    tables.dedup();
    tables
}

const OWNER_TABLES: [&str; 9] = [
    "sources",
    "source_pages",
    "source_uploads",
    "source_jobs",
    "source_suggestions",
    "source_suggestion_reports",
    "account_credentials",
    "account_events",
    "assistant_downloads",
];
const CAMPAIGN_TABLES: [&str; 5] = [
    "campaigns",
    "campaign_members",
    "campaign_requests",
    "campaign_events",
    "private_sheets",
];

async fn seed_source(store: &Store, id: i64, title: &str, complete: &[bool]) {
    // Clear of the published fixture's own source and pages.
    let id = id + 1000;
    let title = title.to_owned();
    let complete = complete.to_vec();
    store
        .run(move |db| {
            db.execute("INSERT INTO sources(id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,raw_json,prepared_json,total_pages) VALUES(?1,?2,?3,?4,'mork-borg','Edition','en',?5,x'',x'',?6)",
                params![id, format!("{id:064}"), format!("source-{id}"), title, "b".repeat(64), complete.len()])?;
            for (index, done) in complete.iter().enumerate() {
                let page = id * 100 + i64::try_from(index).unwrap();
                db.execute("INSERT INTO source_pages(id,source_id,pdf_page,png) VALUES(?1,?2,?3,x'89504e47')",
                    params![page, id, index + 1])?;
                if *done {
                    db.execute("UPDATE source_pages SET complete=1,revision=2 WHERE id=?1", [page])?;
                }
            }
            Ok(())
        })
        .await
        .unwrap();
}

async fn seed_owner(store: &Store) {
    seed_source(store, 1, "Open <book>", &[true, false, false]).await;
    seed_source(store, 2, "Finished book", &[true, true]).await;
    store
        .run(|db| {
            // An upload that failed at 50 (pending and finished).
            db.execute("INSERT INTO source_uploads(id,pdf_sha256,pdf_bytes,actor,actor_id,actor_epoch,session_digest,created_at,updated_at,state,system_id,edition,language) VALUES(?1,?2,10,'account:1',1,1,?3,40,50,'failed','mork-borg','Upload edition','en')",
                params!["1".repeat(32), "c".repeat(64), "d".repeat(64)])?;
            db.execute("INSERT INTO source_jobs(upload_id,upload_revision,kind,state,reason,created_at,finished_at) VALUES(?1,1,'inspect','failed','timeout',40,50)", ["1".repeat(32)])?;
            // A pending invitation (account 6) and one redeemed at 70 (account 3).
            db.execute("INSERT INTO accounts(id,login,display_name,state,epoch) VALUES(6,'invited','Invited <guest>','pending',1)", [])?;
            db.execute("INSERT INTO account_credentials VALUES(zeroblob(32),6,'enroll',1,90,86490,'active')", [])?;
            db.execute("INSERT INTO account_events(account_id,epoch,occurred_at,actor,action) VALUES(3,1,70,'credential_redemption','enroll')", [])?;
            // A suggestion report imported at 80, an open suggestion and a download in progress.
            db.execute("INSERT INTO source_suggestion_reports(source_id,report_sha256,origin,schema_version,binding,created_at,imported_at,correction,reject,merge,missing_text,note,failed_pages) VALUES(1001,?1,'tool:x',1,'bundle','t','1970-01-01 00:01:20',2,0,1,0,0,0)", ["e".repeat(64)])?;
            db.execute("INSERT INTO assistant_downloads(spec,file_name,sha256,size,confirmed_by,confirmed_at,state) VALUES(?1,'model-q4.gguf',?2,100,1,60,'confirmed')",
                params![format!("hf:org/repo/model-q4.gguf@{}", "f".repeat(40)), "9".repeat(64)])?;
            Ok(())
        })
        .await
        .unwrap();
    // Two suggestions on source 1; the second is dismissed, so only the first is open.
    store
        .run(|db| {
            db.execute("INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(1001,100101,'u1','Text','[]','<p>x</p>','x')", [])?;
            Ok(())
        })
        .await
        .unwrap();
    let mut ids = Vec::new();
    for _ in 0..2 {
        ids.push(
            store
                .insert_source_suggestion(crate::sources::SourceSuggestion {
                    source: 1001,
                    bundle_hash: format!("{:064}", 1001),
                    origin: "tool:fixture".into(),
                    kind: "reject".into(),
                    target: crate::sources::SuggestionTarget::Unit {
                        target: crate::sources::SuggestionUnit {
                            unit: 1001,
                            revision: 1,
                        },
                    },
                    suggested_text: None,
                    rationale: "Compare synthetic original".into(),
                    report_id: None,
                })
                .await
                .unwrap(),
        );
    }
    let dismissed = ids[1];
    store
        .run(move |db| {
            db.execute("INSERT INTO source_suggestion_dismissals(suggestion_id,actor) VALUES(?1,'account:1')", [dismissed])?;
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn owner_home_empty_states_and_no_mark() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let html = home(&app, &auth[0], true).await;
    for empty in [
        "Every imported page is complete.",
        "No upload is running or failed.",
        "No suggestion is waiting.",
        "No invitation is waiting.",
        "No download is waiting.",
        "No upload finished.",
        "No invitation was redeemed.",
        "No suggestions were added.",
    ] {
        assert!(html.contains(empty), "{empty}");
    }
    assert!(html.contains("Latest changes"));
    assert!(!html.contains("Mark as seen"), "nothing to mark");
    assert!(html.contains("/admin/materials/uploads"), "quick links");
}

#[tokio::test]
async fn owner_home_sections_and_mark() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed_owner(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let html = home(&app, &auth[0], true).await;
    // Pending: page-level counts, the first page needing attention, no finished source.
    assert!(html.contains("Open &#60;book&#62;") || html.contains("Open &lt;book&gt;"));
    assert!(html.contains("2 of 3 imported pages not complete"));
    assert!(html.contains("href=\"/admin/materials/pages/100101\""));
    assert!(!html.contains("Finished book"));
    assert!(html.contains("failed (timeout)"));
    assert!(html.contains("open reject suggestion"));
    assert_eq!(
        html.matches("open reject suggestion").count(),
        1,
        "the dismissed one is not open"
    );
    assert!(html.contains("href=\"/admin/materials/suggestions/1\""));
    assert!(html.contains("Invited &#60;guest&#62;") || html.contains("Invited &lt;guest&gt;"));
    assert!(html.contains("invitation not redeemed, expires"));
    assert!(html.contains("model-q4.gguf"));
    // News: the three items (50, 70 and 80), no mark yet.
    assert!(html.contains("Latest changes"));
    assert!(html.contains("upload failed (timeout)"));
    assert!(html.contains("redeemed an invitation"));
    assert!(html.contains("3 suggestions imported"));
    assert!(html.contains("<input type=\"hidden\" name=\"seen\" value=\"80\">"));
    assert!(html.contains("01 January 1970, 00:01 UTC"));

    // A mark needs the CSRF token.
    for bad in ["b".repeat(64), String::new(), "a".into()] {
        assert_eq!(
            mark(&app, &auth[0], &bad, 80).await.status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(stored_mark(&store, 1).await, None);
    // Not in the future (the fixture clock is 100), not beyond what is shown, not zero.
    for seen in [101, 81, 0, -5] {
        assert_eq!(
            mark(&app, &auth[0], &csrf(), seen).await.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{seen}"
        );
    }
    assert_eq!(stored_mark(&store, 1).await, None);
    // Marking part of it moves the mark there only.
    let response = mark(&app, &auth[0], &csrf(), 70).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()["location"], "/");
    assert_eq!(stored_mark(&store, 1).await, Some(70));
    let html = home(&app, &auth[0], true).await;
    assert!(html.contains("New since you last marked them as seen"));
    assert!(html.contains("3 suggestions imported"));
    assert!(!html.contains("redeemed an invitation"));
    assert!(!html.contains("upload failed (timeout)"));
    // Never backwards: an older value changes nothing, and neither does a repeat.
    for seen in [50, 70] {
        assert_eq!(
            mark(&app, &auth[0], &csrf(), seen).await.status(),
            StatusCode::SEE_OTHER
        );
        assert_eq!(stored_mark(&store, 1).await, Some(70));
    }
    // Mark the rest; the section is empty with no button, pending items stay.
    assert_eq!(
        mark(&app, &auth[0], &csrf(), 80).await.status(),
        StatusCode::SEE_OTHER
    );
    let html = home(&app, &auth[0], true).await;
    assert!(!html.contains("Mark as seen"));
    assert!(html.contains("No suggestions were added."));
    assert!(html.contains("2 of 3 imported pages not complete"));
    // A newer item appears alone, and the player-side marks are separate rows.
    store
        .run(|db| {
            db.execute("INSERT INTO account_events(account_id,epoch,occurred_at,actor,action) VALUES(4,1,90,'credential_redemption','enroll')", [])?;
            Ok(())
        })
        .await
        .unwrap();
    let html = home(&app, &auth[0], true).await;
    assert!(html.contains("<input type=\"hidden\" name=\"seen\" value=\"90\">"));
    assert!(!html.contains("3 suggestions imported"));
    assert_eq!(stored_mark(&store, 2).await, None);
}

#[tokio::test]
async fn owner_home_shows_ten_rows_per_section_with_a_link_to_the_list() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    for id in 1..=12 {
        seed_source(&store, id, &format!("Book {id:02}"), &[false]).await;
    }
    store
        .run(|db| {
            for index in 0..12 {
                db.execute("INSERT INTO account_events(account_id,epoch,occurred_at,actor,action) VALUES(3,1,?1,'credential_redemption','enroll')", [10 + index])?;
            }
            Ok(())
        })
        .await
        .unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    let html = home(&app, &auth[0], true).await;
    assert_eq!(
        html.matches("1 of 1 imported pages not complete").count(),
        10
    );
    assert!(html.contains("Book 10") && !html.contains("Book 11"));
    assert_eq!(html.matches("redeemed an invitation").count(), 10);
    assert!(html.matches("Only the first 10 are shown;").count() >= 2);
    // The ten newest events (times 12..=21), and the mark moves to the newest.
    assert!(html.contains("name=\"seen\" value=\"21\""));
    assert_eq!(
        mark(&app, &auth[0], &csrf(), 21).await.status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(stored_mark(&store, 1).await, Some(21));
}

#[tokio::test]
async fn owner_home_runs_no_campaign_query_and_a_player_home_no_owner_query() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed_owner(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let tables = tables_read(&store, || async {
        home(&app, &auth[0], true).await;
    })
    .await;
    for table in OWNER_TABLES {
        assert!(
            tables.iter().any(|read| read == table),
            "{table}: {tables:?}"
        );
    }
    for table in CAMPAIGN_TABLES {
        assert!(
            !tables.iter().any(|read| read == table),
            "{table}: {tables:?}"
        );
    }
    let tables = tables_read(&store, || async {
        home(&app, &auth[1], false).await;
    })
    .await;
    for table in OWNER_TABLES {
        assert!(
            !tables.iter().any(|read| read == table),
            "{table}: {tables:?}"
        );
    }
    assert!(tables.iter().any(|read| read == "campaign_members"));
    // The owner's mark cannot be posted through the player path, nor the reverse.
    assert_eq!(
        mark(&app, &auth[1], &csrf(), 80).await.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "a player has nothing new to mark"
    );
}

async fn seed_rulesets(store: &Store) {
    published_fixture(store).await;
    store
        .run(|db| {
            db.execute("INSERT INTO sources(id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,raw_json,prepared_json,total_pages) VALUES(900,'owner-only-fixture','fixture','OWNER-ONLY-TITLE','synthetic-game','Owner-only edition','en','fixture',x'',x'',1)", [])?;
            db.execute("INSERT INTO source_rulesets VALUES(900,'synthetic-game','first')", [])?;
            Ok(())
        })
        .await
        .unwrap();
}

fn at(clock: &Arc<AtomicI64>, time: i64) {
    clock.store(time, Ordering::SeqCst);
}

async fn revision(store: &Store, auth: &RequestAuth, campaign: i64, account: i64) -> i64 {
    let detail = store
        .campaign_detail(auth.clone(), campaign, 0)
        .await
        .unwrap();
    detail
        .members
        .iter()
        .find(|member| member.account_id == account)
        .map(|member| member.revision)
        .unwrap()
}

/// Accounts 2 (Master), 3 and 4 (players); account 5 is outside the campaign.
async fn campaign_with_players(store: &Store, auth: &[RequestAuth], clock: &Arc<AtomicI64>) -> i64 {
    seed_rulesets(store).await;
    at(clock, 100);
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Alpha <campaign>".into(),
            "synthetic-game".into(),
            "first".into(),
        )
        .await
        .unwrap();
    at(clock, 110);
    store.request_join(auth[2].clone(), campaign).await.unwrap();
    at(clock, 120);
    store
        .decide_join(auth[1].clone(), campaign, 3, 1, true)
        .await
        .unwrap();
    at(clock, 130);
    store.request_join(auth[3].clone(), campaign).await.unwrap();
    at(clock, 140);
    store
        .decide_join(auth[1].clone(), campaign, 4, 1, true)
        .await
        .unwrap();
    campaign
}

/// Marks by a player and a Master (the fixture clock stands at 150), and the Owner's refusals.
async fn player_marks(app: &axum::Router, store: &Store, auth: &[RequestAuth]) {
    // The player marks their changes seen; only later ones show.
    let auth_three = &auth[2];
    assert_eq!(
        mark_at(app, auth_three, &csrf(), 120, 150).await.status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(stored_mark(store, 3).await, Some(120));
    assert_eq!(stored_mark(store, 2).await, None, "marks are per account");
    // Two tabs: a stale, lower value after a higher one changes nothing.
    assert_eq!(
        mark_at(app, auth_three, &csrf(), 110, 150).await.status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(stored_mark(store, 3).await, Some(120));
    // The Owner cannot mark a time that exists only in campaign data (150 is a player-side
    // request), and the player's POST reads no Owner table.
    assert_eq!(
        mark_at(app, &auth[0], &csrf(), 120, 150).await.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(stored_mark(store, 1).await, None);
    let tables = tables_read(store, || async {
        mark_at(app, &auth[1], &csrf(), 150, 150).await;
    })
    .await;
    for table in OWNER_TABLES {
        assert!(
            !tables.iter().any(|read| read == table),
            "{table}: {tables:?}"
        );
    }
    assert!(tables.iter().any(|read| read == "campaign_events"));
    assert_eq!(
        stored_mark(store, 2).await,
        Some(149),
        "the mark stops before the second the page was rendered"
    );
    let html = home(app, &auth[2], false).await;
    assert!(html.contains("Nothing changed in your campaigns."));
    assert!(!html.contains("Mark as seen"));
    // A mark beyond what the player is shown is refused; a stale CSRF token too.
    assert_eq!(
        mark_at(app, auth_three, &csrf(), 121, 150).await.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        mark_at(app, auth_three, &"b".repeat(64), 120, 150)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn player_and_master_home_sections() {
    let _lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, clock) = fixture().await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    // Empty states first (nobody has a campaign).
    let html = home(&app, &auth[4], false).await;
    assert!(html.contains("You do not run a campaign."));
    assert!(html.contains("You do not play in a campaign yet."));
    assert!(html.contains("No join request of yours is waiting."));
    assert!(!html.contains("Join requests to answer"), "not a Master");
    assert!(html.contains("Nothing changed in your campaigns."));
    assert!(!html.contains("Mark as seen"));

    let campaign = campaign_with_players(&store, &auth, &clock).await;
    let mut three = sheet(campaign, 3);
    three.name = "Sheet of three".into();
    insert_sheet(&store, auth[2].clone(), three).await;
    let mut four = sheet(campaign, 4);
    four.name = "Sheet of <four>".into();
    four.sheet_id = 2;
    let four = insert_sheet(&store, auth[3].clone(), four).await;
    // A pending request of account 5 for the Master to answer.
    at(&clock, 150);
    store.request_join(auth[4].clone(), campaign).await.unwrap();
    seed_owner(&store).await;

    // The Master: the campaign they run, requests to answer, the changes by others (the
    // requests of 3 and 4 at 110 and 130 and of 5 at 150), never their own actions.
    let html = home(&app, &auth[1], false).await;
    assert!(html.contains("Alpha &#60;campaign&#62;") || html.contains("Alpha &lt;campaign&gt;"));
    assert!(html.contains(&format!("href=\"/campaigns/{campaign}\"")));
    assert!(html.contains("player5 asks to join"));
    assert!(html.contains("player3 asked to join"));
    assert!(html.contains("player4 asked to join"));
    assert!(
        !html.contains("player2 accepted"),
        "own actions are not news"
    );
    assert!(html.contains("name=\"seen\" value=\"150\""));
    assert!(!html.contains("You do not run a campaign."));

    // A player: the campaign they play in with their own sheet only, and the Master's
    // decision about them.
    let html = home(&app, &auth[2], false).await;
    assert!(html.contains("Campaigns I play in"));
    assert!(html.contains("Sheet #1"));
    assert!(html.contains("href=\"/private-sheets/1\""));
    assert!(!html.contains("Sheet #2"));
    assert!(!html.contains(&format!("href=\"/private-sheets/{}\"", four.sheet_id)));
    assert!(html.contains("player2 accepted you"));
    assert!(
        !html.contains("player4 asked to join"),
        "players do not see Master events"
    );
    assert!(!html.contains("Join requests to answer"));
    assert!(html.contains("name=\"seen\" value=\"120\""));
    // Nothing of the Owner's data reaches a player.
    for forbidden in [
        "OWNER-ONLY-TITLE",
        "Open &#60;book&#62;",
        "Invited",
        "model-q4",
        "Finished book",
    ] {
        assert!(!html.contains(forbidden), "{forbidden}");
    }
    // The pending request of 5 (their own view).
    let html = home(&app, &auth[4], false).await;
    assert!(html.contains("waiting for a Master to answer"));
    assert!(!html.contains("No join request of yours is waiting."));

    player_marks(&app, &store, &auth).await;

    // Promoted at 160: they now see the Master's campaign events after their mark (the
    // requests of 4 at 130 and of 5 at 150); demoted at 170: only events about themselves.
    at(&clock, 160);
    let revision_3 = revision(&store, &auth[1], campaign, 3).await;
    store
        .change_role(auth[1].clone(), campaign, 3, revision_3, true)
        .await
        .unwrap();
    let html = home(&app, &auth[2], false).await;
    assert!(html.contains("Campaigns I run"));
    assert!(html.contains("player4 asked to join"));
    assert!(html.contains("player5 asks to join"));
    assert!(html.contains("player2 made you a Master"));
    at(&clock, 170);
    let revision_3 = revision(&store, &auth[1], campaign, 3).await;
    store
        .change_role(auth[1].clone(), campaign, 3, revision_3, false)
        .await
        .unwrap();
    let html = home(&app, &auth[2], false).await;
    assert!(
        !html.contains("player4 asked to join"),
        "no longer a Master"
    );
    assert!(html.contains("player2 made you a player"));
}

#[tokio::test]
async fn player_home_shows_ten_campaigns_with_a_link_to_the_list() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed_rulesets(&store).await;
    for index in 0..12 {
        store
            .create_campaign(
                auth[1].clone(),
                format!("Campaign {index:02}"),
                "synthetic-game".into(),
                "first".into(),
            )
            .await
            .unwrap();
    }
    let app = crate::auth::app(store.clone()).await.unwrap();
    let html = home(&app, &auth[1], false).await;
    assert!(html.contains("Campaign 09") && !html.contains("Campaign 10"));
    assert!(html.contains("Only the first 10 are shown;"));
    assert!(html.contains("href=\"/campaigns\">All campaigns</a>"));
}

#[tokio::test]
async fn signed_out_home_goes_to_the_login_and_the_mark_needs_a_session() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, _auth, _clock) = fixture().await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/")
                .header("host", "127.0.0.1:3000")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()["location"], "/auth/login");
    let response = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/home/seen")
                .header("host", "127.0.0.1:3000")
                .header("origin", "http://127.0.0.1:3000")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(axum::body::Body::from(format!(
                    "csrf={}&seen=1&shown_at=2",
                    csrf()
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        response.status() == StatusCode::FORBIDDEN || response.status() == StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn the_mark_table_refuses_backwards_and_future_values_and_deletion() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, _auth, _clock) = fixture().await;
    let result = store
        .run(|db| {
            db.execute("INSERT INTO account_home_marks VALUES(2,50)", [])?;
            let mut refused = Vec::new();
            for sql in [
                "UPDATE account_home_marks SET seen_at=49 WHERE account_id=2",
                "UPDATE account_home_marks SET seen_at=101 WHERE account_id=2",
                "UPDATE account_home_marks SET account_id=3 WHERE account_id=2",
                "DELETE FROM account_home_marks WHERE account_id=2",
                "INSERT INTO account_home_marks VALUES(3,101)",
                "INSERT INTO account_home_marks VALUES(3,0)",
                "INSERT INTO account_home_marks VALUES(99,5)",
            ] {
                refused.push(db.execute(sql, []).is_err());
            }
            db.execute(
                "UPDATE account_home_marks SET seen_at=50 WHERE account_id=2",
                [],
            )?;
            db.execute(
                "UPDATE account_home_marks SET seen_at=60 WHERE account_id=2",
                [],
            )?;
            Ok(refused)
        })
        .await
        .unwrap();
    assert_eq!(result, vec![true; 7]);
}

#[tokio::test]
async fn schema_30_upgrades_to_the_home_marks_with_data_intact() {
    let _lock = crate::auth::test_resource_lock().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("upgrade.sqlite3");
    {
        let store = Store::open(&path).unwrap();
        store
            .run(|db| {
                db.execute("INSERT INTO accounts(id,login,display_name,state,epoch) VALUES(7,'later','Later','pending',1)", [])?;
                db.execute("INSERT INTO account_events(account_id,epoch,occurred_at,actor,action) VALUES(7,1,70,'local_operator','invite')", [])?;
                db.execute_batch(crate::auth::DROP_HOME_SCHEMA)?;
                db.execute_batch("PRAGMA user_version=30;")?;
                Ok(())
            })
            .await
            .unwrap();
    }
    let store = Store::open(&path).unwrap();
    store
        .run(|db| {
            assert_eq!(
                db.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?,
                crate::store::SCHEMA_VERSION
            );
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM account_events WHERE account_id=7",
                    [],
                    |row| row.get::<_, i64>(0)
                )?,
                1
            );
            for index in [
                "campaign_events_target",
                "campaign_events_campaign",
                "campaign_requests_account",
                "account_events_action",
                "account_credentials_open",
                "source_pages_complete",
            ] {
                assert_eq!(
                    db.query_row(
                        "SELECT count(*) FROM sqlite_master WHERE type='index' AND name=?1",
                        [index],
                        |row| row.get::<_, i64>(0)
                    )?,
                    1,
                    "{index}"
                );
            }
            assert_eq!(
                db.query_row("SELECT count(*) FROM account_home_marks", [], |row| row
                    .get::<_, i64>(0))?,
                0
            );
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn an_item_arriving_in_the_second_a_page_was_rendered_is_not_marked_seen() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let event = |account: i64| {
        move |db: &mut rusqlite::Connection| {
            db.execute("INSERT INTO account_events(account_id,epoch,occurred_at,actor,action) VALUES(?1,1,100,'credential_redemption','enroll')", [account])?;
            Ok(())
        }
    };
    store.run(event(3)).await.unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    let html = home(&app, &auth[0], true).await;
    assert!(html.contains("name=\"seen\" value=\"100\""));
    assert!(html.contains("name=\"shown_at\" value=\"100\""));
    assert!(html.contains("player3") && !html.contains("player4"));
    // Another item lands in the same second after the render; then the page's mark is posted.
    store.run(event(4)).await.unwrap();
    assert_eq!(
        mark(&app, &auth[0], &csrf(), 100).await.status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(stored_mark(&store, 1).await, Some(99));
    let html = home(&app, &auth[0], true).await;
    assert!(html.contains("player4"), "the late item is still new");
}

#[tokio::test]
async fn the_render_second_must_be_valid_and_the_owner_post_reads_no_campaign_table() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed_owner(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    for shown_at in [0, -1, 101] {
        assert_eq!(
            mark_at(&app, &auth[0], &csrf(), 80, shown_at)
                .await
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{shown_at}"
        );
    }
    assert_eq!(stored_mark(&store, 1).await, None);
    let tables = tables_read(&store, || async {
        assert_eq!(
            mark(&app, &auth[0], &csrf(), 80).await.status(),
            StatusCode::SEE_OTHER
        );
    })
    .await;
    assert!(tables.iter().any(|read| read == "account_events"));
    for table in CAMPAIGN_TABLES {
        assert!(
            !tables.iter().any(|read| read == table),
            "{table}: {tables:?}"
        );
    }
    assert_eq!(stored_mark(&store, 1).await, Some(80));
}

/// The names `migrations/031_home_marks.sql` creates equal the drop lists the rewinding tests
/// use (`auth::DROP_HOME_SCHEMA` and `tests/support/schema_fixture.rs`).
#[test]
fn drop_lists_match_migration_031() {
    let migration = include_str!("../../migrations/031_home_marks.sql");
    let mut created = Vec::new();
    for line in migration.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        if let ["CREATE", kind @ ("TABLE" | "INDEX"), name, ..] = words.as_slice() {
            created.push(format!("DROP {kind} IF EXISTS {name}"));
        }
    }
    created.sort();
    let mut dropped: Vec<String> = crate::auth::DROP_HOME_SCHEMA
        .split(';')
        .map(str::trim)
        .filter(|statement| !statement.is_empty())
        .map(str::to_owned)
        .collect();
    dropped.sort();
    assert_eq!(created, dropped);
    let fixture = include_str!("../../tests/support/schema_fixture.rs").replace("\\\n", "");
    for statement in &created {
        assert!(
            fixture.contains(&format!("{statement};")),
            "{statement} missing from tests/support/schema_fixture.rs"
        );
    }
}
