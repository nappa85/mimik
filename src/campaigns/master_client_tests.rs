//! Master view slice (c1): the progressive-enhancement page and its two modules under the
//! strict sheet policy, and the publication-level races of the opened acknowledgement the
//! modules depend on (CAMP-14, ROAD-8).
use super::super::unit_tests::unit_form;
use super::*;
use axum::http::header;

/// The policy of every sheet page, the Master workspace included.
const STRICT: &str = "default-src 'self'; script-src 'self'; style-src 'self'; \
    object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'; \
    require-trusted-types-for 'script'; trusted-types 'none'";

const LIVE_TAG: &str = r#"<script type="module" src="/assets/master-live.js"></script>"#;
const SEEN_TAG: &str = r#"<script type="module" src="/assets/sheet-seen.js"></script>"#;

async fn page(
    setup: &Setup,
    who: usize,
    path: &str,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let response = router_request(&setup.app, &setup.auth[who], path, None).await;
    let (status, headers) = (response.status(), response.headers().clone());
    (
        status,
        headers,
        super::super::formula_tests::body(response).await,
    )
}

fn csp(headers: &axum::http::HeaderMap) -> &str {
    headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap()
}

#[tokio::test]
async fn the_workspace_runs_one_module_under_the_strict_policy_and_degrades_to_plain_html() {
    let _locks = lock().await;
    let setup = world().await;
    two_revisions(&setup).await;
    assert_eq!(create_as(&setup, 3, true).await, StatusCode::SEE_OTHER);
    let (status, headers, html) = page(&setup, 1, "/campaigns/1/master").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(csp(&headers), STRICT);
    // One module, no inline script, style or handler: the page works with the script gone.
    assert_eq!(html.matches("<script").count(), 1, "{html}");
    assert!(html.contains(LIVE_TAG), "{html}");
    assert!(
        !html.contains(" style=") && !html.contains(" onclick="),
        "{html}"
    );
    let document = scraper::Html::parse_document(&html);
    let one = |selector: &str| {
        document
            .select(&scraper::Selector::parse(selector).unwrap())
            .collect::<Vec<_>>()
    };
    let root = one("div#master-live");
    assert_eq!(root.len(), 1);
    for (name, value) in [
        ("data-campaign", "1"),
        ("data-players-after", "0"),
        ("data-npcs-after", "0"),
    ] {
        assert_eq!(root[0].value().attr(name), Some(value), "{name}");
    }
    // A polite status region that says what is true without the module, the Reload link and
    // keyed rows with their Open links.
    let status = one("p#live-status[role=status]");
    assert_eq!(status.len(), 1);
    let text = status[0].text().collect::<String>();
    assert!(
        text.contains("JavaScript") && text.contains("Reload"),
        "{text}"
    );
    assert!(
        status[0].value().attr("aria-live").is_none(),
        "role=status is already polite"
    );
    assert_eq!(one("a#reload").len(), 1);
    assert_eq!(one("div#players-rows > article[data-id] > a.open").len(), 1);
    assert_eq!(one("div#npcs-rows > article[data-id] > a.open").len(), 1);
    // Another page of the cursors keeps its own cursors for the module.
    let (_, _, paged) = page(
        &setup,
        1,
        "/campaigns/1/master?players_after=1&npcs_after=0",
    )
    .await;
    assert!(paged.contains("data-players-after=\"1\""), "{paged}");
    assert!(paged.contains("No player sheets on this page."), "{paged}");
    // Everyone else gets the uniform refusal with the default policy and no module.
    for who in [2, 4, 0] {
        let (status, headers, body) = page(&setup, who, "/campaigns/1/master").await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{who}");
        assert_ne!(csp(&headers), STRICT, "{who}");
        assert!(!body.contains("master-live"), "{who}");
        nothing_leaks(&body);
    }
}

#[tokio::test]
async fn a_masters_sheet_page_loads_the_acknowledgement_module_only_while_the_form_is_offered() {
    let _locks = lock().await;
    let setup = world().await;
    two_revisions(&setup).await;
    assert_eq!(create_as(&setup, 2, false).await, StatusCode::SEE_OTHER);
    // The offer: the form carries the exact rendered revision, and the module is loaded.
    let (status, headers, html) = page(&setup, 3, "/private-sheets/1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(csp(&headers), STRICT);
    assert!(
        html.contains("id=\"seen-form\"") && html.contains(SEEN_TAG),
        "{html}"
    );
    assert!(html.contains("name=\"revision\" value=\"2\""), "{html}");
    // A refused draft (stale 409) before B acknowledged anything never offers it.
    let (status, stale) = send(
        &setup,
        3,
        "/private-sheets/1",
        Some(form(1, &[("value-hp", "5")], &[row("r1-1", "1")])),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{stale}");
    assert!(!stale.contains(SEEN_TAG), "{stale}");
    // Opened: nothing to submit, no module. A history page, the custodian's own page
    // (player), a refused draft and the unchanged re-render never offer it.
    assert_eq!(
        location(&setup, 3, "/private-sheets/1/seen", seen_form("2"))
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    let (_, _, opened) = page(&setup, 3, "/private-sheets/1").await;
    assert!(
        opened.contains("Opened (revision 2)") && !opened.contains(SEEN_TAG),
        "{opened}"
    );
    let (_, _, history) = page(&setup, 3, "/private-sheets/1/history/1").await;
    assert!(!history.contains(SEEN_TAG), "{history}");
    let (_, _, own) = page(&setup, 2, "/private-sheets/2").await;
    assert!(
        !own.contains(SEEN_TAG) && !own.contains("seen-form"),
        "{own}"
    );
}

/// The modules are static (the same bytes for everyone) and free of every sink the
/// strict policy would refuse (they must not need it relaxed).
#[tokio::test]
async fn the_modules_are_static_scripts_without_html_sinks_eval_or_style() {
    let _locks = lock().await;
    let setup = world().await;
    let style = regex::Regex::new(r"\bstyle\b|\.style\b").unwrap();
    for (path, source) in [
        (
            "/assets/master-live.js",
            include_str!("../../assets/master-live.js"),
        ),
        (
            "/assets/sheet-seen.js",
            include_str!("../../assets/sheet-seen.js"),
        ),
    ] {
        let response = router_request(&setup.app, &setup.auth[1], path, None).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/javascript; charset=utf-8"
        );
        assert_eq!(super::super::formula_tests::body(response).await, source);
        for forbidden in [
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            "document.write",
            "DOMParser",
            "createContextualFragment",
            "srcdoc",
            "cssText",
            "eval(",
            "Function(",
            "setTimeout('",
            "setTimeout(\"",
            "javascript:",
            "trustedTypes",
            "XMLHttpRequest",
            "WebSocket",
            "importScripts",
            "<script",
            "localStorage",
            "sessionStorage",
        ] {
            assert!(!source.contains(forbidden), "{path} uses {forbidden}");
        }
        assert!(!style.is_match(source), "{path} names style");
        // No hand-written event-stream parser: the browser's EventSource only.
        assert!(
            !source.contains("text/event-stream") && !source.contains("getReader"),
            "{path}"
        );
        // Every URL it calls is same-origin and built from digits.
        assert!(
            !source.contains("http://") && !source.contains("https://"),
            "{path}"
        );
    }
    let live = include_str!("../../assets/master-live.js");
    assert!(live.contains("new EventSource("));
    assert!(live.contains("RETRY_MIN = 3000") && live.contains("RETRY_MAX = 30000"));
    let anonymous = {
        use tower::ServiceExt;
        setup
            .app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/assets/master-live.js")
                    .header("host", "127.0.0.1:3000")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    };
    // Like the other assets they are the same bytes for everyone (no per-account content).
    assert_eq!(super::super::formula_tests::body(anonymous).await, live);
}

/// A stale open racing a newer revision stays "Changed", and overlapping opens of several
/// revisions end at the highest one with exactly one change published to that Master alone.
#[tokio::test]
async fn a_stale_open_racing_a_newer_revision_stays_changed_and_overlapping_opens_end_highest() {
    let _locks = lock().await;
    let setup = world().await;
    two_revisions(&setup).await;
    // B renders revision 2 (the form it will submit later) ...
    let rendered = get(&setup, 3, "/private-sheets/1").await;
    assert!(
        rendered.contains("name=\"revision\" value=\"2\""),
        "{rendered}"
    );
    // ... and A saves revision 3 before B's open arrives.
    let (status, html) = send(
        &setup,
        1,
        "/private-sheets/1",
        Some(form(2, &[("value-hp", "40")], &[row("r1-1", "1")])),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
    let mut feed_a = setup.store.feed(1, 2);
    let mut feed_b = setup.store.feed(1, 4);
    assert_eq!(
        location(&setup, 3, "/private-sheets/1/seen", seen_form("2"))
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(seen_of(&setup, 4, 1).await, Some(2));
    let list = get(&setup, 3, "/campaigns/1/master").await;
    assert!(
        section_text(&list, "players-heading").contains("Changed"),
        "{list}"
    );
    assert_eq!(feed_b.recv().await.unwrap(), super::hub::seen_hint(4, 1));
    // Overlapping opens of 3 and 2 (any interleaving): the mark ends at 3 and one hint says so.
    let (third, second) = tokio::join!(
        location(&setup, 3, "/private-sheets/1/seen", seen_form("3")),
        location(&setup, 3, "/private-sheets/1/seen", seen_form("2")),
    );
    assert_eq!(
        (third.0, second.0),
        (StatusCode::SEE_OTHER, StatusCode::SEE_OTHER)
    );
    assert_eq!(seen_of(&setup, 4, 1).await, Some(3));
    assert_eq!(feed_b.recv().await.unwrap(), super::hub::seen_hint(4, 1));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), feed_b.recv())
            .await
            .is_err(),
        "the second of the overlapping opens published nothing"
    );
    let list = get(&setup, 3, "/campaigns/1/master").await;
    assert!(
        !section_text(&list, "players-heading").contains("Changed"),
        "{list}"
    );
    // A's mark and feed are untouched by B's opens.
    assert_eq!(seen_of(&setup, 2, 1).await, Some(3));
    let hints = tokio::time::timeout(std::time::Duration::from_millis(100), feed_a.recv()).await;
    assert!(hints.is_err(), "B's opens reached A: {hints:?}");
}

/// A Master's own acknowledgement reaches every tab of that Master (their feeds and their
/// snapshot) and no other Master, player or outsider.
#[tokio::test]
async fn an_acknowledgement_reaches_every_tab_of_the_same_master_and_no_one_else() {
    let _locks = lock().await;
    let setup = world().await;
    two_revisions(&setup).await;
    // A saves revision 3 (A's own mark follows to 3); B will acknowledge revision 2 only.
    let (status, html) = send(
        &setup,
        1,
        "/private-sheets/1",
        Some(form(2, &[("value-hp", "41")], &[row("r1-1", "1")])),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
    let mut tab_one = setup.store.feed(1, 4);
    let mut tab_two = setup.store.feed(1, 4);
    let mut other_master = setup.store.feed(1, 2);
    assert_eq!(
        location(&setup, 3, "/private-sheets/1/seen", seen_form("2"))
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    for tab in [&mut tab_one, &mut tab_two] {
        assert_eq!(tab.recv().await.unwrap(), super::hub::seen_hint(4, 1));
    }
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), other_master.recv())
            .await
            .is_err()
    );
    // Both tabs' next projection carries the same mark: the snapshot shows it for B only.
    let (_, _, as_b) = super::hub::snapshot_as(&setup, 3, &[]).await;
    let (_, _, as_a) = super::hub::snapshot_as(&setup, 1, &[]).await;
    let seen_of_row = |body: &str| {
        let value: Value = serde_json::from_str(body).unwrap();
        value["players"]["rows"][0]["seen"].clone()
    };
    assert_eq!(seen_of_row(&as_b), json!("2"), "B sees B's own mark");
    assert_eq!(
        seen_of_row(&as_a),
        json!("3"),
        "A sees A's own mark, not B's"
    );
    // A third Master-less account gets no snapshot at all.
    assert_eq!(
        super::hub::snapshot_as(&setup, 2, &[]).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        super::hub::snapshot_as(&setup, 4, &[]).await.0,
        StatusCode::FORBIDDEN
    );
}

/// A JavaScript unit save (the module's JSON route) after a refused or rebased answer never
/// moves another Master's mark, and moves its own only by owner decision 6.
#[tokio::test]
async fn a_json_unit_save_after_a_refusal_or_rebase_never_acknowledges_another_masters_revision() {
    let _locks = lock().await;
    let setup = world().await;
    two_revisions(&setup).await;
    // Both Masters acknowledged revision 2 (A by their own save, B by opening it).
    assert_eq!(
        location(&setup, 3, "/private-sheets/1/seen", seen_form("2"))
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    let html_a = get(&setup, 1, "/private-sheets/1").await;
    let html_b = get(&setup, 3, "/private-sheets/1").await;
    // A's JSON unit save writes revision 3: A's mark follows (it had seen the live revision),
    // B's stays at 2.
    let (path, pairs) = unit_form(&html_a, "v0", Some("51"));
    let (status, answer) = json_post(&setup, 1, &path, pairs).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(saved(&setup, 1).await.0, 3);
    assert_eq!(
        (seen_of(&setup, 2, 1).await, seen_of(&setup, 4, 1).await),
        (Some(3), Some(2))
    );
    // B's save from the page rendered at revision 2: the shared save defaults to
    // compare-and-set, so it is refused and acknowledges nothing for anyone.
    let (path, pairs) = unit_form(&html_b, "v0", Some("52"));
    let (status, refused) = json_post(&setup, 3, &path, pairs).await;
    assert!(status.is_client_error(), "{status}: {refused}");
    assert_eq!(saved(&setup, 1).await.0, 3, "nothing was written");
    assert_eq!(
        (seen_of(&setup, 2, 1).await, seen_of(&setup, 4, 1).await),
        (Some(3), Some(2))
    );
    // B reloads and saves the live revision without having acknowledged it: revision 4 is
    // written, B's mark stays 2 and A's stays 3 (neither acknowledged revision 4).
    let html_b = get(&setup, 3, "/private-sheets/1").await;
    let (path, pairs) = unit_form(&html_b, "v0", Some("53"));
    let (status, answer) = json_post(&setup, 3, &path, pairs).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(saved(&setup, 1).await.0, 4);
    assert_eq!(
        (seen_of(&setup, 2, 1).await, seen_of(&setup, 4, 1).await),
        (Some(3), Some(2))
    );
    // The acknowledgement module posts the page's own form: its revision is the rendered one.
    let rendered = get(&setup, 3, "/private-sheets/1").await;
    assert!(
        rendered.contains("name=\"revision\" value=\"4\""),
        "{rendered}"
    );
    assert_eq!(
        location(&setup, 3, "/private-sheets/1/seen", seen_form("4"))
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        (seen_of(&setup, 2, 1).await, seen_of(&setup, 4, 1).await),
        (Some(3), Some(4))
    );
}

/// The probe, the stream and both modules carry the strict policy like the page.
#[tokio::test]
async fn the_probe_the_stream_and_the_modules_carry_the_strict_policy() {
    let _locks = lock().await;
    let setup = world().await;
    two_revisions(&setup).await;
    let (status, headers, _) = super::hub::snapshot_as(&setup, 3, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(csp(&headers), STRICT, "snapshot");
    for path in [
        "/campaigns/1/master/events",
        "/assets/master-live.js",
        "/assets/sheet-seen.js",
    ] {
        let response = router_request(&setup.app, &setup.auth[3], path, None).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(csp(response.headers()), STRICT, "{path}");
    }
}
