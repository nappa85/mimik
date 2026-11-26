//! The Master's home block of the latest edited sheets (ROAD-8, `docs/auth-and-sessions.md`,
//! Home page): who sees it, what a row says, the badges, the order, the bounds, the tables it
//! reads and the query plan that keeps it off the snapshot overflow pages.
use super::formula_tests::Setup;
use super::home_tests::{at, columns_read, home, mark_at, tables_read};
use super::master_tests::{
    get, location, lock, seen_form, seen_of, send, two_revisions, world, world_with_clock,
};
use super::notes_tests::{form, row};
use axum::http::StatusCode;

const HEADING: &str = "Latest edited sheets in campaigns I run";

/// The block of a home page: its rows (the text of each item) and every link target in it.
struct Block {
    rows: Vec<String>,
    hrefs: Vec<(String, String)>,
    text: String,
}

fn block(html: &str) -> Option<Block> {
    let page = scraper::Html::parse_document(html);
    let sections = scraper::Selector::parse("section.home-block").unwrap();
    let title = scraper::Selector::parse("h3").unwrap();
    let items = scraper::Selector::parse("li").unwrap();
    let links = scraper::Selector::parse("a").unwrap();
    let section = page.select(&sections).find(|section| {
        section
            .select(&title)
            .next()
            .is_some_and(|h| h.text().collect::<String>() == HEADING)
    })?;
    let squash = |text: String| text.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(Block {
        rows: section
            .select(&items)
            .map(|item| squash(item.text().collect()))
            .collect(),
        hrefs: section
            .select(&links)
            .filter_map(|link| {
                Some((
                    link.value().attr("href")?.to_owned(),
                    squash(link.text().collect()),
                ))
            })
            .collect(),
        text: squash(section.text().collect()),
    })
}

/// The row whose sheet link is `/private-sheets/{id}`.
fn row_of(block: &Block, id: i64) -> &str {
    let href = format!("/private-sheets/{id}");
    let name = &block
        .hrefs
        .iter()
        .find(|(h, _)| *h == href)
        .unwrap_or_else(|| panic!("no row for {id}: {:?}", block.hrefs))
        .1;
    block
        .rows
        .iter()
        .find(|row| row.starts_with(&format!("{name} · ")))
        .unwrap_or_else(|| panic!("no row text for {name}: {:?}", block.rows))
}

fn sheet_ids(block: &Block) -> Vec<String> {
    block
        .hrefs
        .iter()
        .filter(|(href, _)| href.starts_with("/private-sheets/"))
        .map(|(href, _)| href.clone())
        .collect()
}

async fn create_named(setup: &Setup, who: usize, campaign: i64, name: &str, npc: bool) {
    let path = format!(
        "/campaigns/{campaign}/sheets/new?version={}{}",
        setup.version,
        if npc { "&npc=1" } else { "" }
    );
    let (status, html) = send(
        setup,
        who,
        &path,
        Some(form(0, &[("name", name)], &[row("", "1")])),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
}

async fn campaign_of(setup: &Setup, who: usize, title: &str) -> i64 {
    setup
        .store
        .create_campaign(
            setup.auth[who].clone(),
            title.into(),
            "synthetic-game".into(),
            "first".into(),
        )
        .await
        .unwrap()
}

async fn block_of(setup: &Setup, who: usize) -> Block {
    block(&home(&setup.app, &setup.auth[who], false).await).expect("the home has the block")
}

#[tokio::test]
async fn a_master_sees_the_latest_edited_sheets_with_names_and_badges() {
    let _locks = lock().await;
    let setup = world().await;
    // Player 3's sheet is named like the classification; Master A's own NPC sheet is not.
    create_named(&setup, 2, setup.campaign, "NPC hero", false).await;
    create_named(&setup, 1, setup.campaign, "Warden", true).await;
    let a = block_of(&setup, 1).await;
    assert_eq!(a.rows.len(), 2, "{:?}", a.rows);
    let player = row_of(&a, 1);
    assert!(
        player.starts_with("NPC hero · Formula · player3 · Not opened · "),
        "{player}"
    );
    assert!(
        !player.contains(" · NPC"),
        "a name is not the classification: {player}"
    );
    let npc = row_of(&a, 2);
    assert!(
        npc.starts_with("Warden · Formula · player2 · Opened · NPC · "),
        "{npc}"
    );
    assert!(
        a.hrefs
            .contains(&("/private-sheets/1".into(), "NPC hero".into()))
    );
    // Everyone else gets no block, and the Owner none either.
    for who in [2, 4] {
        let html = home(&setup.app, &setup.auth[who], false).await;
        assert!(block(&html).is_none() && !html.contains(HEADING), "{who}");
        assert!(!html.contains("Warden"), "{who}");
    }
    assert!(
        !home(&setup.app, &setup.auth[0], true)
            .await
            .contains(HEADING)
    );
    // Opened by acknowledgement, and still listed: the block shows every recent edit.
    assert_eq!(
        location(&setup, 1, "/private-sheets/1/seen", seen_form("1"))
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    let a = block_of(&setup, 1).await;
    assert!(row_of(&a, 1).contains(" · Opened · "), "{:?}", a.rows);
    // An edit by the player: Changed again.
    let (status, html) = send(
        &setup,
        2,
        "/private-sheets/1",
        Some(form(
            1,
            &[("value-hp", "-15"), ("name", "NPC hero")],
            &[row("r1-1", "1")],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
    let a = block_of(&setup, 1).await;
    assert!(row_of(&a, 1).contains(" · Changed · "), "{:?}", a.rows);
    // CAMP-16: A's own save outside a form acknowledges only a revision A had seen. A has
    // not seen revision 2, so the sheet stays Changed, under its new name.
    let html = get(&setup, 1, "/private-sheets/1").await;
    let (path, pairs) = super::unit_tests::unit_form(&html, "meta-name", Some("Renamed"));
    assert_eq!(
        send(&setup, 1, &path, Some(pairs)).await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(seen_of(&setup, 2, 1).await, Some(1));
    let a = block_of(&setup, 1).await;
    assert!(
        row_of(&a, 1).starts_with("Renamed · Formula · player3 · Changed · "),
        "{:?}",
        a.rows
    );
    // The home mark never moves a row: "Mark as seen" changes nothing here.
    // A full-form save acknowledges its own revision.
    let (status, html) = send(
        &setup,
        1,
        "/private-sheets/1",
        Some(form(
            3,
            &[("value-hp", "40"), ("name", "Renamed")],
            &[row("r1-1", "1")],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
    let a = block_of(&setup, 1).await;
    assert!(row_of(&a, 1).contains(" · Opened · "), "{:?}", a.rows);
    // Master B's badges are their own: B never opened either sheet.
    let b = block_of(&setup, 3).await;
    assert!(row_of(&b, 1).contains(" · Not opened · "), "{:?}", b.rows);
    assert!(
        row_of(&b, 2).contains(" · Not opened · NPC · "),
        "{:?}",
        b.rows
    );
}

#[tokio::test]
async fn a_departure_freeze_shows_as_changed_and_frozen() {
    let _locks = lock().await;
    let setup = world().await;
    create_named(&setup, 2, setup.campaign, "Departing", false).await;
    assert_eq!(
        location(&setup, 1, "/private-sheets/1/seen", seen_form("1"))
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    assert!(row_of(&block_of(&setup, 1).await, 1).contains(" · Opened · "));
    setup
        .store
        .depart(setup.auth[1].clone(), setup.campaign, 3, 1)
        .await
        .unwrap();
    let a = block_of(&setup, 1).await;
    assert!(
        row_of(&a, 1).starts_with("Departing · Formula · player3 · Changed · frozen · "),
        "{:?}",
        a.rows
    );
}

#[tokio::test]
async fn the_block_follows_the_current_master_role_and_only_that_campaign() {
    let _locks = lock().await;
    let setup = world().await;
    two_revisions(&setup).await;
    // A is a player of campaign 2, run by account 5; A's sheet there is not A's to oversee.
    let second = campaign_of(&setup, 4, "Other").await;
    setup
        .store
        .request_join(setup.auth[1].clone(), second)
        .await
        .unwrap();
    setup
        .store
        .decide_join(setup.auth[4].clone(), second, 2, 1, true)
        .await
        .unwrap();
    create_named(&setup, 1, second, "Elsewhere", false).await;
    let a = block_of(&setup, 1).await;
    assert_eq!(sheet_ids(&a), ["/private-sheets/1"], "{:?}", a.rows);
    assert!(
        !a.text.contains("Elsewhere") && !a.text.contains("Other"),
        "{}",
        a.text
    );
    // The positive control: campaign 2's own Master sees it.
    let five = block_of(&setup, 4).await;
    assert!(row_of(&five, 2).starts_with("Elsewhere · Other · player2 · Not opened · "));
    // Player 3 also plays in campaign 2 with a sheet of their own there.
    setup
        .store
        .request_join(setup.auth[2].clone(), second)
        .await
        .unwrap();
    setup
        .store
        .decide_join(setup.auth[4].clone(), second, 3, 1, true)
        .await
        .unwrap();
    create_named(&setup, 2, second, "Elsewhere too", false).await;
    // A player promoted sees the block; demoted again, nothing, though the mark survives.
    assert!(block(&home(&setup.app, &setup.auth[2], false).await).is_none());
    setup
        .store
        .change_role(setup.auth[1].clone(), setup.campaign, 3, 1, true)
        .await
        .unwrap();
    let promoted = block_of(&setup, 2).await;
    assert!(row_of(&promoted, 1).contains(" · Not opened · "));
    assert_eq!(
        sheet_ids(&promoted),
        ["/private-sheets/1"],
        "{:?}",
        promoted.rows
    );
    assert_eq!(
        location(&setup, 2, "/private-sheets/1/seen", seen_form("2"))
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    setup
        .store
        .change_role(setup.auth[1].clone(), setup.campaign, 3, 2, false)
        .await
        .unwrap();
    let html = home(&setup.app, &setup.auth[2], false).await;
    assert!(block(&html).is_none() && !html.contains(HEADING));
    assert_eq!(
        seen_of(&setup, 3, 1).await,
        Some(2),
        "the mark survives the demotion"
    );
    // Their sheet in campaign 2 never gives a player's home a name.
    assert!(!html.contains("Elsewhere"));
}

/// A Master who left campaign 1 (an inactive Master row) but still runs campaign 2 sees only
/// campaign 2's sheets: the `state='active'` test of the home query.
#[tokio::test]
async fn a_master_who_left_a_campaign_sees_only_the_campaign_they_still_run() {
    let _locks = lock().await;
    let setup = world().await;
    create_named(&setup, 2, setup.campaign, "Left behind", false).await;
    let second = campaign_of(&setup, 3, "Still run").await;
    create_named(&setup, 3, second, "Kept", false).await;
    let b = block_of(&setup, 3).await;
    assert_eq!(
        sheet_ids(&b),
        ["/private-sheets/2", "/private-sheets/1"],
        "{:?}",
        b.rows
    );
    // B (account 4, member revision 2 after the promotion) leaves campaign 1.
    setup
        .store
        .depart(setup.auth[3].clone(), setup.campaign, 4, 2)
        .await
        .unwrap();
    let b = block_of(&setup, 3).await;
    assert_eq!(sheet_ids(&b), ["/private-sheets/2"], "{:?}", b.rows);
    assert!(
        !b.text.contains("Left behind") && !b.text.contains("Formula"),
        "{}",
        b.text
    );
}

#[tokio::test]
async fn the_block_isolates_player_text_and_escapes_it() {
    let _locks = lock().await;
    let setup = world().await;
    create_named(&setup, 2, setup.campaign, "X\u{202E}abc", false).await;
    create_named(&setup, 2, setup.campaign, "<b>x</b>", false).await;
    let html = home(&setup.app, &setup.auth[1], false).await;
    assert!(!html.contains("<b>x</b>"), "{html}");
    assert!(
        html.contains("&#60;b&#62;x&#60;/b&#62;") || html.contains("&lt;b&gt;x&lt;/b&gt;"),
        "{html}"
    );
    let page = scraper::Html::parse_document(&html);
    let item = scraper::Selector::parse("section.home-block li").unwrap();
    let isolated = scraper::Selector::parse("bdi").unwrap();
    let row = page
        .select(&item)
        .find(|li| li.html().contains("/private-sheets/1\""))
        .expect("the row of sheet 1");
    let inside: Vec<String> = row.select(&isolated).map(|b| b.text().collect()).collect();
    assert_eq!(
        inside,
        ["X\u{202E}abc", "Formula", "player3"],
        "{}",
        row.html()
    );
    let text: String = row.text().collect();
    assert!(text.contains("Not opened"));
    assert!(
        inside.iter().all(|part| !part.contains("Not opened")),
        "the badge is outside every isolated span"
    );
    let markup = page
        .select(&item)
        .find(|li| li.html().contains("/private-sheets/2\""))
        .expect("the row of sheet 2");
    let inside: Vec<String> = markup
        .select(&isolated)
        .map(|b| b.text().collect())
        .collect();
    assert_eq!(inside[0], "<b>x</b>");
}

/// "Mark as seen" with a news item present leaves the block alone, and the time of an edited
/// sheet is no news time.
#[tokio::test]
async fn mark_as_seen_changes_nothing_in_the_block_and_edit_times_are_no_news() {
    let _locks = lock().await;
    let (setup, clock) = world_with_clock().await;
    // The join requests of players 3 and 4 are news at 100; the sheet is edited at 200.
    at(&clock, 200);
    create_named(&setup, 2, setup.campaign, "Marked", false).await;
    at(&clock, 300);
    let before = block_of(&setup, 1).await;
    let html = home(&setup.app, &setup.auth[1], false).await;
    assert!(html.contains("name=\"seen\" value=\"100\""), "{html}");
    let csrf = "a".repeat(64);
    assert_eq!(
        mark_at(&setup.app, &setup.auth[1], &csrf, 100, 300)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let after = block_of(&setup, 1).await;
    assert_eq!(before.rows, after.rows);
    assert!(row_of(&after, 1).contains(" · Not opened · "));
    // The edited sheet's time (200) is newer than any news item: refused.
    assert_eq!(
        mark_at(&setup.app, &setup.auth[1], &csrf, 200, 300)
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

/// Four sheets edited in the same second: the three kept are the highest ids.
#[tokio::test]
async fn a_tie_inside_one_campaign_keeps_the_three_highest_ids() {
    let _locks = lock().await;
    let (setup, clock) = world_with_clock().await;
    at(&clock, 150);
    for index in 1..=4 {
        create_named(&setup, 1, setup.campaign, &format!("Tied {index}"), false).await;
    }
    let a = block_of(&setup, 1).await;
    assert_eq!(
        sheet_ids(&a),
        [
            "/private-sheets/4",
            "/private-sheets/3",
            "/private-sheets/2"
        ],
        "{:?}",
        a.rows
    );
}

/// A player's home never reads a snapshot; a Master's reads it for the names shown only.
#[tokio::test]
async fn only_a_masters_home_reads_the_snapshot() {
    let _locks = lock().await;
    let setup = world().await;
    two_revisions(&setup).await;
    let snapshot = |columns: &[(String, String)]| {
        columns
            .iter()
            .any(|(table, column)| table.starts_with("private_sheet") && column == "snapshot")
    };
    let player = columns_read(&setup.store, || async {
        home(&setup.app, &setup.auth[2], false).await;
    })
    .await;
    assert!(!snapshot(&player), "{player:?}");
    let master = columns_read(&setup.store, || async {
        home(&setup.app, &setup.auth[1], false).await;
    })
    .await;
    assert!(
        master.contains(&("private_sheets".into(), "snapshot".into())),
        "{master:?}"
    );
    assert!(
        !master.contains(&("private_sheet_history".into(), "snapshot".into())),
        "{master:?}"
    );
}

#[tokio::test]
async fn rows_are_newest_first_across_campaigns_and_ties_go_to_the_higher_id() {
    let _locks = lock().await;
    let (setup, clock) = world_with_clock().await;
    let second = campaign_of(&setup, 1, "Second").await;
    for (time, campaign, name) in [
        (110, setup.campaign, "One"),
        (130, second, "Two"),
        (120, setup.campaign, "Three"),
        (120, second, "Four"),
    ] {
        at(&clock, time);
        create_named(&setup, 1, campaign, name, false).await;
    }
    let a = block_of(&setup, 1).await;
    assert_eq!(
        sheet_ids(&a),
        [
            "/private-sheets/2",
            "/private-sheets/4",
            "/private-sheets/3",
            "/private-sheets/1"
        ],
        "{:?}",
        a.rows
    );
}

#[tokio::test]
async fn three_sheets_per_campaign_with_one_workspace_link_each() {
    let _locks = lock().await;
    let (setup, clock) = world_with_clock().await;
    let workspace = format!("/campaigns/{}/master", setup.campaign);
    for index in 1..=5_i64 {
        at(&clock, 100 + 10 * index);
        create_named(&setup, 1, setup.campaign, &format!("Sheet {index}"), false).await;
        let a = block_of(&setup, 1).await;
        let links: Vec<_> = a
            .hrefs
            .iter()
            .filter(|(href, _)| *href == workspace)
            .collect();
        assert_eq!(links.len(), 1, "one workspace link: {:?}", a.hrefs);
        let (_, label) = links[0];
        assert_eq!(
            label,
            if index > 3 {
                "workspace (more)"
            } else {
                "workspace"
            },
            "{index}"
        );
        assert_eq!(sheet_ids(&a).len(), usize::try_from(index.min(3)).unwrap());
        let mut all: Vec<_> = a.hrefs.iter().collect();
        all.sort();
        all.dedup();
        assert_eq!(
            all.len(),
            a.hrefs.len(),
            "no duplicate links: {:?}",
            a.hrefs
        );
    }
    let a = block_of(&setup, 1).await;
    assert_eq!(
        sheet_ids(&a),
        [
            "/private-sheets/5",
            "/private-sheets/4",
            "/private-sheets/3"
        ]
    );
}

#[tokio::test]
async fn twelve_campaigns_show_ten_rows_and_the_note() {
    let _locks = lock().await;
    let (setup, clock) = world_with_clock().await;
    for index in 0..12 {
        let campaign = campaign_of(&setup, 1, &format!("Run {index:02}")).await;
        at(&clock, 200 + index);
        create_named(&setup, 1, campaign, &format!("Sheet {index:02}"), false).await;
    }
    let a = block_of(&setup, 1).await;
    assert_eq!(a.rows.len(), 10, "{:?}", a.rows);
    assert!(a.text.contains("Only the first 10 are shown"), "{}", a.text);
    assert!(
        a.rows[0].starts_with("Sheet 11 · Run 11 · "),
        "{:?}",
        a.rows
    );
    assert!(
        a.hrefs
            .contains(&("/campaigns".into(), "All campaigns".into()))
    );
}

#[tokio::test]
async fn only_a_master_home_reads_the_sheet_tables() {
    let _locks = lock().await;
    let setup = world().await;
    two_revisions(&setup).await;
    let read = |who: usize, owner: bool| {
        let setup = &setup;
        async move {
            tables_read(&setup.store, || async {
                home(&setup.app, &setup.auth[who], owner).await;
            })
            .await
        }
    };
    let player = read(2, false).await;
    for table in ["master_sheet_seen", "private_sheet_history"] {
        assert!(!player.iter().any(|t| t == table), "{table}: {player:?}");
    }
    let owner = read(0, true).await;
    for table in [
        "master_sheet_seen",
        "private_sheet_history",
        "private_sheets",
    ] {
        assert!(!owner.iter().any(|t| t == table), "{table}: {owner:?}");
    }
    let master = read(1, false).await;
    for table in [
        "master_sheet_seen",
        "private_sheet_history",
        "private_sheets",
    ] {
        assert!(master.iter().any(|t| t == table), "{table}: {master:?}");
    }
}

#[tokio::test]
async fn the_name_read_is_guarded_and_falls_back() {
    let _locks = lock().await;
    let setup = world().await;
    create_named(&setup, 1, setup.campaign, "Named", false).await;
    let names = setup
        .store
        .run(|db| crate::campaigns::sheet_names(db, &[1, 999]))
        .await
        .unwrap();
    assert_eq!(names[&1], "Named");
    assert_eq!(names[&999], "(name unavailable)");
}

/// The statement reaches the sheets through the class index and the edit time through the
/// covering history index, and scans neither table.
#[tokio::test]
async fn the_home_query_never_scans_the_sheet_or_history_tables() {
    let _locks = lock().await;
    let (setup, clock) = world_with_clock().await;
    for index in 1..=6 {
        at(&clock, 100 + index);
        create_named(
            &setup,
            1,
            setup.campaign,
            &format!("Sheet {index}"),
            index % 2 == 0,
        )
        .await;
    }
    two_revisions(&setup).await;
    let plan = setup
        .store
        .run(|db| {
            let mut query = db.prepare(&format!(
                "EXPLAIN QUERY PLAN {}",
                crate::auth::EDITED_SHEETS
            ))?;
            let rows = query
                .query_map([2_i64], |row| row.get::<_, String>(3))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
        .unwrap();
    let text = plan.join("\n");
    assert!(text.contains("private_sheets_classes"), "{text}");
    assert!(
        text.contains("USING COVERING INDEX private_sheet_history_time"),
        "{text}"
    );
    for line in &plan {
        for alias in ["s", "h"] {
            assert!(
                line != &format!("SCAN {alias}") && !line.starts_with(&format!("SCAN {alias} ")),
                "{text}"
            );
        }
    }
}
