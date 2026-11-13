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
    // Matching: the two agreeing units as a compact list under the batch form, not cards.
    let matching = get(&app, &auth[0], "?show=matching").await;
    assert_eq!(cards(&matching), Vec::<i64>::new());
    assert!(matching.contains("id=\"matching-list\""));
    assert!(matching.contains("id=\"agreement-batch\""));
    assert_eq!(
        scraper::Html::parse_document(&matching)
            .select(&css("#matching-list li"))
            .count(),
        2
    );
    // Attention: everything else, in reading order, first window.
    let attention = get(&app, &auth[0], "?show=attention").await;
    let shown = cards(&attention);
    assert_eq!(shown, attention_ids()[..50].to_vec());
    // Reviewed: nothing yet.
    assert_eq!(
        cards(&get(&app, &auth[0], "?show=reviewed").await),
        Vec::<i64>::new()
    );
    // The default is `attention` (the batch form is offered), an unknown word is the default,
    // `units=` alone is all.
    assert_eq!(
        cards(&get(&app, &auth[0], "").await),
        attention_ids()[..50].to_vec()
    );
    let unknown = get(&app, &auth[0], "?show=bogus").await;
    assert_eq!(cards(&unknown), attention_ids()[..50].to_vec());
    assert!(unknown.contains("aria-current=\"page\">Needs attention"));
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
    assert_eq!(cards(&landed), Vec::<i64>::new());
    assert_eq!(
        text_of(&landed, "#matching-list li"),
        "Unit 5 (Text) \u{2014} zeta"
    );
    // Without `show` (an old form) the page's default tab applies, `attention` while the
    // batch form is offered: the next attention unit after unit 5, 6.
    let (unit, fields) = decide(5, None);
    let response = document_at(
        &app,
        &auth[0],
        &format!("/admin/materials/pages/1/units/{unit}"),
        fields,
    )
    .await;
    let location = response.headers()["location"].to_str().unwrap().to_owned();
    assert!(
        location.contains("to=6&then=unit&show=attention#unit-6"),
        "{location}"
    );
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

/// The worst case the response bound must hold for: 2,000 units, 2,105 orphan words, `huge`
/// cards of maximal escaped text, 1,030 planted matching units and 41 cards with 20 changes each.
/// The comparison's 64 KiB text budget is shared by every candidate unit, so the cards and the
/// matching units cannot both be at their maximum: nine huge cards saturate the card budget and
/// leave room for about 430 matching units; three leave room for over 1,024 (a full batch form).
async fn worst_case(huge: u32) -> (tempfile::TempDir, Store, Vec<RequestAuth>) {
    let mut sql = String::new();
    let mut words = Vec::new();
    for id in 1..=huge {
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
    // 1,030 matching units (the batch form's worst case), each with its own box and word,
    // below the orphan block.
    for n in 0..1_030_u32 {
        // Id 900 is page 2's unit.
        let id = 71 + n + u32::from(n >= 829);
        let (x, y) = (2 + (n % 8) * 24, 2320 + (n / 8) * 4);
        let _ = write!(
            sql,
            "UPDATE source_units SET raw_text='m{id}',polygon='[[{x},{y}],[{r},{y}],[{r},{b}],[{x},{b}]]' WHERE id={id};",
            r = x + 22,
            b = y + 3
        );
        let (xf, yf) = (
            f32::from(u16::try_from(x).unwrap()),
            f32::from(u16::try_from(y).unwrap()),
        );
        words.push(word(
            &format!("m{id}"),
            [xf + 1.0, yf + 0.5, xf + 21.0, yf + 2.5],
        ));
    }
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
    // Nine huge cards saturate the card budget; three leave room for a full batch form.
    for huge in [9, 3] {
        let (_dir, store, auth) = worst_case(huge).await;
        let app = crate::auth::app(store).await.unwrap();
        for query in ["?show=all", "?show=attention", "?show=matching"] {
            let html = get(&app, &auth[0], query).await;
            let page = scraper::Html::parse_document(&html);
            let summary = text_of(&html, "[data-agreement-summary]");
            let matching: usize = summary.split(' ').next().unwrap().parse().unwrap();
            eprintln!(
                "worst-case huge={huge} {query}: {} bytes, {matching} matching",
                html.len()
            );
            // The batch form is rendered with every matching unit that fits its part.
            assert!(html.contains("id=\"agreement-batch\""), "{huge} {query}");
            if huge == 3 {
                assert!(matching >= 1_024, "{matching}");
                assert!(html.contains("Accept the first 1024 of the"), "{query}");
            } else {
                assert!(matching >= 400, "{matching}");
                // The compact matching list shows no card.
                assert!(query == "?show=matching" || html.contains("Correction form omitted"));
            }
            // The case is the worst one: overlay, orphans and diffs.
            assert!(
                page.select(&css("#review-overlay polygon[data-overlay-unit]"))
                    .count()
                    >= 1_900
            );
            assert!(html.contains("105 text-layer words in no unit are not drawn"));
            if query != "?show=matching" {
                assert_eq!(page.select(&css("[data-agreement-diff] li")).count(), 200);
            }
            assert!(
                html.len() <= 2 * 1024 * 1024,
                "{huge} {query}: {} bytes",
                html.len()
            );
        }
        if huge == 3 {
            continue;
        }
        // A refused save of the largest draft renders the same page with the draft on its
        // first card.
        let mut fields = reject(99, "all");
        fields[2].1 = "corrected".into();
        fields[3].1 = "&".repeat(20_000);
        fields[4].1 = "&".repeat(5_000);
        let response =
            document_at(&app, &auth[0], "/admin/materials/pages/1/units/10", fields).await;
        assert_ne!(response.status(), StatusCode::SEE_OTHER);
        let html = body(response).await;
        assert!(html.contains("data-agreement-diff"));
        assert!(
            html.len() <= 2 * 1024 * 1024,
            "refused: {} bytes",
            html.len()
        );
    }
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

// ---------------------------------------------------------------------------------------
// Slice 4: batch accept
// ---------------------------------------------------------------------------------------

const BATCH: &str = "/admin/materials/pages/1/batch";
const OWNER_TEXT: &str = "matches the PDF text layer word for word; checked against the page image";

/// The fields the page's own batch form carries, read from its HTML.
fn batch_fields(html: &str) -> Vec<(String, String)> {
    let page = scraper::Html::parse_document(html);
    let mut fields: Vec<(String, String)> = page
        .select(&css("#agreement-batch input[type=hidden]"))
        .map(|e| {
            (
                e.value().attr("name").unwrap().to_owned(),
                e.value().attr("value").unwrap().to_owned(),
            )
        })
        .collect();
    fields.push(("rationale".to_owned(), OWNER_TEXT.to_owned()));
    fields
}

fn set_field(fields: &mut [(String, String)], name: &str, value: &str) {
    fields.iter_mut().find(|f| f.0 == name).unwrap().1 = value.to_owned();
}

async fn batch_form(app: &axum::Router, auth: &RequestAuth) -> Vec<(String, String)> {
    batch_fields(&get(app, auth, "").await)
}

type BatchRows = Vec<(i64, String, i64, String, String)>;

/// Every unit row with its text and every review event, for before/after comparisons.
async fn batch_state(store: &Store) -> (BatchRows, BatchRows) {
    store
        .run(|db| {
            let units = db
                .prepare("SELECT id,state,revision,coalesce(raw_text,''),correction FROM source_units ORDER BY id")?
                .query_map([], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                })?
                .collect::<Result<_, _>>()?;
            let events = db
                .prepare("SELECT unit_id,state,previous_revision,coalesce(operation_id,''),rationale FROM source_review_events ORDER BY id")?
                .query_map([], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                })?
                .collect::<Result<_, _>>()?;
            Ok((units, events))
        })
        .await
        .unwrap()
}

async fn word_bytes(store: &Store) -> Vec<u8> {
    store
        .run(|db| {
            Ok(db.query_row(
                "SELECT words FROM source_page_words WHERE page_id=1",
                [],
                |r| r.get::<_, Vec<u8>>(0),
            )?)
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn the_batch_form_lists_the_matching_units_with_the_fixed_prefix() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = standard().await;
    let app = crate::auth::app(store).await.unwrap();
    let html = get(&app, &auth[0], "").await;
    assert_eq!(
        text_of(&html, "#agreement-batch h3"),
        "Accept the 2 matching units"
    );
    assert!(text_of(&html, "[data-batch-units]").starts_with("Units 1, 5."));
    assert_eq!(
        text_of(&html, "[data-batch-prefix]"),
        "agreement-v1; independent:"
    );
    let page = scraper::Html::parse_document(&html);
    // The prefix is fixed text, not a field; the textarea is prefilled.
    assert_eq!(
        page.select(&css("#agreement-batch [name=rationale]"))
            .count(),
        1
    );
    assert_eq!(text_of(&html, "#agreement-batch textarea"), OWNER_TEXT);
    let fields = batch_fields(&html);
    let names: Vec<&str> = fields.iter().map(|f| f.0.as_str()).collect();
    assert_eq!(names, ["csrf", "revision", "words", "units", "rationale"]);
    assert_eq!(fields[3].1, "1:1,5:1");
}

#[tokio::test]
async fn batch_accept_records_one_operation_with_the_full_rationale_and_changes_no_text() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = standard().await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let fields = batch_form(&app, &auth[0]).await;
    let words = word_bytes(&store).await;
    let (units_before, events_before) = batch_state(&store).await;
    assert_eq!(events_before.len(), 0);
    let response = document_at(&app, &auth[0], BATCH, fields).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response.headers()["location"].to_str().unwrap().to_owned();
    assert_eq!(location, "/admin/materials/pages/1?batch=2#agreement-bar");
    let (units_after, events) = batch_state(&store).await;
    // Raw text and corrections are the same bytes; only state and revision of units 1 and 5.
    for (before, after) in units_before.iter().zip(&units_after) {
        assert_eq!(
            (before.0, &before.3, &before.4),
            (after.0, &after.3, &after.4)
        );
        if before.0 == 1 || before.0 == 5 {
            assert_eq!((after.1.as_str(), after.2), ("accepted", 2));
        } else {
            assert_eq!(before, after);
        }
    }
    assert_eq!(word_bytes(&store).await, words);
    // One operation id, one event per unit, each with the full rationale and a sidecar.
    assert_eq!(events.len(), 2);
    let expected = format!("agreement-v1; independent: {OWNER_TEXT}");
    assert!(events.iter().all(|e| e.1 == "accepted" && e.4 == expected));
    assert!(events[0].3.starts_with("batch-") && events[0].3 == events[1].3);
    assert_eq!(events.iter().map(|e| e.0).collect::<Vec<_>>(), vec![1, 5]);
    let sidecars: i64 = store
        .run(|db| {
            Ok(db.query_row(
                "SELECT count(*) FROM source_web_events w JOIN source_review_events e ON e.id=w.event_id WHERE e.operation_id IS NOT NULL",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(sidecars, 2);
    // The page completes only by its own action: it is not complete, and shows the notice.
    let html = get(&app, &auth[0], "?batch=2").await;
    assert_eq!(
        text_of(&html, "[data-batch-notice]"),
        "Accepted 2 units that match the PDF text layer."
    );
    assert!(
        !html.contains("id=\"agreement-batch\""),
        "nothing left to accept"
    );
    // A hand-made notice link that claims another count is not announced.
    let forged = get(&app, &auth[0], "?batch=7").await;
    assert!(!forged.contains("data-batch-notice"));
}

#[tokio::test]
async fn a_refused_batch_stays_unsaved_and_keeps_the_owners_text() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = standard().await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let before = batch_state(&store).await;
    let good = batch_form(&app, &auth[0]).await;
    // A stale page revision.
    let mut stale_page = good.clone();
    set_field(&mut stale_page, "revision", "99");
    set_field(&mut stale_page, "rationale", "my own words <b>");
    let response = document_at(&app, &auth[0], BATCH, stale_page).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = body(response).await;
    assert!(text_of(&html, "[data-batch-error]").contains("The page changed"));
    assert_eq!(
        text_of(&html, "#agreement-batch textarea"),
        "my own words <b>"
    );
    assert!(html.contains("my own words &#60;b&#62;") || html.contains("my own words &lt;b&gt;"));
    // The re-rendered form is current: its own fields post.
    let renewed = batch_fields(&html);
    assert_eq!(renewed[1].1, good[1].1);
    // A stale unit revision, a unit from another page, a duplicate, garbage and too many.
    let units = |value: &str| {
        let mut f = good.clone();
        set_field(&mut f, "units", value);
        f
    };
    for (value, needle) in [
        ("1:9,5:1", "A unit changed"),
        ("1:1,900:1", "not on this page"),
        ("1:1,1:1", "twice"),
        ("1:1,2:1", "no longer matches"),
        ("1:1,4:1", "no longer matches"),
        ("", "incomplete"),
        ("1;1", "incomplete"),
        ("1:1,5", "incomplete"),
    ] {
        let response = document_at(&app, &auth[0], BATCH, units(value)).await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{value}"
        );
        let html = body(response).await;
        assert!(
            text_of(&html, "[data-batch-error]").contains(needle),
            "{value}: {}",
            text_of(&html, "[data-batch-error]")
        );
    }
    let too_many = (1..=1025)
        .map(|i| format!("{i}:1"))
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(
        document_at(&app, &auth[0], BATCH, units(&too_many))
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // An empty Owner rationale is refused, the prefix alone is no rationale.
    let mut empty = good.clone();
    set_field(&mut empty, "rationale", "  ");
    assert_eq!(
        document_at(&app, &auth[0], BATCH, empty).await.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(batch_state(&store).await, before, "nothing was written");
}

#[tokio::test]
async fn a_unit_reviewed_meanwhile_or_with_a_new_suggestion_refuses_the_whole_batch() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = standard().await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let good = batch_form(&app, &auth[0]).await;
    // A suggestion opened on unit 5 after the form rendered: the page revision is unchanged,
    // the recomputed verdict is not `agrees`.
    let id = suggestion_fixture(&store, "correction", suggestion_unit(5)).await;
    let before = batch_state(&store).await;
    let response = document_at(&app, &auth[0], BATCH, good.clone()).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(text_of(&body(response).await, "[data-batch-error]").contains("no longer matches"));
    assert_eq!(batch_state(&store).await, before);
    store
        .material_dismiss(auth[0].clone(), auth[0].csrf_fixture(), id, String::new())
        .await
        .unwrap();
    // Unit 1 reviewed meanwhile: the page revision moved.
    store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            1,
            crate::sources::UnitReview {
                revision: 1,
                state: "rejected".into(),
                correction: String::new(),
                rationale: "not evidence".into(),
                actor: String::new(),
            },
        )
        .await
        .unwrap();
    let before = batch_state(&store).await;
    let response = document_at(&app, &auth[0], BATCH, good).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        batch_state(&store).await,
        before,
        "unit 5 was not accepted either"
    );
}

#[tokio::test]
async fn a_word_row_that_changed_size_refuses_the_batch() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = standard().await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let good = batch_form(&app, &auth[0]).await;
    let mut more = base_words();
    more.push(word("extra", [100.0, 200.0, 120.0, 210.0]));
    let bytes = crate::sources::page_words::encode(&more, 200.0, 300.0).unwrap();
    store
        .run(move |db| {
            // The words table is append only: this scratch database drops its guards.
            db.execute_batch("DROP TRIGGER source_page_words_update")?;
            db.execute(
                "UPDATE source_page_words SET words=?1 WHERE page_id=1",
                [bytes],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let before = batch_state(&store).await;
    let response = document_at(&app, &auth[0], BATCH, good).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(text_of(&body(response).await, "[data-batch-error]").contains("words changed"));
    assert_eq!(batch_state(&store).await, before);
}

#[tokio::test]
async fn a_forged_prefix_is_only_the_owners_text_after_the_servers_prefix() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = standard().await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let mut fields = batch_form(&app, &auth[0]).await;
    set_field(
        &mut fields,
        "rationale",
        "agreement-v1; independent: and also agreement-v1; independent: ok",
    );
    let response = document_at(&app, &auth[0], BATCH, fields).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let (_, events) = batch_state(&store).await;
    assert_eq!(events.len(), 2);
    for event in events {
        assert_eq!(
            event.4,
            "agreement-v1; independent: agreement-v1; independent: and also agreement-v1; independent: ok"
        );
    }
    // A prefix field in the request is no field of the form at all.
    let (_d2, store2, auth2) = standard().await;
    let app2 = crate::auth::app(store2.clone()).await.unwrap();
    let mut forged = batch_form(&app2, &auth2[0]).await;
    forged.push(("prefix".to_owned(), "other".to_owned()));
    let response = document_at(&app2, &auth2[0], BATCH, forged).await;
    assert_ne!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(batch_state(&store2).await.1.len(), 0);
}

#[tokio::test]
async fn csrf_and_the_owner_guard_the_batch() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = standard().await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let good = batch_form(&app, &auth[0]).await;
    let before = batch_state(&store).await;
    let mut bad = good.clone();
    set_field(&mut bad, "csrf", &"b".repeat(64));
    assert_eq!(
        document_at(&app, &auth[0], BATCH, bad).await.status(),
        StatusCode::FORBIDDEN
    );
    let missing: Vec<_> = good.iter().filter(|f| f.0 != "csrf").cloned().collect();
    let status = document_at(&app, &auth[0], BATCH, missing).await.status();
    assert!(status.is_client_error(), "{status}");
    // A non-owner is refused like every material route.
    assert_eq!(
        document_at(&app, &auth[1], BATCH, good.clone())
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    // The store refuses a stale admission and a non-owner on its own.
    let words_len = i64::try_from(word_bytes(&store).await.len()).unwrap();
    let request = || crate::sources::agreement_view::BatchAccept {
        page_revision: 1,
        words_len,
        units: vec![(1, 1)],
        text: "ok".into(),
    };
    assert!(
        store
            .material_batch_accept_agreeing(
                auth[0].clone(),
                auth[0].stale_csrf_fixture(),
                1,
                request()
            )
            .await
            .is_err()
    );
    assert!(
        store
            .material_batch_accept_agreeing(auth[1].clone(), auth[1].csrf_fixture(), 1, request())
            .await
            .is_err()
    );
    assert_eq!(batch_state(&store).await, before);
}

#[tokio::test]
async fn the_store_checks_everything_a_batch_rests_on_by_itself() {
    use crate::sources::agreement_view::BatchAccept;
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = standard().await;
    let words_len = i64::try_from(word_bytes(&store).await.len()).unwrap();
    let request = |revision: i64, words: i64, units: Vec<(i64, i64)>| BatchAccept {
        page_revision: revision,
        words_len: words,
        units,
        text: "ok".into(),
    };
    let before = batch_state(&store).await;
    let run = |r: BatchAccept| {
        let store = store.clone();
        let a = auth[0].clone();
        async move {
            store
                .material_batch_accept_agreeing(a.clone(), a.csrf_fixture(), 1, r)
                .await
        }
    };
    // Stale page revision, words size, unit revision, a unit of another page, a duplicate,
    // an empty set.
    for bad in [
        request(2, words_len, vec![(1, 1)]),
        request(1, words_len + 1, vec![(1, 1)]),
        request(1, words_len, vec![(1, 2)]),
        request(1, words_len, vec![(900, 1)]),
        request(1, words_len, vec![(1, 1), (1, 1)]),
        request(1, words_len, vec![]),
        request(1, words_len, vec![(1, 1), (5, 1), (404, 1)]),
    ] {
        assert!(run(bad).await.is_err());
    }
    // A suggestion opened after the agreement was computed: the store refuses on its own.
    suggestion_fixture(&store, "correction", suggestion_unit(5)).await;
    assert!(
        run(request(1, words_len, vec![(1, 1), (5, 1)]))
            .await
            .is_err()
    );
    assert_eq!(batch_state(&store).await, before);
    // A unit reviewed since: its state is no longer unreviewed.
    store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            2,
            crate::sources::UnitReview {
                revision: 1,
                state: "rejected".into(),
                correction: String::new(),
                rationale: "no".into(),
                actor: String::new(),
            },
        )
        .await
        .unwrap();
    let revision = store
        .run(|db| {
            Ok(
                db.query_row("SELECT revision FROM source_pages WHERE id=1", [], |r| {
                    r.get::<_, i64>(0)
                })?,
            )
        })
        .await
        .unwrap();
    let after_review = batch_state(&store).await;
    assert!(
        run(request(revision, words_len, vec![(2, 2)]))
            .await
            .is_err()
    );
    assert_eq!(batch_state(&store).await, after_review);
    // The same request on a clean unit passes and is one operation.
    let ok = run(request(revision, words_len, vec![(1, 1)]))
        .await
        .unwrap();
    assert_eq!(ok.len(), 1);
}

#[tokio::test]
async fn the_form_is_not_offered_for_other_readings_or_when_nothing_matches() {
    let _lock = crate::auth::test_resource_lock().await;
    let derived = serde_json::json!({"name":"mimik-route-a","version":"1","settings":{}});
    for extractor in [same_layer(), derived, serde_json::json!({"name":"other"})] {
        let (_dir, store, auth) = fixture_with(extractor, base_words(), "").await;
        let app = crate::auth::app(store.clone()).await.unwrap();
        for query in ["", "?show=all", "?show=matching"] {
            let html = get(&app, &auth[0], query).await;
            assert!(!html.contains("agreement-batch"), "{query}");
        }
        // A forged post is refused by the recomputation, and writes nothing.
        let mut fields = vec![
            ("csrf".to_owned(), "a".repeat(64)),
            ("revision".to_owned(), "1".to_owned()),
            ("words".to_owned(), "0".to_owned()),
            ("units".to_owned(), "1:1,5:1".to_owned()),
            ("rationale".to_owned(), OWNER_TEXT.to_owned()),
        ];
        fields[2].1 = i64::try_from(word_bytes(&store).await.len())
            .unwrap()
            .to_string();
        let response = document_at(&app, &auth[0], BATCH, fields).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(batch_state(&store).await.1.len(), 0);
    }
    // Independent, but no unit matches.
    let (_dir, store, auth) = fixture_with(
        independent(),
        vec![word("nothing", [11.0, 11.0, 30.0, 21.0])],
        "",
    )
    .await;
    let app = crate::auth::app(store).await.unwrap();
    assert!(!get(&app, &auth[0], "").await.contains("agreement-batch"));
    // No words at all.
    let (_d2, store2, auth2) = fixture_with(independent(), Vec::new(), "").await;
    let app2 = crate::auth::app(store2).await.unwrap();
    assert!(!get(&app2, &auth2[0], "").await.contains("agreement-batch"));
}

#[tokio::test]
async fn more_than_the_atomic_bound_is_accepted_in_parts() {
    let _lock = crate::auth::test_resource_lock().await;
    // 1,100 matching units on a tall page: one text-layer word inside each.
    let mut sql = String::new();
    let mut words = Vec::new();
    for i in 0..1100_u32 {
        let id = 1000 + i;
        let (x, y) = (2 + (i % 8) * 24, 400 + (i / 8) * 4);
        let _ = write!(
            sql,
            "INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES({id},1,'unit-{id}','Text','[[{x},{y}],[{r},{y}],[{r},{b}],[{x},{b}]]','<p>m</p>','m{id}');",
            r = x + 22,
            b = y + 3
        );
        let (xf, yf) = (
            f32::from(u16::try_from(x).unwrap()),
            f32::from(u16::try_from(y).unwrap()),
        );
        words.push(word(
            &format!("m{id}"),
            [xf + 1.0, yf + 0.5, xf + 21.0, yf + 2.5],
        ));
    }
    let (_dir, store, auth) =
        fixture_sized(independent(), words, Box::leak(sql.into_boxed_str()), 1_000).await;
    // The raw rows get their search text as a legacy upgrade would.
    store
        .run(|db| {
            crate::sources::search::ensure(db)?;
            Ok(())
        })
        .await
        .unwrap();
    // Units 1-70 of the fixture have no word here: they are not matching.
    let app = crate::auth::app(store.clone()).await.unwrap();
    let response = router_get(&app, &auth[0], "/admin/materials/pages/1").await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body(response).await;
    assert!(html.len() < 2 * 1024 * 1024, "{}", html.len());
    assert_eq!(
        text_of(&html, "#agreement-batch h3"),
        "Accept the first 1024 of the 1100 matching units",
        "{}",
        text_of(&html, "#agreement-bar")
    );
    assert!(text_of(&html, "[data-batch-units]").contains("The other 76 follow in the next parts"));
    let first = batch_fields(&html);
    assert_eq!(first[3].1.split(',').count(), 1024);
    let response = document_at(&app, &auth[0], BATCH, first).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let (_, events) = batch_state(&store).await;
    assert_eq!(events.len(), 1024);
    assert_eq!(
        events
            .iter()
            .map(|e| e.3.clone())
            .collect::<std::collections::HashSet<_>>()
            .len(),
        1
    );
    // The rest follow.
    let html = get(&app, &auth[0], "").await;
    assert_eq!(
        text_of(&html, "#agreement-batch h3"),
        "Accept the 76 matching units"
    );
    let second = batch_fields(&html);
    assert_eq!(
        document_at(&app, &auth[0], BATCH, second).await.status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(batch_state(&store).await.1.len(), 1100);
}

/// The batch form of a page whose ids are as wide as an id can be.
#[tokio::test]
async fn the_batch_form_is_bounded_whatever_the_unit_ids() {
    let _lock = crate::auth::test_resource_lock().await;
    let mut sql = String::new();
    let mut words = Vec::new();
    for i in 0..1100_u32 {
        let id = 9_000_000_000_000_000_000_u64 + u64::from(i);
        let (x, y) = (2 + (i % 8) * 24, 400 + (i / 8) * 4);
        let _ = write!(
            sql,
            "INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text,revision) VALUES({id},1,'unit-{id}','Text','[[{x},{y}],[{r},{y}],[{r},{b}],[{x},{b}]]','<p>m</p>','m{i}',9000000000000000000);",
            r = x + 22,
            b = y + 3
        );
        let (xf, yf) = (
            f32::from(u16::try_from(x).unwrap()),
            f32::from(u16::try_from(y).unwrap()),
        );
        words.push(word(
            &format!("m{i}"),
            [xf + 1.0, yf + 0.5, xf + 21.0, yf + 2.5],
        ));
    }
    let (_dir, store, auth) =
        fixture_sized(independent(), words, Box::leak(sql.into_boxed_str()), 1_000).await;
    let app = crate::auth::app(store).await.unwrap();
    let html = get(&app, &auth[0], "").await;
    let form = scraper::Html::parse_document(&html)
        .select(&css("#agreement-batch"))
        .next()
        .expect("the form")
        .html();
    // 1,024 pairs of two 19-digit numbers are 43 KiB; the visible list is capped.
    eprintln!(
        "form {} bytes: {}",
        form.len(),
        text_of(&html, "#agreement-batch h3")
    );
    assert!(form.len() <= 50 * 1024, "{} bytes", form.len());
    assert!(text_of(&html, "[data-batch-units]").contains("more in this form"));
}

#[tokio::test]
async fn a_refused_batch_echoes_at_most_5000_characters_of_the_rationale() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = worst_case(3).await;
    let app = crate::auth::app(store).await.unwrap();
    let mut fields = batch_form(&app, &auth[0]).await;
    set_field(&mut fields, "units", "71:99");
    set_field(&mut fields, "rationale", &"<".repeat(100_000));
    let response = document_at(&app, &auth[0], BATCH, fields).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = body(response).await;
    let echoed = text_of(&html, "#agreement-batch textarea");
    assert!(echoed.chars().count() <= 5_000, "{}", echoed.len());
    assert!(html.len() <= 2 * 1024 * 1024, "{} bytes", html.len());
}

#[tokio::test]
async fn the_batch_post_recomputes_and_leaves_the_memo_alone() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = standard().await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    // The view memoises the pass, which says unit 5 agrees.
    let good = batch_form(&app, &auth[0]).await;
    let computed = store.2.passes_computed();
    assert_eq!(computed, 1);
    // The stored text changes without a revision bump: the memo is now wrong.
    store
        .run(|db| {
            // The finalized units are immutable: this scratch database drops the guard.
            db.execute_batch("DROP TRIGGER units_finalized_update")?;
            db.execute(
                "UPDATE source_units SET raw_text='zeta CHANGED' WHERE id=5",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let before = batch_state(&store).await;
    let response = document_at(&app, &auth[0], BATCH, good).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(batch_state(&store).await, before);
    // Nothing was read from the memo, and nothing was written to it.
    assert_eq!(store.2.passes_computed(), computed);
}

#[tokio::test]
async fn a_page_level_suggestion_opened_after_the_form_refuses_in_the_store() {
    use crate::sources::agreement_view::BatchAccept;
    let _lock = crate::auth::test_resource_lock().await;
    for kind in ["note", "missing_text"] {
        let (_dir, store, auth) = standard().await;
        let words_len = i64::try_from(word_bytes(&store).await.len()).unwrap();
        let target = if kind == "note" {
            crate::sources::SuggestionTarget::Page {
                page: 1,
                block_id: None,
            }
        } else {
            crate::sources::SuggestionTarget::Region {
                page: 1,
                polygon: [[10.0, 200.0], [20.0, 200.0], [20.0, 210.0], [10.0, 210.0]],
            }
        };
        suggestion_fixture(&store, kind, target).await;
        let before = batch_state(&store).await;
        let result = store
            .material_batch_accept_agreeing(
                auth[0].clone(),
                auth[0].csrf_fixture(),
                1,
                BatchAccept {
                    page_revision: 1,
                    words_len,
                    units: vec![(1, 1)],
                    text: "ok".into(),
                },
            )
            .await;
        assert!(result.is_err(), "{kind}");
        assert_eq!(batch_state(&store).await, before, "{kind}");
    }
}

#[tokio::test]
async fn a_storage_error_in_the_batch_recompute_is_not_a_form_refusal() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = standard().await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let good = batch_form(&app, &auth[0]).await;
    store
        .run(|db| {
            db.execute_batch("ALTER TABLE source_page_words RENAME TO words_gone")?;
            Ok(())
        })
        .await
        .unwrap();
    let response = document_at(&app, &auth[0], BATCH, good).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn a_rationale_starting_with_a_newline_survives_a_refusal() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = standard().await;
    let app = crate::auth::app(store).await.unwrap();
    let mut fields = batch_form(&app, &auth[0]).await;
    set_field(&mut fields, "revision", "99");
    set_field(&mut fields, "rationale", "\nsecond line");
    let html = body(document_at(&app, &auth[0], BATCH, fields).await).await;
    let page = scraper::Html::parse_document(&html);
    let area = page
        .select(&css("#agreement-batch textarea"))
        .next()
        .unwrap();
    assert_eq!(area.text().collect::<String>(), "\nsecond line");
}
