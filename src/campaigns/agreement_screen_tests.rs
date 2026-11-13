//! The agreement on the review screen (agreement plan, slice 3b): tabs, filtered windows,
//! direct links, the bounded difference, the orphan overlay and the memo.
use super::*;
use crate::sources::agreement::{Exclusion, Verdict};
use std::fmt::Write;

const SETUP: &str = "\
UPDATE source_units SET raw_text='alpha beta' WHERE id=1;\
UPDATE source_units SET raw_text='gamma delta' WHERE id=2;\
UPDATE source_units SET raw_text='epsilon',polygon='[[10,100],[50,100],[50,114],[10,114]]' WHERE id=3;\
UPDATE source_units SET kind='Figure' WHERE id=4;\
UPDATE source_units SET raw_text='zeta',polygon='[[10,130],[50,130],[50,144],[10,144]]' WHERE id=5;\
WITH RECURSIVE n(i) AS (SELECT 52 UNION ALL SELECT i+1 FROM n WHERE i<70) \
INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) \
SELECT i,1,'unit-'||i,'Text','[]','<p>Fixture</p>','Fixture' FROM n;";
const UNIT1_BOX: &str = "[[10,10],[50,10],[50,24],[10,24]]";

fn word(text: &str, bounds: [f32; 4]) -> crate::sources::page_words::PageWord {
    crate::sources::page_words::PageWord {
        text: text.to_owned(),
        bounds,
    }
}

/// Units 1 and 5 agree, unit 2 differs ("deltX"), unit 3 has no word, unit 4 is a figure,
/// the other 65 units have no usable box. 70 units on the page.
fn base_words() -> Vec<crate::sources::page_words::PageWord> {
    vec![
        word("alpha", [11.0, 11.0, 30.0, 21.0]),
        word("beta", [32.0, 11.0, 48.0, 21.0]),
        word("gamma", [62.0, 30.0, 80.0, 40.0]),
        word("deltX", [84.0, 30.0, 100.0, 40.0]),
        word("zeta", [12.0, 131.0, 30.0, 141.0]),
    ]
}

fn independent() -> serde_json::Value {
    serde_json::json!({"name":"marker-pdf","version":"2.0.0","settings":{"force_ocr":true}})
}
fn same_layer() -> serde_json::Value {
    serde_json::json!({"name":"marker-pdf","version":"2.0.0","settings":{}})
}

async fn fixture_with(
    extractor: serde_json::Value,
    words: Vec<crate::sources::page_words::PageWord>,
    extra: &'static str,
) -> (tempfile::TempDir, Store, Vec<RequestAuth>) {
    fixture_sized(extractor, words, extra, 300).await
}

/// As [`fixture_with`] on a page of 200 x `height` points.
async fn fixture_sized(
    extractor: serde_json::Value,
    words: Vec<crate::sources::page_words::PageWord>,
    extra: &'static str,
    height: u32,
) -> (tempfile::TempDir, Store, Vec<RequestAuth>) {
    let leaked: &'static str = Box::leak(format!("{SETUP}{extra}").into_boxed_str());
    let (dir, store, auth) = sized_two_page_fixture(
        leaked,
        UNIT1_BOX,
        (serde_json::json!(200), serde_json::json!(height)),
    )
    .await;
    let bytes = crate::sources::page_words::encode(&words, 200.0, f64::from(height)).unwrap();
    store
        .run(move |db| {
            db.execute(
                "INSERT INTO source_page_words(page_id,words) VALUES(1,?1)",
                [bytes],
            )?;
            let raw: Vec<u8> =
                db.query_row("SELECT prepared_json FROM sources WHERE id=1", [], |r| {
                    r.get(0)
                })?;
            let mut prepared: serde_json::Value = serde_json::from_slice(&raw).unwrap();
            prepared["extractor"] = extractor;
            // The fixture's source is final; this scratch database only needs a provenance.
            let triggers: Vec<String> = db
                .prepare("SELECT name FROM sqlite_master WHERE type='trigger' AND tbl_name='sources' AND sql LIKE '%BEFORE UPDATE%'")?
                .query_map([], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            for name in triggers {
                db.execute_batch(&format!("DROP TRIGGER {name}"))?;
            }
            db.execute(
                "UPDATE sources SET prepared_json=?1 WHERE id=1",
                [serde_json::to_vec(&prepared)?],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    (dir, store, auth)
}

async fn standard() -> (tempfile::TempDir, Store, Vec<RequestAuth>) {
    fixture_with(independent(), base_words(), "").await
}

async fn get(app: &axum::Router, auth: &RequestAuth, query: &str) -> String {
    let response = router_get(app, auth, &format!("/admin/materials/pages/1{query}")).await;
    assert_eq!(response.status(), StatusCode::OK, "{query}");
    body(response).await
}

fn cards(html: &str) -> Vec<i64> {
    scraper::Html::parse_document(html)
        .select(&css("#review-list [data-unit]"))
        .map(|e| e.value().attr("data-unit").unwrap().parse().unwrap())
        .collect()
}

fn text_of(html: &str, selector: &str) -> String {
    scraper::Html::parse_document(html)
        .select(&css(selector))
        .map(|e| e.text().collect::<String>())
        .collect::<Vec<_>>()
        .join("|")
}

fn attention_ids() -> Vec<i64> {
    let mut ids = vec![2, 3, 4];
    ids.extend(6..=51);
    ids.extend(52..=70);
    ids
}

#[tokio::test]
async fn agreement_tabs_have_the_stated_members_counts_and_default() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = standard().await;
    let app = crate::auth::app(store).await.unwrap();
    let all = get(&app, &auth[0], "?show=all").await;
    assert_eq!(
        text_of(&all, "[data-agreement-summary]").trim(),
        "2 match, 1 differ, 66 have no second reading, 1 are excluded, 0 text-layer words are in no unit."
    );
    let tabs = text_of(&all, ".review-tabs a");
    assert_eq!(
        tabs,
        "Needs attention (68)|Matching (2)|Reviewed (0)|All (70)"
    );
    assert!(text_of(&all, "[data-agreement-kind]").contains("read from the page image"));
    assert_eq!(cards(&all).len(), 50);
    // Matching: the two agreeing units; the batch form is not shipped, so full cards.
    let matching = get(&app, &auth[0], "?show=matching").await;
    assert_eq!(cards(&matching), vec![1, 5]);
    assert!(matching.contains("name=\"state\" value=\"accepted\""));
    assert!(!matching.contains("id=\"matching-list\""));
    // Attention: everything else, in reading order, first window.
    let attention = get(&app, &auth[0], "?show=attention").await;
    let shown = cards(&attention);
    assert_eq!(shown, attention_ids()[..50].to_vec());
    // Reviewed: nothing yet.
    assert_eq!(
        cards(&get(&app, &auth[0], "?show=reviewed").await),
        Vec::<i64>::new()
    );
    // The default is `all` (no batch form), an unknown word is the default, `units=` alone is all.
    assert_eq!(cards(&get(&app, &auth[0], "").await).len(), 50);
    let unknown = get(&app, &auth[0], "?show=bogus").await;
    assert_eq!(cards(&unknown).len(), 50);
    assert!(unknown.contains("aria-current=\"page\">All"));
    assert_eq!(cards(&get(&app, &auth[0], "?units=1").await).len(), 50);
}

#[tokio::test]
async fn the_filtered_window_pages_through_its_own_list() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = standard().await;
    let app = crate::auth::app(store).await.unwrap();
    let first = get(&app, &auth[0], "?show=attention").await;
    let mut seen = cards(&first);
    let page = scraper::Html::parse_document(&first);
    let next = page
        .select(&css("a"))
        .find(|a| a.text().collect::<String>() == "Next units")
        .expect("a next link")
        .value()
        .attr("href")
        .unwrap()
        .to_owned();
    assert!(next.contains("show=attention"), "{next}");
    let second = get(
        &app,
        &auth[0],
        next.trim_start_matches("/admin/materials/pages/1"),
    )
    .await;
    seen.extend(cards(&second));
    assert_eq!(seen, attention_ids());
    assert!(!seen.contains(&1) && !seen.contains(&5));
}

#[tokio::test]
async fn a_requested_unit_the_tab_would_hide_shows_all_around_it() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = standard().await;
    let app = crate::auth::app(store).await.unwrap();
    // Unit 2 is in `attention`: the tab stays.
    let kept = get(&app, &auth[0], "?show=attention&unit=2").await;
    assert!(!cards(&kept).contains(&1));
    assert!(!kept.contains("data-tab-notice"));
    // Unit 1 matches: `attention` would hide it, `unit=` shows all.
    let hidden = get(&app, &auth[0], "?show=attention&unit=1").await;
    assert!(cards(&hidden).contains(&1) && cards(&hidden).contains(&2));
    assert!(hidden.contains("data-tab-notice"));
    // A save's `to` does the same.
    let to = get(&app, &auth[0], "?show=attention&to=1").await;
    assert!(cards(&to).contains(&1));
    // The matching tab hides unit 2.
    let matching = get(&app, &auth[0], "?show=matching&unit=2").await;
    assert!(cards(&matching).contains(&2));
    // Every unit-link builder carries `unit=`: the source review list and the overlay.
    let review = router_get(&app, &auth[0], "/admin/materials/sources/1/review").await;
    let review = body(review).await;
    assert!(review.contains("unit=1#unit-1"), "{review}");
    let overlay_far = get(&app, &auth[0], "?show=matching").await;
    assert!(
        overlay_far.contains("?units=1&#38;unit=2#unit-2")
            || overlay_far.contains("?units=1&amp;unit=2#unit-2")
            || overlay_far.contains("?units=1&unit=2#unit-2"),
        "an overlay link to a unit outside the window keeps unit="
    );
}

#[tokio::test]
async fn show_round_trips_through_a_save_to_the_next_unit_of_the_tab() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = standard().await;
    let app = crate::auth::app(store).await.unwrap();
    let decide = |unit: i64, show: Option<&str>| {
        let mut fields = vec![
            ("csrf".to_owned(), "a".repeat(64)),
            ("revision".to_owned(), "1".to_owned()),
            ("state".to_owned(), "accepted".to_owned()),
            ("correction".to_owned(), String::new()),
            ("rationale".to_owned(), String::new()),
        ];
        if let Some(show) = show {
            fields.push(("show".to_owned(), show.to_owned()));
        }
        (unit, fields)
    };
    // Accepting unit 1 from the matching tab goes to the next matching unit, 5 (the all view
    // would go to 2).
    let (unit, fields) = decide(1, Some("matching"));
    let response = document_at(
        &app,
        &auth[0],
        &format!("/admin/materials/pages/1/units/{unit}"),
        fields,
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response.headers()["location"].to_str().unwrap().to_owned();
    assert!(
        location.contains("to=5&then=unit&show=matching#unit-5"),
        "{location}"
    );
    let landed = get(
        &app,
        &auth[0],
        location.trim_start_matches("/admin/materials/pages/1"),
    )
    .await;
    assert_eq!(cards(&landed), vec![5]);
    // Without `show` (an old form) the all view's rule applies: the next unreviewed unit
    // after the saved one, 6.
    let (unit, fields) = decide(5, None);
    let response = document_at(
        &app,
        &auth[0],
        &format!("/admin/materials/pages/1/units/{unit}"),
        fields,
    )
    .await;
    let location = response.headers()["location"].to_str().unwrap().to_owned();
    assert!(location.contains("to=6&then=unit#unit-6"), "{location}");
    // From the attention tab: the next attention unit after unit 2 is 3.
    let (unit, fields) = decide(2, Some("attention"));
    let response = document_at(
        &app,
        &auth[0],
        &format!("/admin/materials/pages/1/units/{unit}"),
        fields,
    )
    .await;
    let location = response.headers()["location"].to_str().unwrap().to_owned();
    assert!(
        location.contains("to=3&then=unit&show=attention#unit-3"),
        "{location}"
    );
    // The header's next link is per tab: the first attention unit now is 3.
    let page = get(&app, &auth[0], "?show=attention").await;
    let next = scraper::Html::parse_document(&page)
        .select(&css(".review-nav a"))
        .find(|a| a.text().collect::<String>() == "Next unreviewed unit")
        .and_then(|a| a.value().attr("href").map(str::to_owned))
        .expect("a next link");
    assert_eq!(next, "#unit-3");
    // A refused save renders with every unit, so its draft binding stays valid.
    let mut stale = decide(3, Some("attention")).1;
    stale[1].1 = "99".into();
    let response = document_at(&app, &auth[0], "/admin/materials/pages/1/units/3", stale).await;
    assert_ne!(response.status(), StatusCode::SEE_OTHER);
    let refused = body(response).await;
    assert!(refused.contains("data-agreement-summary"));
    assert!(refused.contains("aria-current=\"page\">All"));
    assert!(refused.contains("name=\"show\" value=\"all\""));
    assert!(refused.contains("show=all#unit-3"));
}

#[tokio::test]
async fn a_difference_is_escaped_and_bounded() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = fixture_with(
        independent(),
        base_words(),
        "UPDATE source_units SET raw_text='gamma <script>x</script> & \"delta\"' WHERE id=2;",
    )
    .await;
    let app = crate::auth::app(store).await.unwrap();
    let html = get(&app, &auth[0], "?show=attention").await;
    assert!(!html.contains("<script>x</script>"));
    let diff = text_of(&html, "[data-agreement-diff]");
    assert!(diff.contains("<script>x</script>"), "{diff}");
    assert!(diff.contains("PDF text layer"), "{diff}");
    assert!(
        text_of(&html, "[data-agreement=\"differs\"]").contains("Differs from the PDF text layer")
    );
    // A long unit text is clipped on the card.
    let long: &'static str = Box::leak(
        format!(
            "UPDATE source_units SET raw_text='{}' WHERE id=2;",
            (0..1500)
                .map(|i| format!("w{i}"))
                .collect::<Vec<_>>()
                .join(" ")
        )
        .into_boxed_str(),
    );
    let (_dir2, store2, auth2) = fixture_with(independent(), base_words(), long).await;
    let app2 = crate::auth::app(store2).await.unwrap();
    let html = get(&app2, &auth2[0], "?show=attention").await;
    let diff = text_of(&html, "[data-agreement-diff]");
    assert!(
        !diff.is_empty() && diff.chars().count() < 1_200,
        "{}",
        diff.chars().count()
    );
    assert!(diff.contains("More differences are not shown") || diff.contains('\u{2026}'));
}

#[tokio::test]
async fn orphan_words_are_outlined_with_a_bound() {
    let _lock = crate::auth::test_resource_lock().await;
    let mut words = base_words();
    for i in 0..2_105_u32 {
        let (col, row) = (
            f32::from(u16::try_from(i % 36).unwrap()),
            f32::from(u16::try_from(i / 36).unwrap()),
        );
        let (x, y) = (140.0 + col * 1.5, 100.0 + row * 1.5);
        words.push(word(&format!("o{i}"), [x, y, x + 1.0, y + 1.0]));
    }
    let (_dir, store, auth) = fixture_with(independent(), words, "").await;
    let app = crate::auth::app(store).await.unwrap();
    let html = get(&app, &auth[0], "?show=all").await;
    let page = scraper::Html::parse_document(&html);
    let drawn = page
        .select(&css("#review-overlay path.orphan-word"))
        .next()
        .and_then(|p| p.value().attr("d"))
        .unwrap_or_default();
    assert_eq!(drawn.matches('M').count(), 2_000);
    assert!(html.contains("105 text-layer words in no unit are not drawn"));
    assert!(
        text_of(&html, "[data-agreement-summary]").contains("2105 text-layer words are in no unit")
    );
    assert!(text_of(&html, ".review-legend").contains("PDF text-layer word in no unit"));
    // The bound is the drawing's only: the response stays well inside its budget.
    assert!(html.len() < 2 * 1024 * 1024);
}

#[tokio::test]
async fn a_same_layer_page_shows_full_cards_in_matching() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = fixture_with(same_layer(), base_words(), "").await;
    let app = crate::auth::app(store).await.unwrap();
    let html = get(&app, &auth[0], "?show=matching").await;
    assert_eq!(cards(&html), vec![1, 5]);
    assert!(text_of(&html, "[data-agreement-kind]").contains("carried over intact"));
    assert!(!html.contains("id=\"matching-list\""));
    assert!(html.contains("name=\"state\" value=\"accepted\""));
}

#[tokio::test]
async fn a_page_without_words_says_why_and_falls_back_to_all() {
    let _lock = crate::auth::test_resource_lock().await;
    // A stored row with no word in it is no second reading.
    let (_dir, store, auth) = fixture_with(independent(), Vec::new(), "").await;
    let app = crate::auth::app(store).await.unwrap();
    let html = get(&app, &auth[0], "?show=matching").await;
    assert!(text_of(&html, "[data-agreement-none]").contains("no stored PDF text-layer words"));
    assert!(html.contains("data-tab-notice"));
    assert_eq!(cards(&html).len(), 50);
    assert_eq!(text_of(&html, ".review-tabs a"), "Reviewed (0)|All (70)");
}

#[tokio::test]
async fn the_memo_sees_a_dismissed_suggestion_at_once() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = standard().await;
    let verdict = |view: &crate::sources::agreement_view::AgreementView, id: i64| {
        view.unit(id).unwrap().verdict.clone()
    };
    let id = suggestion_fixture(&store, "correction", suggestion_unit(1)).await;
    let first = store.material_agreement(auth[0].clone(), 1).await.unwrap();
    assert_eq!(
        verdict(&first, 1),
        Verdict::Excluded(Exclusion::OpenSuggestion)
    );
    assert_eq!(verdict(&first, 5), Verdict::Agrees);
    let computed = store.2.passes_computed();
    assert_eq!(computed, 1);
    store
        .material_dismiss(auth[0].clone(), auth[0].csrf_fixture(), id, String::new())
        .await
        .unwrap();
    let second = store.material_agreement(auth[0].clone(), 1).await.unwrap();
    assert_eq!(verdict(&second, 1), Verdict::Agrees);
    // The pass itself was reused: only the marks changed.
    assert_eq!(store.2.passes_computed(), computed);
    // A decision advances the page revision, which is a new key.
    store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            5,
            crate::sources::UnitReview {
                revision: 1,
                state: "accepted".into(),
                correction: String::new(),
                rationale: String::new(),
                actor: String::new(),
            },
        )
        .await
        .unwrap();
    let third = store.material_agreement(auth[0].clone(), 1).await.unwrap();
    assert_eq!(store.2.passes_computed(), computed + 1);
    assert_eq!(verdict(&third, 1), Verdict::Agrees);
}

// ---------------------------------------------------------------------------------------
// Review follow-ups
// ---------------------------------------------------------------------------------------

fn units_sql(from: u32, to: u32) -> String {
    format!(
        "WITH RECURSIVE n(i) AS (SELECT {from} UNION ALL SELECT i+1 FROM n WHERE i<{to}) \
         INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) \
         SELECT i,1,'unit-'||i,'Text','[[150.5,150.5],[199.5,150.5],[199.5,199.5],[150.5,199.5]]','<p>Fixture</p>','Fixture' FROM n;"
    )
}

/// The worst case the response bound must hold for: 2,000 units, 2,105 orphan words, nine cards
/// of maximal escaped text (the card budget is saturated), and 41 cards with 20 changes each.
async fn worst_case() -> (tempfile::TempDir, Store, Vec<RequestAuth>) {
    let mut sql = String::new();
    let mut words = Vec::new();
    for id in 1..=9 {
        let _ = write!(
            sql,
            "UPDATE source_units SET raw_text=replace(hex(zeroblob(2400)),'00','&<>\"'' ') WHERE id={id};"
        );
    }
    for id in 10..=50_u32 {
        let y = 400 + (id - 10) * 40;
        let _ = write!(
            sql,
            "UPDATE source_units SET raw_text='{}',polygon='[[5,{y}],[195,{y}],[195,{}],[5,{}]]' WHERE id={id};",
            (0..40)
                .map(|k| format!("a{k}"))
                .collect::<Vec<_>>()
                .join(" "),
            y + 14,
            y + 14
        );
        for k in 0..40_u32 {
            let x = 6.0 + 4.4 * f32::from(u16::try_from(k).unwrap());
            let y = f32::from(u16::try_from(y).unwrap());
            let text = if k % 2 == 0 {
                format!("a{k}")
            } else {
                format!("b{k}")
            };
            words.push(word(&text, [x, y + 2.0, x + 4.0, y + 12.0]));
        }
    }
    // Page 2's unit is 900: 2,000 units on page 1 are ids 1..=899 and 901..=2001.
    sql += &units_sql(71, 899);
    sql += &units_sql(901, 2001);
    for i in 0..2_105_u32 {
        let (col, row) = (
            f32::from(u16::try_from(i % 36).unwrap()),
            f32::from(u16::try_from(i / 36).unwrap()),
        );
        let (x, y) = (140.0 + col * 1.5, 2200.0 + row * 1.5);
        words.push(word(&format!("o{i}"), [x, y, x + 1.0, y + 1.0]));
    }
    fixture_sized(independent(), words, Box::leak(sql.into_boxed_str()), 3000).await
}

#[tokio::test]
async fn the_worst_case_page_stays_inside_the_response_budget() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = worst_case().await;
    let app = crate::auth::app(store).await.unwrap();
    for query in ["?show=all", "?show=attention"] {
        let html = get(&app, &auth[0], query).await;
        let page = scraper::Html::parse_document(&html);
        // The case is the worst one: overlay, orphans, diffs and a saturated card budget.
        assert!(
            page.select(&css("#review-overlay polygon[data-overlay-unit]"))
                .count()
                >= 1_900
        );
        assert!(html.contains("105 text-layer words in no unit are not drawn"));
        assert_eq!(page.select(&css("[data-agreement-diff] li")).count(), 200);
        assert!(html.contains("Correction form omitted"));
        assert!(
            html.len() <= 2 * 1024 * 1024,
            "{query}: {} bytes",
            html.len()
        );
    }
    // A refused save of the largest draft renders the same page with the draft on its first card.
    let mut fields = reject(99, "all");
    fields[2].1 = "corrected".into();
    fields[3].1 = "&".repeat(20_000);
    fields[4].1 = "&".repeat(5_000);
    let response = document_at(&app, &auth[0], "/admin/materials/pages/1/units/10", fields).await;
    assert_ne!(response.status(), StatusCode::SEE_OTHER);
    let html = body(response).await;
    assert!(html.contains("[data-agreement-diff]") || html.contains("data-agreement-diff"));
    assert!(
        html.len() <= 2 * 1024 * 1024,
        "refused: {} bytes",
        html.len()
    );
}

#[tokio::test]
async fn a_missing_page_is_forbidden_like_every_other_route() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = standard().await;
    let app = crate::auth::app(store).await.unwrap();
    let page = router_get(&app, &auth[0], "/admin/materials/pages/9999").await;
    let image = router_get(&app, &auth[0], "/admin/materials/pages/9999/image").await;
    assert_eq!(image.status(), StatusCode::FORBIDDEN);
    assert_eq!(page.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_derived_page_has_no_comparison() {
    let _lock = crate::auth::test_resource_lock().await;
    let derived = serde_json::json!({"name":"mimik-route-a","version":"1","settings":{}});
    let (_dir, store, auth) = fixture_with(derived, base_words(), "").await;
    let app = crate::auth::app(store).await.unwrap();
    let html = get(&app, &auth[0], "?show=matching").await;
    assert!(
        text_of(&html, "[data-agreement-none]")
            .contains("built from the PDF text layer's words; no comparison"),
        "{}",
        text_of(&html, "#agreement-bar")
    );
    assert!(!html.contains("data-agreement-summary"));
    assert_eq!(text_of(&html, ".review-tabs a"), "Reviewed (0)|All (70)");
}

fn reject(revision: i64, show: &str) -> Vec<(String, String)> {
    vec![
        ("csrf".to_owned(), "a".repeat(64)),
        ("revision".to_owned(), revision.to_string()),
        ("state".to_owned(), "rejected".to_owned()),
        ("correction".to_owned(), String::new()),
        ("rationale".to_owned(), "not evidence".to_owned()),
        ("show".to_owned(), show.to_owned()),
    ]
}

#[tokio::test]
async fn re_deciding_on_a_page_without_words_stays_on_the_reviewed_tab() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = fixture_with(independent(), Vec::new(), "").await;
    let app = crate::auth::app(store).await.unwrap();
    let first = document_at(
        &app,
        &auth[0],
        "/admin/materials/pages/1/units/1",
        reject(1, "all"),
    )
    .await;
    assert_eq!(first.status(), StatusCode::SEE_OTHER);
    // Unit 1 is decided (revision 2); re-deciding it stays on the reviewed tab.
    let again = document_at(
        &app,
        &auth[0],
        "/admin/materials/pages/1/units/1",
        reject(2, "reviewed"),
    )
    .await;
    assert_eq!(again.status(), StatusCode::SEE_OTHER);
    let location = again.headers()["location"].to_str().unwrap().to_owned();
    assert!(
        location.contains("then=stay") && location.contains("show=reviewed"),
        "{location}"
    );
    let html = get(
        &app,
        &auth[0],
        location.trim_start_matches("/admin/materials/pages/1"),
    )
    .await;
    assert!(
        !html.contains("data-tab-notice"),
        "{}",
        text_of(&html, "#agreement-bar")
    );
    assert!(html.contains("aria-current=\"page\">Reviewed"));
    assert_eq!(cards(&html), vec![1]);
}

#[tokio::test]
async fn an_unknown_next_unit_still_names_its_unit() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = standard().await;
    store
        .force_read_deadline("next", std::time::Duration::ZERO)
        .await;
    let app = crate::auth::app(store).await.unwrap();
    let response = document_at(
        &app,
        &auth[0],
        "/admin/materials/pages/1/units/1",
        accept_fields("all"),
    )
    .await;
    let location = response.headers()["location"].to_str().unwrap().to_owned();
    assert!(location.contains("to=1&then=unknown"), "{location}");
    // The anchor stays valid on a hiding tab: the page names the unit it wants.
    let html = get(&app, &auth[0], "?show=attention&to=1").await;
    assert!(html.contains("id=\"unit-1\""));
}

fn accept_fields(show: &str) -> Vec<(String, String)> {
    vec![
        ("csrf".to_owned(), "a".repeat(64)),
        ("revision".to_owned(), "1".to_owned()),
        ("state".to_owned(), "accepted".to_owned()),
        ("correction".to_owned(), String::new()),
        ("rationale".to_owned(), String::new()),
        ("show".to_owned(), show.to_owned()),
    ]
}

#[tokio::test]
async fn a_re_decision_does_not_compute_the_agreement() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = standard().await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let first = document_at(
        &app,
        &auth[0],
        "/admin/materials/pages/1/units/1",
        accept_fields("all"),
    )
    .await;
    assert_eq!(first.status(), StatusCode::SEE_OTHER);
    let before = store.2.passes_computed();
    let again = document_at(
        &app,
        &auth[0],
        "/admin/materials/pages/1/units/1",
        reject(2, "all"),
    )
    .await;
    assert_eq!(again.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        store.2.passes_computed(),
        before,
        "the stay path reads no agreement"
    );
}

#[tokio::test]
async fn a_difference_too_long_to_display_says_so() {
    let _lock = crate::auth::test_resource_lock().await;
    let long: &'static str = Box::leak(
        format!(
            "UPDATE source_units SET raw_text='{}' WHERE id=2;",
            (0..2100)
                .map(|i| format!("w{i}"))
                .collect::<Vec<_>>()
                .join(" ")
        )
        .into_boxed_str(),
    );
    let (_dir, store, auth) = fixture_with(independent(), base_words(), long).await;
    let app = crate::auth::app(store).await.unwrap();
    let html = get(&app, &auth[0], "?show=attention&unit=2").await;
    assert!(html.contains("The difference is too long to display"));
}

#[tokio::test]
async fn a_storage_error_is_not_reported_as_a_time_limit() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = standard().await;
    store
        .run(|db| {
            db.execute_batch("ALTER TABLE source_page_words RENAME TO words_gone")?;
            Ok(())
        })
        .await
        .unwrap();
    let app = crate::auth::app(store).await.unwrap();
    let response = router_get(&app, &auth[0], "/admin/materials/pages/1").await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body(response).await;
    let none = text_of(&html, "[data-agreement-none]");
    assert!(none.contains("could not be read"), "{none}");
    assert!(!none.contains("time limit"), "{none}");
}

/// Acts as if slice 4's batch form shipped, for the test's own thread.
struct BatchOn;
impl BatchOn {
    fn new() -> Self {
        crate::sources::agreement_view::FORCE_BATCH.set(true);
        Self
    }
}
impl Drop for BatchOn {
    fn drop(&mut self) {
        crate::sources::agreement_view::FORCE_BATCH.set(false);
    }
}

fn link_with(html: &str, text: &str) -> String {
    scraper::Html::parse_document(html)
        .select(&css("a"))
        .find(|a| a.text().collect::<String>() == text)
        .unwrap_or_else(|| panic!("a link {text:?}"))
        .value()
        .attr("href")
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn with_the_form_offered_the_default_is_attention_and_units_alone_is_all() {
    let _lock = crate::auth::test_resource_lock().await;
    let _batch = BatchOn::new();
    let (_dir, store, auth) = standard().await;
    let app = crate::auth::app(store).await.unwrap();
    let default = get(&app, &auth[0], "").await;
    assert_eq!(cards(&default), attention_ids()[..50].to_vec());
    assert!(default.contains("aria-current=\"page\">Needs attention"));
    // `units=` without `show` is `all`: unit 5 matches yet is listed.
    let cursor = get(&app, &auth[0], "?units=1").await;
    assert!(cards(&cursor).contains(&5));
    assert!(cursor.contains("aria-current=\"page\">All"));
    // An unknown word is the default.
    assert_eq!(
        cards(&get(&app, &auth[0], "?show=bogus").await),
        cards(&default)
    );
    // Without a result the default is `all`.
    let (_d2, store2, auth2) = fixture_with(independent(), Vec::new(), "").await;
    let app2 = crate::auth::app(store2).await.unwrap();
    let none = get(&app2, &auth2[0], "").await;
    assert!(none.contains("aria-current=\"page\">All"));
    assert!(!none.contains("data-tab-notice"));
}

#[tokio::test]
async fn a_same_layer_page_never_offers_the_form_even_when_it_ships() {
    let _lock = crate::auth::test_resource_lock().await;
    let _batch = BatchOn::new();
    let (_dir, store, auth) = fixture_with(same_layer(), base_words(), "").await;
    let app = crate::auth::app(store).await.unwrap();
    // The default is `all` and `matching` has full cards.
    assert_eq!(cards(&get(&app, &auth[0], "").await).len(), 50);
    let html = get(&app, &auth[0], "?show=matching").await;
    assert_eq!(cards(&html), vec![1, 5]);
    assert!(!html.contains("id=\"matching-list\""));
}

fn many_matching_sql() -> (&'static str, Vec<crate::sources::page_words::PageWord>) {
    let mut sql = String::new();
    let mut words = base_words();
    for i in 101..=205_u32 {
        let y = 400 + (i - 101) * 20;
        let _ = write!(
            sql,
            "INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES({i},1,'unit-{i}','Text','[[10,{y}],[60,{y}],[60,{}],[10,{}]]','<p>m</p>','m{i}');",
            y + 12,
            y + 12
        );
        let y = f32::from(u16::try_from(y).unwrap());
        words.push(word(&format!("m{i}"), [12.0, y + 1.0, 40.0, y + 11.0]));
    }
    (Box::leak(sql.into_boxed_str()), words)
}

#[tokio::test]
async fn the_compact_matching_list_has_rows_a_next_link_and_no_cards() {
    let _lock = crate::auth::test_resource_lock().await;
    let _batch = BatchOn::new();
    let (sql, words) = many_matching_sql();
    let (_dir, store, auth) = fixture_sized(independent(), words, sql, 3000).await;
    let app = crate::auth::app(store).await.unwrap();
    let html = get(&app, &auth[0], "?show=matching").await;
    let page = scraper::Html::parse_document(&html);
    // 107 matching units: the first window lists 100 rows and no card.
    assert_eq!(cards(&html), Vec::<i64>::new());
    assert!(!html.contains("name=\"state\""));
    let rows: Vec<String> = page
        .select(&css("#matching-list li a"))
        .map(|a| a.value().attr("href").unwrap().to_owned())
        .collect();
    assert_eq!(rows.len(), 100);
    assert!(
        rows[0].contains("unit=1") && rows[0].contains("show=all") && rows[0].ends_with("#unit-1"),
        "{}",
        rows[0]
    );
    assert!(
        text_of(&html, "#matching-list li").contains("alpha beta")
            || text_of(&html, "#matching-list li").contains("Unit 1")
    );
    assert!(text_of(&html, "#matching-list").contains("use the all view"));
    // The next window has the other seven; the row links open the card in the all view.
    let next = link_with(&html, "Next matching units");
    assert!(
        next.contains("show=matching") && next.contains("units="),
        "{next}"
    );
    let second = get(
        &app,
        &auth[0],
        next.trim_start_matches("/admin/materials/pages/1"),
    )
    .await;
    assert_eq!(
        scraper::Html::parse_document(&second)
            .select(&css("#matching-list li"))
            .count(),
        7
    );
    let landed = get(
        &app,
        &auth[0],
        rows[0].trim_start_matches("/admin/materials/pages/1"),
    )
    .await;
    assert!(cards(&landed).contains(&1));
    assert!(landed.contains("aria-current=\"page\">All"));
}

#[tokio::test]
async fn a_timed_out_agreement_or_proposals_read_says_so_and_falls_back() {
    let _lock = crate::auth::test_resource_lock().await;
    for label in ["agreement", "proposals"] {
        let (_dir, store, auth) = standard().await;
        store
            .force_read_deadline(label, std::time::Duration::ZERO)
            .await;
        let app = crate::auth::app(store).await.unwrap();
        let html = get(&app, &auth[0], "?show=attention").await;
        assert!(
            text_of(&html, "[data-agreement-none]").contains("time limit"),
            "{label}: {}",
            text_of(&html, "#agreement-bar")
        );
        assert!(html.contains("data-tab-notice"));
        assert_eq!(text_of(&html, ".review-tabs a"), "Reviewed (0)|All (70)");
        assert_eq!(cards(&html).len(), 50);
    }
}

#[tokio::test]
async fn a_page_over_the_unit_bound_has_no_comparison() {
    let _lock = crate::auth::test_resource_lock().await;
    // Page 1 gets 2,001 units (page 2's unit is 900).
    let sql = format!("{}{}", units_sql(71, 899), units_sql(901, 2002));
    let (_dir, store, auth) = fixture_sized(
        independent(),
        base_words(),
        Box::leak(sql.into_boxed_str()),
        300,
    )
    .await;
    let app = crate::auth::app(store).await.unwrap();
    let html = get(&app, &auth[0], "?show=matching").await;
    assert!(text_of(&html, "[data-agreement-none]").contains("more than 2,000 units"));
    assert!(html.contains("data-tab-notice"));
    assert_eq!(cards(&html).len(), 50);
}

#[tokio::test]
async fn the_reduced_screen_omits_tab_counts() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = standard().await;
    store
        .force_read_deadline("screen", std::time::Duration::ZERO)
        .await;
    let app = crate::auth::app(store).await.unwrap();
    let html = get(&app, &auth[0], "").await;
    assert!(html.contains("Screen read deadline reached"));
    let tabs = text_of(&html, ".review-tabs a");
    assert_eq!(tabs, "Needs attention|Matching|Reviewed|All");
    assert!(!tabs.contains('('));
}

#[tokio::test]
async fn the_next_unit_of_a_tab_past_the_first_window_carries_its_cursor() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = standard().await;
    let app = crate::auth::app(store).await.unwrap();
    // Unit 50 is the last of the first window; the next attention unit, 51, is at position 51.
    let response = document_at(
        &app,
        &auth[0],
        "/admin/materials/pages/1/units/50",
        accept_fields("attention"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response.headers()["location"].to_str().unwrap().to_owned();
    assert_eq!(
        location,
        "/admin/materials/pages/1?units=50&saved=accepted&saved_unit=50&to=51&then=unit&show=attention#unit-51"
    );
    let landed = get(
        &app,
        &auth[0],
        location.trim_start_matches("/admin/materials/pages/1"),
    )
    .await;
    assert_eq!(cards(&landed)[0], 51);
}

fn label_of(html: &str, unit: i64) -> String {
    text_of(html, &format!("#unit-{unit} [data-agreement]"))
}

#[tokio::test]
async fn caller_exclusions_label_their_cards() {
    let _lock = crate::auth::test_resource_lock().await;
    // Unit 6 reads as a table by its text alone (a shape scoring 80, two body rows: no
    // proposal); unit 8 is four roll rows (a proposal).
    let sql = "UPDATE source_units SET raw_text='d6 Result\n1 Rat\n2 Crow' WHERE id=6;\
               UPDATE source_units SET raw_text='d6 Result' WHERE id=7;\
               UPDATE source_units SET raw_text='1 Rat\n2 Crow\n3 Wolf\n4 Bear' WHERE id=8;\
               UPDATE source_units SET raw_text='5 Cat\n6 Dog' WHERE id=9;";
    let (_dir, store, auth) = fixture_with(independent(), base_words(), sql).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let html = get(&app, &auth[0], "?show=all").await;
    assert!(
        label_of(&html, 6).contains("looks like a table"),
        "{}",
        label_of(&html, 6)
    );
    assert!(html.contains("id=\"table-proposals\""));
    assert!(
        label_of(&html, 8).contains("inside a likely table"),
        "{}",
        label_of(&html, 8)
    );
    // An open suggestion on a unit.
    suggestion_fixture(&store, "correction", suggestion_unit(1)).await;
    let html = get(&app, &auth[0], "?show=all").await;
    assert!(label_of(&html, 1).contains("an open suggestion targets it"));
    assert!(label_of(&html, 5).contains("Matches the PDF text layer"));
    // A page-level suggestion marks every candidate.
    suggestion_fixture(
        &store,
        "note",
        crate::sources::SuggestionTarget::Page {
            page: 1,
            block_id: None,
        },
    )
    .await;
    let html = get(&app, &auth[0], "?show=all").await;
    for unit in [1, 5] {
        assert!(
            label_of(&html, unit).contains("open page-level suggestion"),
            "{unit}: {}",
            label_of(&html, unit)
        );
    }
    // A figure keeps its own reason.
    assert!(label_of(&html, 4).contains("not running text"));
}

#[tokio::test]
async fn the_derived_status_is_not_memoised_as_a_comparison() {
    let _lock = crate::auth::test_resource_lock().await;
    let derived = serde_json::json!({"name":"mimik-route-a","version":"1","settings":{}});
    let (_dir, store, auth) = fixture_with(derived, base_words(), "").await;
    let view = store.material_agreement(auth[0].clone(), 1).await.unwrap();
    assert_eq!(view.status, crate::sources::agreement_view::Status::Derived);
    assert!(view.units.is_empty());
}
