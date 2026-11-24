//! Master view, slice (a) (CAMP-14, ROAD-8): NPC classification at creation, the Master
//! workspace, the click-only seen acknowledgement and the own-save rules of owner decision 6.
use super::cell_tests::{new_row, row_action};
use super::formula_tests::{Setup, setup};
use super::notes_tests::{assist, create_path, form, history_len, pair, row, saved};
use super::{admitted, router_request, sheet};
use axum::http::StatusCode;
use serde_json::{Value, json};

type Pairs = Vec<(String, String)>;

/// The assist definition with a quick-add line on Equipment (field 4).
fn definition() -> Value {
    let mut definition = assist();
    definition["fields"][4]["items"]["quick_add"] = json!({"name": "name", "quantity": "quantity"});
    definition
}

/// Campaign 1 of the assist fixture: account 2 (`auth[1]`) is its creator and Master A,
/// account 3 (`auth[2]`) a player, account 4 (`auth[3]`) a second Master B, account 5
/// (`auth[4]`) an outsider and account 1 (`auth[0]`) the Owner.
async fn world() -> Setup {
    let setup = setup(&definition()).await;
    for (who, target) in [(2, 3), (3, 4)] {
        setup
            .store
            .request_join(setup.auth[who].clone(), setup.campaign)
            .await
            .unwrap();
        setup
            .store
            .decide_join(setup.auth[1].clone(), setup.campaign, target, 1, true)
            .await
            .unwrap();
    }
    setup
        .store
        .change_role(setup.auth[1].clone(), setup.campaign, 4, 1, true)
        .await
        .unwrap();
    setup
}

async fn send(setup: &Setup, who: usize, path: &str, form: Option<Pairs>) -> (StatusCode, String) {
    let response = router_request(&setup.app, &setup.auth[who], path, form).await;
    let status = response.status();
    (status, super::formula_tests::body(response).await)
}

async fn get(setup: &Setup, who: usize, path: &str) -> String {
    let (status, html) = send(setup, who, path, None).await;
    assert_eq!(status, StatusCode::OK, "{path}: {html}");
    html
}

/// The `Location` of a redirect answer.
async fn location(setup: &Setup, who: usize, path: &str, form: Pairs) -> (StatusCode, String) {
    let response = router_request(&setup.app, &setup.auth[who], path, Some(form)).await;
    let location = response
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    (response.status(), location)
}

fn seen_form(revision: &str) -> Pairs {
    vec![pair("csrf", &"a".repeat(64)), pair("revision", revision)]
}

/// `who` creates a sheet (classified when `npc`) through the router; returns the status.
async fn create_as(setup: &Setup, who: usize, npc: bool) -> StatusCode {
    let path = format!("{}{}", create_path(setup), if npc { "&npc=1" } else { "" });
    send(setup, who, &path, Some(form(0, &[], &[row("", "1")])))
        .await
        .0
}

/// A raw single value (`None` when the row is absent).
async fn scalar(setup: &Setup, sql: &'static str, params: Vec<i64>) -> Option<i64> {
    setup
        .store
        .run(move |db| {
            use rusqlite::OptionalExtension;
            Ok(db
                .query_row(sql, rusqlite::params_from_iter(params), |r| r.get(0))
                .optional()?)
        })
        .await
        .unwrap()
}

async fn seen_of(setup: &Setup, master: i64, sheet: i64) -> Option<i64> {
    scalar(
        setup,
        "SELECT seen_revision FROM master_sheet_seen WHERE master_id=?1 AND sheet_id=?2",
        vec![master, sheet],
    )
    .await
}

async fn npc_of(setup: &Setup, sheet: i64) -> i64 {
    scalar(
        setup,
        "SELECT is_npc FROM private_sheets WHERE id=?1",
        vec![sheet],
    )
    .await
    .unwrap()
}

async fn sheet_count(setup: &Setup) -> i64 {
    scalar(setup, "SELECT count(*) FROM private_sheets", vec![])
        .await
        .unwrap()
}

/// Runs `sql` in a transaction that is rolled back, as the request of `who` (so the
/// request-auth triggers see that account); the error text on failure.
async fn sql_as(setup: &Setup, who: usize, sql: &'static str) -> Result<(), String> {
    setup
        .store
        .run_campaign(setup.auth[who].clone(), move |db| {
            let tx = db.transaction()?;
            Ok(tx.execute_batch(sql).map_err(|error| error.to_string()))
        })
        .await
        .unwrap()
}

async fn lock() -> (
    tokio::sync::OwnedMutexGuard<()>,
    tokio::sync::MutexGuard<'static, ()>,
) {
    (
        crate::auth::test_resource_lock().await,
        crate::definitions::TEST_RESOURCE_LOCK.lock().await,
    )
}

/// Every refusal reached through the router is the uniform 403 and names no sheet.
fn nothing_leaks(body: &str) {
    for needle in ["Guard", "Assisted sheet", "NPC", "Player sheets"] {
        assert!(!body.contains(needle), "{needle} in {body}");
    }
}

/// The action of the (only) create form of a page.
fn create_action(html: &str) -> String {
    let page = scraper::Html::parse_document(html);
    let selector = scraper::Selector::parse("form[action*=\"/sheets/new\"]").unwrap();
    page.select(&selector)
        .next()
        .and_then(|form| form.value().attr("action").map(str::to_owned))
        .unwrap_or_else(|| panic!("no create form in {html}"))
}

fn articles(html: &str, section: &str) -> usize {
    let page = scraper::Html::parse_document(html);
    let selector =
        scraper::Selector::parse(&format!("section[aria-labelledby=\"{section}\"] article"))
            .unwrap();
    page.select(&selector).count()
}

fn has_link(html: &str, text: &str) -> bool {
    let page = scraper::Html::parse_document(html);
    let selector = scraper::Selector::parse("a").unwrap();
    page.select(&selector)
        .any(|link| link.text().collect::<String>().trim() == text)
}

fn section_text(html: &str, section: &str) -> String {
    let page = scraper::Html::parse_document(html);
    let selector =
        scraper::Selector::parse(&format!("section[aria-labelledby=\"{section}\"]")).unwrap();
    page.select(&selector)
        .next()
        .map(|element| element.text().collect::<String>())
        .unwrap_or_default()
}

/// Sheet 1 (revision 1) created by Master A, then edited by A (HP −15, revision 2).
async fn two_revisions(setup: &Setup) {
    assert_eq!(create_as(setup, 1, false).await, StatusCode::SEE_OTHER);
    let (status, html) = send(
        setup,
        1,
        "/private-sheets/1",
        Some(form(1, &[("value-hp", "-15")], &[row("r1-1", "1")])),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
}

// ---------------------------------------------------------------------------------------
// Classified creation

#[tokio::test]
async fn a_master_creates_an_npc_sheet_and_everyone_else_cannot() {
    let _locks = lock().await;
    let setup = world().await;
    // Default: a player sheet. Master A's classified creation: an NPC sheet.
    assert_eq!(create_as(&setup, 1, false).await, StatusCode::SEE_OTHER);
    assert_eq!(create_as(&setup, 1, true).await, StatusCode::SEE_OTHER);
    assert_eq!((npc_of(&setup, 1).await, npc_of(&setup, 2).await), (0, 1));
    // The classification never enters the snapshot or its hash.
    let stored = setup
        .store
        .run(|db| {
            Ok(db.query_row(
                "SELECT group_concat(snapshot) FROM private_sheet_history",
                [],
                |r| r.get::<_, String>(0),
            )?)
        })
        .await
        .unwrap();
    assert!(!stored.to_lowercase().contains("npc"), "{stored}");
    // The form action carries the flag on the create URL, never as a form key.
    let page = get(&setup, 1, &format!("{}&npc=1", create_path(&setup))).await;
    assert!(create_action(&page).ends_with("&npc=1"), "{page}");
    assert!(!page.contains("name=\"npc\""), "{page}");
    let plain = get(&setup, 1, &create_path(&setup)).await;
    assert!(!plain.to_lowercase().contains("npc"), "{plain}");
    assert!(!create_action(&plain).contains("npc"));
    // Forged classification by a player, an outsider, the Owner: 403, nothing created, on GET
    // and POST; the player's ordinary creation is unaffected.
    let before = sheet_count(&setup).await;
    for who in [0, 2, 4] {
        let path = format!("{}&npc=1", create_path(&setup));
        assert_eq!(
            send(&setup, who, &path, None).await.0,
            StatusCode::FORBIDDEN,
            "{who}"
        );
        assert_eq!(
            create_as(&setup, who, true).await,
            StatusCode::FORBIDDEN,
            "{who}"
        );
    }
    assert_eq!(sheet_count(&setup).await, before);
    // Anything but npc=0 or npc=1 is refused.
    for bad in ["npc=2", "npc=-1", "npc=yes", "npc=1&npc=1", "npc=1&x=1"] {
        let path = format!("{}&{bad}", create_path(&setup));
        let (status, _) = send(&setup, 1, &path, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    assert_eq!(create_as(&setup, 2, false).await, StatusCode::SEE_OTHER);
    assert_eq!(npc_of(&setup, 3).await, 0);
    // A former Master (demoted) cannot classify either.
    setup
        .store
        .change_role(setup.auth[1].clone(), setup.campaign, 4, 2, false)
        .await
        .unwrap();
    assert_eq!(create_as(&setup, 3, true).await, StatusCode::FORBIDDEN);
    assert_eq!(create_as(&setup, 3, false).await, StatusCode::SEE_OTHER);
    assert_eq!(npc_of(&setup, 4).await, 0, "a player sheet, never an NPC");
}

#[tokio::test]
async fn the_choice_survives_a_refused_draft_only_for_a_still_authorized_master() {
    let _locks = lock().await;
    let setup = world().await;
    let path = format!("{}&npc=1", create_path(&setup));
    // A stale base (revision 5 on a create page) is refused with the draft re-rendered.
    let invalid = form(5, &[("name", "Draftmarker")], &[row("", "1")]);
    let (status, html) = send(&setup, 3, &path, Some(invalid.clone())).await;
    assert_eq!(status, StatusCode::CONFLICT, "{html}");
    assert!(html.contains("Draftmarker"), "{html}");
    assert!(create_action(&html).ends_with("&npc=1"), "{html}");
    // The draft is refused for a Master who lost the role meanwhile: 403, no echo.
    setup
        .store
        .change_role(setup.auth[1].clone(), setup.campaign, 4, 2, false)
        .await
        .unwrap();
    let (status, html) = send(&setup, 3, &path, Some(invalid.clone())).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(!html.contains("Draftmarker"), "{html}");
    let valid = form(0, &[], &[row("", "1")]);
    assert_eq!(
        send(&setup, 3, &path, Some(valid)).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(sheet_count(&setup).await, 0);
}

#[tokio::test]
async fn npc_creation_triggers_refuse_forgery_and_the_flag_is_immutable() {
    let _locks = lock().await;
    let setup = world().await;
    assert_eq!(create_as(&setup, 1, true).await, StatusCode::SEE_OTHER);
    // A copy of sheet 1 for custodian N, classified or not, inserted as the request of `who`.
    macro_rules! copy {
        ($custodian:literal, $npc:literal) => {
            concat!(
                "INSERT INTO private_sheets(id,campaign_id,custodian_id,game_id,edition_id,kind,pin,revision,state,snapshot,hash,is_npc) SELECT 9,campaign_id,",
                $custodian,
                ",game_id,edition_id,kind,pin,revision,state,snapshot,hash,",
                $npc,
                " FROM private_sheets WHERE id=1"
            )
        };
    }
    // The existing creation restrictions still hold on their own (the control cases).
    assert_eq!(sql_as(&setup, 2, copy!("3", "0")).await, Ok(()));
    assert_eq!(sql_as(&setup, 1, copy!("2", "1")).await, Ok(()));
    for (who, sql) in [
        (2, copy!("3", "1")),
        (4, copy!("5", "1")),
        (0, copy!("1", "1")),
        // A Master cannot create an NPC held by somebody else (nor by another Master).
        (1, copy!("3", "1")),
        (1, copy!("4", "1")),
    ] {
        let error = sql_as(&setup, who, sql).await.unwrap_err();
        assert!(
            error.contains("npc creation requires a master")
                || error.contains("private auth required"),
            "{who} {sql}: {error}"
        );
    }
    // A demoted Master (account 4) fails on the NPC rule itself, past every other guard.
    setup
        .store
        .change_role(setup.auth[1].clone(), setup.campaign, 4, 2, false)
        .await
        .unwrap();
    let error = sql_as(&setup, 3, copy!("4", "1")).await.unwrap_err();
    assert!(error.contains("npc creation requires a master"), "{error}");
    assert_eq!(sql_as(&setup, 3, copy!("4", "0")).await, Ok(()));
    // The flag is permanent in both directions, for any actor.
    let error = sql_as(&setup, 1, "UPDATE private_sheets SET is_npc=0 WHERE id=1")
        .await
        .unwrap_err();
    assert!(error.contains("npc classification is permanent"), "{error}");
    assert_eq!(create_as(&setup, 2, false).await, StatusCode::SEE_OTHER);
    let error = sql_as(&setup, 1, "UPDATE private_sheets SET is_npc=1 WHERE id=2")
        .await
        .unwrap_err();
    assert!(error.contains("npc classification is permanent"), "{error}");
    let error = sql_as(&setup, 1, "UPDATE private_sheets SET is_npc=7 WHERE id=1")
        .await
        .unwrap_err();
    assert!(
        error.contains("CHECK") || error.contains("permanent"),
        "{error}"
    );
    // An ordinary save keeps it.
    let (status, html) = send(
        &setup,
        1,
        "/private-sheets/1",
        Some(form(1, &[("value-hp", "50")], &[row("r1-1", "1")])),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
    assert_eq!(npc_of(&setup, 1).await, 1);
}

// ---------------------------------------------------------------------------------------
// The workspace

#[tokio::test]
async fn only_a_current_master_reaches_the_workspace() {
    let _locks = lock().await;
    let setup = world().await;
    assert_eq!(create_as(&setup, 1, false).await, StatusCode::SEE_OTHER);
    assert_eq!(create_as(&setup, 1, true).await, StatusCode::SEE_OTHER);
    let path = format!("/campaigns/{}/master", setup.campaign);
    for who in [1, 3] {
        let html = get(&setup, who, &path).await;
        assert!(
            html.contains("Player sheets") && html.contains("NPC sheets"),
            "{html}"
        );
    }
    // Owner, player, outsider: the same 403 with no sheet metadata, as for a campaign that
    // does not exist.
    for who in [0, 2, 4] {
        let (status, body) = send(&setup, who, &path, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{who}");
        nothing_leaks(&body);
    }
    let (status, _) = send(&setup, 1, "/campaigns/99/master", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // A former Master is refused, and regains the page on re-promotion.
    setup
        .store
        .change_role(setup.auth[1].clone(), setup.campaign, 4, 2, false)
        .await
        .unwrap();
    let (status, body) = send(&setup, 3, &path, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    nothing_leaks(&body);
    // Master of another campaign only: still refused.
    let other = setup
        .store
        .create_campaign(
            setup.auth[4].clone(),
            "Other".into(),
            "synthetic-game".into(),
            "first".into(),
        )
        .await
        .unwrap();
    assert_eq!(send(&setup, 4, &path, None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        send(&setup, 4, &format!("/campaigns/{other}/master"), None)
            .await
            .0,
        StatusCode::OK
    );
    // Cursors: nonnegative integers and no other parameter.
    for bad in [
        "players_after=-1",
        "npcs_after=-5",
        "players_after=x",
        "other=1",
    ] {
        let (status, _) = send(&setup, 1, &format!("{path}?{bad}"), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    // The campaign page links to it, and offers NPC creation, only to current Masters.
    let campaign = get(&setup, 1, &format!("/campaigns/{}", setup.campaign)).await;
    assert!(
        campaign.contains(&path) && campaign.contains("npc=1"),
        "{campaign}"
    );
    for who in [2, 3] {
        let page = get(&setup, who, &format!("/campaigns/{}", setup.campaign)).await;
        assert!(!page.contains("/campaigns/1/master"), "{who}: {page}");
        assert!(!page.to_lowercase().contains("npc"), "{who}: {page}");
    }
}

#[tokio::test]
async fn the_workspace_sorts_sheets_into_sections_and_marks_each_master_apart() {
    let _locks = lock().await;
    let setup = world().await;
    // Player 3's sheet, Master A's own player sheet, Master B's NPC.
    assert_eq!(create_as(&setup, 2, false).await, StatusCode::SEE_OTHER);
    assert_eq!(create_as(&setup, 1, false).await, StatusCode::SEE_OTHER);
    assert_eq!(create_as(&setup, 3, true).await, StatusCode::SEE_OTHER);
    let path = format!("/campaigns/{}/master", setup.campaign);
    let a = get(&setup, 1, &path).await;
    let players = section_text(&a, "players-heading");
    let npcs = section_text(&a, "npcs-heading");
    assert_eq!(articles(&a, "players-heading"), 2, "{a}");
    assert_eq!(articles(&a, "npcs-heading"), 1, "{a}");
    assert!(
        players.contains("Not opened") && players.contains("Opened"),
        "{players}"
    );
    assert!(
        npcs.contains("Not opened") && npcs.contains("custodian"),
        "{npcs}"
    );
    // B created sheet 3 (seen 1); A's own sheet 2 is "Opened"; each sees only their own marks.
    assert_eq!(seen_of(&setup, 2, 2).await, Some(1));
    assert_eq!(seen_of(&setup, 4, 3).await, Some(1));
    let b = get(&setup, 3, &path).await;
    assert!(section_text(&b, "npcs-heading").contains("Opened"), "{b}");
    assert!(
        section_text(&b, "players-heading").contains("Not opened"),
        "{b}"
    );
    assert_eq!(
        seen_of(&setup, 2, 3).await,
        None,
        "no mark for a sheet A never opened"
    );
    // A change by B on A's sheet makes it "Changed" for A.
    let (status, html) = send(
        &setup,
        3,
        "/private-sheets/2",
        Some(form(1, &[("name", "Renamed")], &[row("r1-1", "1")])),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
    let a = get(&setup, 1, &path).await;
    assert!(
        section_text(&a, "players-heading").contains("Changed"),
        "{a}"
    );
}

/// Writes one sheet held by `custodian` (account `who + 1`), classified when `npc`.
async fn plant(setup: &Setup, who: usize, custodian: i64, npc: bool) {
    let auth = setup.auth[who].clone();
    let campaign = setup.campaign;
    setup
        .store
        .run_campaign(auth.clone(), move |db| {
            let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let mut snapshot = sheet(campaign, custodian);
            let pin = admitted(&tx, &mut snapshot);
            let max: i64 =
                tx.query_row("SELECT coalesce(max(id),0) FROM private_sheets", [], |r| {
                    r.get(0)
                })?;
            snapshot.sheet_id = max + 1;
            super::snapshots::write_classified(&tx, &auth, snapshot, "create", Some(&pin), npc)?;
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn both_sections_paginate_at_fifty_and_never_truncate() {
    let _locks = lock().await;
    let setup = world().await;
    // 50 player sheets (custodians 3 and 5, the cap is 32 each) and 50 NPC sheets (Masters 2
    // and 4), written through the runtime writer.
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
    for n in 0..50 {
        plant(
            &setup,
            if n < 25 { 2 } else { 4 },
            if n < 25 { 3 } else { 5 },
            false,
        )
        .await;
        plant(
            &setup,
            if n < 25 { 1 } else { 3 },
            if n < 25 { 2 } else { 4 },
            true,
        )
        .await;
    }
    let path = format!("/campaigns/{}/master", setup.campaign);
    let html = get(&setup, 1, &path).await;
    assert_eq!(articles(&html, "players-heading"), 50);
    assert_eq!(articles(&html, "npcs-heading"), 50);
    assert!(!has_link(&html, "Next player sheets") && !has_link(&html, "Next NPC sheets"));
    plant(&setup, 2, 3, false).await;
    plant(&setup, 1, 2, true).await;
    let html = get(&setup, 1, &path).await;
    assert_eq!(articles(&html, "players-heading"), 50);
    assert_eq!(articles(&html, "npcs-heading"), 50);
    assert!(has_link(&html, "Next player sheets") && has_link(&html, "Next NPC sheets"));
    // Each section pages on its own cursor, to the 51st row and back.
    let page = scraper::Html::parse_document(&html);
    let next = |text: &str| {
        let selector = scraper::Selector::parse("a").unwrap();
        page.select(&selector)
            .find(|link| link.text().collect::<String>() == text)
            .and_then(|link| link.value().attr("href").map(str::to_owned))
            .unwrap()
    };
    let players = get(&setup, 1, &next("Next player sheets")).await;
    assert_eq!(articles(&players, "players-heading"), 1);
    assert_eq!(
        articles(&players, "npcs-heading"),
        50,
        "the other cursor is kept"
    );
    assert!(has_link(&players, "First player sheets") && !has_link(&players, "Next player sheets"));
    let npcs = get(&setup, 1, &next("Next NPC sheets")).await;
    assert_eq!(articles(&npcs, "npcs-heading"), 1);
    assert_eq!(articles(&npcs, "players-heading"), 50);
    assert!(has_link(&npcs, "First NPC sheets"));
    // Past the end: an empty state, not an error.
    let empty = get(
        &setup,
        1,
        &format!("{path}?players_after=1000&npcs_after=1000"),
    )
    .await;
    assert!(empty.contains("No player sheets on this page"), "{empty}");
    assert!(empty.contains("No NPC sheets on this page"), "{empty}");
}

// ---------------------------------------------------------------------------------------
// Seen: triggers, the POST and the page

#[tokio::test]
async fn seen_triggers_refuse_everyone_but_the_current_master_for_themselves() {
    let _locks = lock().await;
    let setup = world().await;
    two_revisions(&setup).await;
    macro_rules! seen {
        ($master:literal, $revision:literal) => {
            concat!(
                "INSERT INTO master_sheet_seen(master_id,sheet_id,seen_revision) VALUES(",
                $master,
                ",1,",
                $revision,
                ")"
            )
        };
    }
    let other = setup
        .store
        .create_campaign(
            setup.auth[4].clone(),
            "Other".into(),
            "synthetic-game".into(),
            "first".into(),
        )
        .await
        .unwrap();
    assert_eq!(other, 2);
    // B writes their own mark; nobody else's, and no player, Owner or outsider writes any.
    assert_eq!(sql_as(&setup, 3, seen!("4", "2")).await, Ok(()));
    for (who, sql) in [
        (1, seen!("4", "2")),
        (2, seen!("3", "2")),
        (0, seen!("1", "2")),
        // A Master of another campaign, and the player 3.
        (4, seen!("5", "2")),
    ] {
        let error = sql_as(&setup, who, sql).await.unwrap_err();
        assert!(error.contains("seen auth required"), "{who}: {error}");
    }
    // A revision that does not exist, a future one and a missing sheet.
    for sql in [
        seen!("4", "3"),
        seen!("4", "99"),
        "INSERT INTO master_sheet_seen(master_id,sheet_id,seen_revision) VALUES(4,77,1)",
    ] {
        let error = sql_as(&setup, 3, sql).await.unwrap_err();
        assert!(
            error.contains("FOREIGN KEY") || error.contains("seen auth required"),
            "{sql}: {error}"
        );
    }
    // A mark goes up and never down, never moves, and is never deleted.
    assert_eq!(
        sql_as(&setup, 3, seen!("4", "1")).await,
        Ok(()),
        "(rolled back)"
    );
    setup
        .store
        .run_campaign(setup.auth[3].clone(), |db| {
            db.execute(
                "INSERT INTO master_sheet_seen(master_id,sheet_id,seen_revision) VALUES(4,1,2)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    for sql in [
        "UPDATE master_sheet_seen SET seen_revision=1 WHERE master_id=4",
        "UPDATE master_sheet_seen SET master_id=2 WHERE master_id=4",
        "DELETE FROM master_sheet_seen WHERE master_id=4",
    ] {
        let error = sql_as(&setup, 3, sql).await.unwrap_err();
        assert!(
            error.contains("seen auth required") || error.contains("permanent"),
            "{sql}: {error}"
        );
    }
    // A demoted Master can neither write nor lower a mark.
    setup
        .store
        .change_role(setup.auth[1].clone(), setup.campaign, 4, 2, false)
        .await
        .unwrap();
    let error = sql_as(
        &setup,
        3,
        "UPDATE master_sheet_seen SET seen_revision=2 WHERE master_id=4",
    )
    .await
    .unwrap_err();
    assert!(error.contains("seen auth required"), "{error}");
}

#[tokio::test]
async fn the_seen_post_records_the_rendered_revision_and_nothing_else() {
    let _locks = lock().await;
    let setup = world().await;
    two_revisions(&setup).await;
    let history = history_len(&setup, 1).await;
    // B opens the sheet: a plain read offers one click carrying the rendered revision.
    let before = get(&setup, 3, "/private-sheets/1").await;
    assert!(before.contains("id=\"seen-form\""), "{before}");
    assert!(before.contains("name=\"revision\" value=\"2\""), "{before}");
    assert!(before.contains("/campaigns/1/master"), "{before}");
    assert_eq!(
        seen_of(&setup, 4, 1).await,
        None,
        "a GET never acknowledges"
    );
    let list = get(&setup, 3, "/campaigns/1/master").await;
    assert!(
        section_text(&list, "players-heading").contains("Not opened"),
        "{list}"
    );
    assert_eq!(
        seen_of(&setup, 4, 1).await,
        None,
        "the list never acknowledges"
    );
    let (status, to) = location(&setup, 3, "/private-sheets/1/seen", seen_form("2")).await;
    assert_eq!(
        (status, to.as_str()),
        (StatusCode::SEE_OTHER, "/private-sheets/1")
    );
    assert_eq!(seen_of(&setup, 4, 1).await, Some(2));
    assert_eq!(
        history_len(&setup, 1).await,
        history,
        "no revision, no history"
    );
    let after = get(&setup, 3, "/private-sheets/1").await;
    assert!(
        after.contains("Opened (revision 2)") && !after.contains("seen-form"),
        "{after}"
    );
    // A's mark is independent and unchanged.
    assert_eq!(seen_of(&setup, 2, 1).await, Some(2));
    // A newer revision: "Changed"; a stale submission of 2 never lowers or advances.
    let (status, html) = send(
        &setup,
        1,
        "/private-sheets/1",
        Some(form(2, &[("value-hp", "40")], &[row("r1-1", "1")])),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
    let list = get(&setup, 3, "/campaigns/1/master").await;
    assert!(
        section_text(&list, "players-heading").contains("Changed"),
        "{list}"
    );
    assert_eq!(
        location(&setup, 3, "/private-sheets/1/seen", seen_form("2"))
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(seen_of(&setup, 4, 1).await, Some(2));
    assert_eq!(
        location(&setup, 3, "/private-sheets/1/seen", seen_form("1"))
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        seen_of(&setup, 4, 1).await,
        Some(2),
        "an older submission never lowers"
    );
    // Opening the current revision clears the badge.
    assert_eq!(
        location(&setup, 3, "/private-sheets/1/seen", seen_form("3"))
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    let list = get(&setup, 3, "/campaigns/1/master").await;
    assert!(
        !section_text(&list, "players-heading").contains("Changed"),
        "{list}"
    );
    assert_eq!(history_len(&setup, 1).await, history + 1);
}

#[tokio::test]
async fn the_seen_post_refuses_malformed_forged_and_cross_site_requests() {
    let _locks = lock().await;
    let setup = world().await;
    two_revisions(&setup).await;
    // A revision that does not exist (future) or is malformed: refused, visibly.
    for (revision, expected) in [
        ("3", StatusCode::UNPROCESSABLE_ENTITY),
        ("99", StatusCode::UNPROCESSABLE_ENTITY),
        ("0", StatusCode::BAD_REQUEST),
        ("-1", StatusCode::BAD_REQUEST),
        ("x", StatusCode::BAD_REQUEST),
        ("", StatusCode::BAD_REQUEST),
    ] {
        let (status, _) = send(
            &setup,
            3,
            "/private-sheets/1/seen",
            Some(seen_form(revision)),
        )
        .await;
        assert_eq!(status, expected, "{revision:?}");
    }
    assert_eq!(seen_of(&setup, 4, 1).await, None, "nothing was recorded");
    // Extra or missing keys are 400; a wrong token is 403.
    let mut extra = seen_form("3");
    extra.push(pair("note", "x"));
    assert_eq!(
        send(&setup, 3, "/private-sheets/1/seen", Some(extra))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(
            &setup,
            3,
            "/private-sheets/1/seen",
            Some(vec![pair("csrf", &"a".repeat(64))])
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let wrong = vec![pair("csrf", &"b".repeat(64)), pair("revision", "3")];
    assert_eq!(
        send(&setup, 3, "/private-sheets/1/seen", Some(wrong))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    // A cross-site POST is refused before the handler.
    {
        use tower::ServiceExt;
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/private-sheets/1/seen")
            .header("host", "127.0.0.1:3000")
            .header(crate::auth::TEST_DEADLINE_HEADER, "120")
            .header("cookie", setup.auth[3].fixture_cookie())
            .header("origin", "http://evil.example")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(axum::body::Body::from(
                serde_urlencoded::to_string(seen_form("3")).unwrap(),
            ))
            .unwrap();
        let response = setup.app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    assert_eq!(seen_of(&setup, 4, 1).await, None, "nothing was recorded");
    assert_eq!(
        location(&setup, 3, "/private-sheets/1/seen", seen_form("2"))
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(seen_of(&setup, 4, 1).await, Some(2));
}

#[tokio::test]
async fn seen_writes_by_anyone_but_a_current_master_of_the_campaign_are_refused() {
    let _locks = lock().await;
    let setup = world().await;
    two_revisions(&setup).await;
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
    // Player 3, the Owner, an outsider who is the Master of another campaign: 403, no row.
    for who in [0, 2, 4] {
        let (status, _) = send(&setup, who, "/private-sheets/1/seen", Some(seen_form("2"))).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{who}");
    }
    let (status, _) = send(&setup, 1, "/private-sheets/99/seen", Some(seen_form("1"))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(seen_of(&setup, 3, 1).await, None);
    assert_eq!(seen_of(&setup, 1, 1).await, None);
    assert_eq!(seen_of(&setup, 5, 1).await, None);
    // A demoted Master keeps the mark, cannot read the page or write it, and gets it back on
    // re-promotion.
    assert_eq!(
        location(&setup, 3, "/private-sheets/1/seen", seen_form("1"))
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    setup
        .store
        .change_role(setup.auth[1].clone(), setup.campaign, 4, 2, false)
        .await
        .unwrap();
    let (status, _) = send(&setup, 3, "/private-sheets/1/seen", Some(seen_form("2"))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        send(&setup, 3, "/campaigns/1/master", None).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(seen_of(&setup, 4, 1).await, Some(1));
    setup
        .store
        .change_role(setup.auth[1].clone(), setup.campaign, 4, 3, true)
        .await
        .unwrap();
    let list = get(&setup, 3, "/campaigns/1/master").await;
    assert!(
        section_text(&list, "players-heading").contains("Changed"),
        "{list}"
    );
}

#[tokio::test]
async fn only_a_plain_current_read_of_a_master_offers_the_acknowledgement() {
    let _locks = lock().await;
    let setup = world().await;
    two_revisions(&setup).await;
    // A player's own sheet: custodian, never offered; the player sees no workspace link.
    assert_eq!(create_as(&setup, 2, false).await, StatusCode::SEE_OTHER);
    let page = get(&setup, 2, "/private-sheets/2").await;
    assert!(
        !page.contains("seen-form") && !page.contains("Master workspace"),
        "{page}"
    );
    // Master B: history pages, the stale-draft 409 and the unchanged re-render offer nothing.
    for path in ["/private-sheets/1/history/1", "/private-sheets/1/history/2"] {
        let history = get(&setup, 3, path).await;
        assert!(!history.contains("seen-form"), "{path}");
    }
    let (status, stale) = send(
        &setup,
        3,
        "/private-sheets/1",
        Some(form(1, &[("value-hp", "5")], &[row("r1-1", "1")])),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{stale}");
    assert!(!stale.contains("seen-form"), "{stale}");
    let (status, same) = send(
        &setup,
        3,
        "/private-sheets/1",
        Some(form(2, &[("value-hp", "-15")], &[row("r1-1", "1")])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{same}");
    assert!(
        same.contains("No change") && !same.contains("seen-form"),
        "{same}"
    );
    assert_eq!(
        seen_of(&setup, 4, 1).await,
        None,
        "nothing above acknowledged"
    );
    // The return link keeps validated cursors only from the list's open; garbage falls back.
    let back = |html: &str| {
        let page = scraper::Html::parse_document(html);
        let selector = scraper::Selector::parse("a#master-link").unwrap();
        page.select(&selector)
            .next()
            .and_then(|link| link.value().attr("href").map(str::to_owned))
    };
    let from_list = get(&setup, 3, "/private-sheets/1?players_after=7&npcs_after=3").await;
    assert_eq!(
        back(&from_list).as_deref(),
        Some("/campaigns/1/master?players_after=7&npcs_after=3")
    );
    for bad in [
        "players_after=-1",
        "players_after=x&npcs_after=3",
        "players_after=1&players_after=2",
    ] {
        let page = get(&setup, 3, &format!("/private-sheets/1?{bad}")).await;
        assert_eq!(back(&page).as_deref(), Some("/campaigns/1/master"), "{bad}");
    }
    // A frozen sheet and an unsupported-pin sheet both offer it to a Master.
    setup
        .store
        .depart(setup.auth[1].clone(), setup.campaign, 3, 1)
        .await
        .unwrap();
    let frozen = get(&setup, 3, "/private-sheets/2").await;
    assert!(
        frozen.contains("id=\"seen-form\"") && frozen.contains("value=\"2\""),
        "{frozen}"
    );
    super::make_unsupported(&setup.store, 1).await;
    let pinned = get(&setup, 3, "/private-sheets/1").await;
    assert!(pinned.contains("id=\"seen-form\""), "{pinned}");
    assert_eq!(
        location(&setup, 3, "/private-sheets/1/seen", seen_form("2"))
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(seen_of(&setup, 4, 1).await, Some(2));
}

// ---------------------------------------------------------------------------------------
// Own saves

#[derive(Clone, Copy, Debug)]
enum Kind {
    Name,
    Field,
    Cell,
    NewRow,
    RemoveRow,
    Quick,
    Switch,
    Undo,
}

/// What Master B's browser submits for `kind` from B's own page.
fn submission(kind: Kind, html: &str) -> (String, Pairs) {
    use super::assist_tests::action;
    use super::unit_tests::unit_form;
    match kind {
        Kind::Name => unit_form(html, "meta-name", Some("Renamed")),
        Kind::Field => unit_form(html, "v0", Some("50")),
        Kind::Cell => unit_form(html, "row-4-0-1", Some("5")),
        Kind::NewRow => new_row(html, 4, &[("row-0", "Lamp"), ("row-1", "2")]),
        Kind::RemoveRow => row_action(html, "remove-4-0"),
        Kind::Quick => unit_form(html, "quick-4", Some("3 torches")),
        Kind::Switch => action(html, "switch-0").unwrap(),
        Kind::Undo => action(html, "undo-0").unwrap(),
    }
}

#[tokio::test]
async fn an_own_save_acknowledges_only_a_revision_the_master_had_seen() {
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
        for (acknowledged_live, expected) in [(true, 3), (false, 1)] {
            let setup = world().await;
            two_revisions(&setup).await;
            // B marks revision 2 (the live one) or only revision 1.
            let marked = if acknowledged_live { "2" } else { "1" };
            assert_eq!(
                location(&setup, 3, "/private-sheets/1/seen", seen_form(marked))
                    .await
                    .0,
                StatusCode::SEE_OTHER
            );
            let html = get(&setup, 3, "/private-sheets/1").await;
            let (path, pairs) = submission(kind, &html);
            let (status, answer) = send(&setup, 3, &path, Some(pairs)).await;
            assert_eq!(status, StatusCode::SEE_OTHER, "{kind:?}: {answer}");
            assert_eq!(saved(&setup, 1).await.0, 3, "{kind:?} wrote revision 3");
            assert_eq!(
                seen_of(&setup, 4, 1).await,
                Some(expected),
                "{kind:?} with the live revision acknowledged: {acknowledged_live}"
            );
            // Master A's own mark (revision 2, from their full-form save) is untouched.
            assert_eq!(seen_of(&setup, 2, 1).await, Some(2), "{kind:?}");
        }
    }
}

#[tokio::test]
async fn a_master_without_a_mark_gets_none_from_a_unit_save_and_create_or_form_saves_acknowledge() {
    let _locks = lock().await;
    let setup = world().await;
    // Create acknowledges revision 1 for a Master creator, not for a player creator.
    assert_eq!(create_as(&setup, 1, false).await, StatusCode::SEE_OTHER);
    assert_eq!(create_as(&setup, 2, false).await, StatusCode::SEE_OTHER);
    assert_eq!(seen_of(&setup, 2, 1).await, Some(1));
    assert_eq!(seen_of(&setup, 3, 2).await, None);
    // B never opened sheet 1: a unit save leaves "Not opened".
    let html = get(&setup, 3, "/private-sheets/1").await;
    let (path, pairs) = submission(Kind::Name, &html);
    assert_eq!(
        send(&setup, 3, &path, Some(pairs)).await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        seen_of(&setup, 4, 1).await,
        None,
        "unit save without a mark"
    );
    // The full form acknowledges its new revision (the page's base is its witness), also
    // without a prior mark, and a list form does the same.
    let (status, html) = send(
        &setup,
        3,
        "/private-sheets/1",
        Some(form(2, &[("value-hp", "10")], &[row("r1-1", "1")])),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
    assert_eq!(seen_of(&setup, 4, 1).await, Some(3));
    let mut list = form(3, &[], &[row("r1-1", "4")]);
    list.retain(|(key, _)| {
        key == "csrf" || key == "revision" || key.contains("-4-") || key.starts_with("id-4")
    });
    list.push(pair("list", "4"));
    let (status, html) = send(&setup, 3, "/private-sheets/1", Some(list)).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
    assert_eq!(seen_of(&setup, 4, 1).await, Some(4));
    assert_eq!(
        seen_of(&setup, 2, 1).await,
        Some(1),
        "A's mark never moves for B's saves"
    );
    // A no-op save changes nothing.
    let (status, same) = send(
        &setup,
        3,
        "/private-sheets/1",
        Some(form(4, &[("value-hp", "10")], &[row("r1-1", "4")])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{same}");
    assert_eq!(seen_of(&setup, 4, 1).await, Some(4));
}

#[tokio::test]
async fn a_departure_freeze_never_acknowledges() {
    let _locks = lock().await;
    let setup = world().await;
    assert_eq!(create_as(&setup, 2, false).await, StatusCode::SEE_OTHER);
    assert_eq!(
        location(&setup, 3, "/private-sheets/1/seen", seen_form("1"))
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    setup
        .store
        .depart(setup.auth[1].clone(), setup.campaign, 3, 1)
        .await
        .unwrap();
    assert_eq!(saved(&setup, 1).await.0, 2, "the freeze is revision 2");
    assert_eq!(
        seen_of(&setup, 4, 1).await,
        Some(1),
        "the freezing Master stays at 1"
    );
    let list = get(&setup, 3, "/campaigns/1/master").await;
    assert!(
        section_text(&list, "players-heading").contains("Changed"),
        "{list}"
    );
}

#[tokio::test]
async fn a_demoted_custodian_saves_without_a_mark_and_the_save_still_works() {
    let _locks = lock().await;
    let setup = world().await;
    assert_eq!(create_as(&setup, 3, true).await, StatusCode::SEE_OTHER);
    assert_eq!(seen_of(&setup, 4, 1).await, Some(1));
    setup
        .store
        .change_role(setup.auth[1].clone(), setup.campaign, 4, 2, false)
        .await
        .unwrap();
    let html = get(&setup, 3, "/private-sheets/1").await;
    let (path, pairs) = submission(Kind::Name, &html);
    let (status, answer) = send(&setup, 3, &path, Some(pairs)).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{answer}");
    assert_eq!(saved(&setup, 1).await.0, 2);
    assert_eq!(
        seen_of(&setup, 4, 1).await,
        Some(1),
        "no acknowledgement without the role"
    );
}

#[tokio::test]
async fn the_shared_save_defaults_to_compare_and_set_for_every_rebased_save() {
    let _locks = lock().await;
    // The store-level save every unit, switch, Undo, interpret and accepted-proposal route
    // calls with its default view: B (mark 1 < live 2) stays "Changed"; B (mark 2) advances.
    // The interpret and accepted-proposal routes are covered here only, at store level: they
    // build their view with `private_read`, and only `save_parsed` ever sets `Ack::Witnessed`.
    for (marked, expected) in [(1, 1), (2, 3)] {
        let setup = world().await;
        two_revisions(&setup).await;
        assert_eq!(
            location(
                &setup,
                3,
                "/private-sheets/1/seen",
                seen_form(&marked.to_string())
            )
            .await
            .0,
            StatusCode::SEE_OTHER
        );
        let permit = crate::definitions::admit().unwrap();
        let (mut view, permit) = setup
            .store
            .private_read(setup.auth[3].clone(), 1, None, permit)
            .await
            .unwrap();
        assert_eq!(view.ack, crate::campaigns::sheets::Ack::CompareAndSet);
        view.snapshot.notes = "Changed by B".into();
        setup
            .store
            .private_save(setup.auth[3].clone(), view, permit)
            .await
            .unwrap();
        assert_eq!(
            seen_of(&setup, 4, 1).await,
            Some(expected),
            "marked {marked}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// No classification in anything a player sees

#[tokio::test]
async fn no_player_visible_response_carries_the_classification() {
    let _locks = lock().await;
    let setup = world().await;
    // B creates an NPC sheet "Assisted sheet", then is demoted and keeps custody.
    assert_eq!(create_as(&setup, 3, true).await, StatusCode::SEE_OTHER);
    assert_eq!(create_as(&setup, 2, false).await, StatusCode::SEE_OTHER);
    let (status, html) = send(
        &setup,
        3,
        "/private-sheets/1",
        Some(form(1, &[("value-hp", "70")], &[row("r1-1", "1")])),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
    setup
        .store
        .change_role(setup.auth[1].clone(), setup.campaign, 4, 2, false)
        .await
        .unwrap();
    let lower = |text: &str| text.to_lowercase();
    // The demoted custodian: every page of their own NPC sheet and the lists.
    for path in [
        "/private-sheets",
        "/campaigns/1",
        "/private-sheets/1",
        "/private-sheets/1/history",
        "/private-sheets/1/history/1",
        "/private-sheets/1/history/2",
    ] {
        let body = get(&setup, 3, path).await;
        assert!(!lower(&body).contains("npc"), "{path}: {body}");
        assert!(
            !body.contains("seen-form") && !body.contains("Master workspace"),
            "{path}"
        );
    }
    assert_eq!(
        send(&setup, 3, "/campaigns/1/master", None).await.0,
        StatusCode::FORBIDDEN
    );
    // A refused draft (stale 409), and the JSON answers of a unit save, as the demoted custodian.
    let (status, stale) = send(
        &setup,
        3,
        "/private-sheets/1",
        Some(form(1, &[("value-hp", "5")], &[row("r1-1", "1")])),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{stale}");
    assert!(!lower(&stale).contains("npc"), "{stale}");
    let html = get(&setup, 3, "/private-sheets/1").await;
    let (path, pairs) = submission(Kind::Field, &html);
    let (status, ok) = json_post(&setup, 3, &path, pairs).await;
    assert_eq!(status, StatusCode::OK, "{ok}");
    assert!(!lower(&ok).contains("npc"), "{ok}");
    let (_, mut stale_pairs) = submission(Kind::Field, &html);
    stale_pairs[1].1 = "1".into();
    stale_pairs[2].1 = "11".into();
    let (status, refused) = json_post(&setup, 3, &path, stale_pairs).await;
    assert!(status.is_client_error(), "{status}: {refused}");
    assert!(!lower(&refused).contains("npc"), "{refused}");
    // The player's views and an ordinary player's campaign page.
    for path in [
        "/private-sheets",
        "/campaigns/1",
        "/private-sheets/2",
        "/private-sheets/2/history",
    ] {
        let body = get(&setup, 2, path).await;
        assert!(!lower(&body).contains("npc"), "{path}: {body}");
    }
    // A current Master's own sheet and history pages carry no marker either.
    for path in [
        "/private-sheets/1",
        "/private-sheets/1/history",
        "/private-sheets/1/history/1",
    ] {
        let body = get(&setup, 1, path).await;
        assert!(!lower(&body).contains("npc"), "{path}: {body}");
    }
}

async fn json_post(setup: &Setup, who: usize, path: &str, pairs: Pairs) -> (StatusCode, String) {
    use tower::ServiceExt;
    let request = axum::http::Request::builder()
        .method("POST")
        .uri(path)
        .header("host", "127.0.0.1:3000")
        .header(crate::auth::TEST_DEADLINE_HEADER, "120")
        .header("cookie", setup.auth[who].fixture_cookie())
        .header("origin", "http://127.0.0.1:3000")
        .header("accept", "application/json")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(axum::body::Body::from(
            serde_urlencoded::to_string(pairs).unwrap(),
        ))
        .unwrap();
    let response = setup.app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    (status, super::formula_tests::body(response).await)
}
