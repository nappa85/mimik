//! The shared header and menu of the authenticated app (`docs/auth-and-sessions.md`, Pages and menu):
//! one table over its page routes for the Owner and a player.
//!
//! Pages that never carry the menu because they have no session: the error documents rebuilt by
//! `errors::document`, the sign-in, enrolment and reset forms (checked below), the catalogue
//! notices, `private_sheet_busy` and the non-Linux upload stub. They keep a logo-only header.
use super::formula_tests::{body, setup};
use super::notes_tests::{assist, created};
use super::router_request;
use axum::http::{StatusCode, header};

const STRICT: &str = "default-src 'self'; script-src 'self'; style-src 'self'; \
    object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'; \
    require-trusted-types-for 'script'; trusted-types 'none'";
const DEFAULT: &str = "frame-ancestors 'none'";
fn csp(response: &axum::response::Response) -> String {
    response.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap()
        .to_owned()
}

#[cfg(target_os = "linux")]
struct Available;
#[cfg(target_os = "linux")]
impl crate::sources::uploads::Worker for Available {
    fn availability(&self) -> crate::sources::worker::Availability {
        crate::sources::worker::Availability::Available
    }
    fn inspect(
        &self,
        _: &std::path::Path,
        _: &crate::sources::worker::Cancel,
    ) -> Result<crate::sources::worker::Inspection, crate::sources::worker::Reason> {
        unreachable!()
    }
    fn render(
        &self,
        _: &std::path::Path,
        _: &[crate::sources::worker::Page],
        _: &crate::sources::worker::Cancel,
        _: &mut (
                 dyn FnMut(
            crate::sources::worker::RenderedPage,
        ) -> Result<(), crate::sources::worker::Error>
                     + Send
             ),
    ) -> Result<(), crate::sources::worker::Reason> {
        unreachable!()
    }
}

#[cfg(target_os = "linux")]
fn options(store: &crate::Store, dir: &std::path::Path) -> crate::auth::AuthOptions {
    crate::auth::AuthOptions {
        uploads: Some(crate::sources::uploads::Uploads::for_route_test(
            store.clone(),
            dir,
            std::sync::Arc::new(Available),
        )),
        ..crate::auth::AuthOptions::default()
    }
}
#[cfg(not(target_os = "linux"))]
fn options(_: &crate::Store, _: &std::path::Path) -> crate::auth::AuthOptions {
    crate::auth::AuthOptions::default()
}

/// One page of the table: who reads it, which entry is marked current and whether the page is
/// served under the strict sheet policy.
struct Page<'a> {
    auth: &'a crate::auth::RequestAuth,
    owner: bool,
    path: String,
    current: Option<&'static str>,
    strict: bool,
}

async fn check(app: &axum::Router, page: &Page<'_>) {
    let Page {
        auth,
        owner,
        path,
        current,
        strict,
    } = page;
    let response = router_request(app, auth, path, None).await;
    assert_eq!(response.status(), StatusCode::OK, "{path}");
    assert_eq!(
        csp(&response),
        if *strict { STRICT } else { DEFAULT },
        "{path}"
    );
    let html = body(response).await;
    crate::auth::nav::check::assert_header(&html, *owner, *current, *strict, path);
    let (_, summaries) = crate::auth::nav::check::menu(&html);
    let labels: &[&str] = if *owner {
        &["Materials", "Administration", "Account: player1"]
    } else {
        &["Play", "Account: player2"]
    };
    assert_eq!(summaries, labels, "{path}");
}

/// Every entry answers 200 for its role, and the menu script is served.
async fn entries_answer(
    app: &axum::Router,
    owner: &crate::auth::RequestAuth,
    player: &crate::auth::RequestAuth,
) {
    for (auth, entries) in [
        (owner, &crate::auth::nav::check::OWNER_ENTRIES[..]),
        (player, &crate::auth::nav::check::PLAYER_ENTRIES[..]),
    ] {
        for entry in entries {
            if cfg!(not(target_os = "linux")) && *entry == "/admin/materials/uploads" {
                continue;
            }
            let response = router_request(app, auth, entry, None).await;
            assert_eq!(response.status(), StatusCode::OK, "{entry}");
        }
    }
    // The script itself is served.
    let script = router_request(app, player, "/assets/nav.js", None).await;
    assert_eq!(script.status(), StatusCode::OK);
    assert_eq!(
        script.headers()[header::CONTENT_TYPE],
        "text/javascript; charset=utf-8"
    );
}

#[tokio::test]
async fn every_page_renders_the_menu_of_its_role_and_every_entry_answers() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    let app =
        crate::auth::app_with_options(setup.store.clone(), options(&setup.store, setup.dir.path()))
            .await
            .unwrap();
    created(&setup, &[]).await;
    let owner = &setup.auth[0];
    let player = &setup.auth[1];
    let page = |auth, owner, path: &str, current, strict| Page {
        auth,
        owner,
        path: path.to_owned(),
        current,
        strict,
    };
    let campaign = format!("/campaigns/{}", setup.campaign);
    let mut table = vec![
        page(owner, true, "/", None, false),
        page(owner, true, "/admin", Some("/admin"), false),
        page(
            owner,
            true,
            "/admin/assistant",
            Some("/admin/assistant"),
            false,
        ),
        page(
            owner,
            true,
            "/admin/rulesets",
            Some("/admin/rulesets"),
            false,
        ),
        page(
            owner,
            true,
            "/admin/materials",
            Some("/admin/materials"),
            true,
        ),
        page(player, false, "/", None, false),
        page(player, false, "/campaigns", Some("/campaigns"), false),
        page(player, false, &campaign, Some("/campaigns"), false),
        page(
            player,
            false,
            "/private-sheets",
            Some("/private-sheets"),
            false,
        ),
        page(
            player,
            false,
            "/private-sheets/1",
            Some("/private-sheets"),
            true,
        ),
        page(
            player,
            false,
            "/private-sheets/1/history",
            Some("/private-sheets"),
            false,
        ),
        page(
            player,
            false,
            "/private-sheets/1/history/1",
            Some("/private-sheets"),
            true,
        ),
    ];
    if cfg!(target_os = "linux") {
        table.push(page(
            owner,
            true,
            "/admin/materials/uploads",
            Some("/admin/materials/uploads"),
            true,
        ));
    }
    for entry in &table {
        check(&app, entry).await;
    }
    entries_answer(&app, owner, player).await;
}

#[tokio::test]
async fn sign_out_needs_the_csrf_token_and_session_page_redirects_home() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    let player = &setup.auth[1];
    let status = router_request(&setup.app, player, "/auth/session", None).await;
    assert_eq!(status.status(), StatusCode::SEE_OTHER);
    assert_eq!(status.headers()[header::LOCATION], "/");
    for form in [vec![], vec![("csrf".to_owned(), "b".repeat(64))]] {
        let refused = router_request(&setup.app, player, "/auth/logout", Some(form)).await;
        assert!(refused.status().is_client_error());
        assert_eq!(
            router_request(&setup.app, player, "/", None).await.status(),
            StatusCode::OK,
            "a refused sign out keeps the session"
        );
    }
    let out = router_request(
        &setup.app,
        player,
        "/auth/logout",
        Some(vec![("csrf".to_owned(), "a".repeat(64))]),
    )
    .await;
    assert_eq!(out.status(), StatusCode::SEE_OTHER);
    assert_eq!(out.headers()[header::LOCATION], "/auth/login");
    let after = router_request(&setup.app, player, "/", None).await;
    assert_eq!(after.status(), StatusCode::SEE_OTHER);
    assert_eq!(after.headers()[header::LOCATION], "/auth/login");
}

#[tokio::test]
async fn pages_without_a_session_keep_a_logo_only_header() {
    use tower::ServiceExt;
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    for path in ["/auth/login", "/auth/enroll", "/auth/reset"] {
        let response = setup
            .app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri(path)
                    .header("host", "127.0.0.1:3000")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let html = body(response).await;
        assert!(!html.contains("class=\"menu\""), "{path}");
        assert!(!html.contains("/auth/logout"), "{path}");
    }
    let anonymous = setup
        .app
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
    assert_eq!(anonymous.status(), StatusCode::SEE_OTHER);
    assert_eq!(anonymous.headers()[header::LOCATION], "/auth/login");
}

#[tokio::test]
async fn sheet_create_page_and_post_rerenders_carry_the_header() {
    use super::notes_tests::{create_path, form, row};
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    let header = |html: &str, at: &str| {
        crate::auth::nav::check::assert_header(html, false, Some("/private-sheets"), true, at);
    };
    let (status, html) = setup.get(&create_path(&setup)).await;
    assert_eq!(status, StatusCode::OK);
    header(&html, "create page");
    // The full form refused: the sheet renderer's POST path.
    let refused = form(0, &[("value-hp", "84\u{7}")], &[row("", "1")]);
    let (status, html) = setup.post(&create_path(&setup), refused).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    header(&html, "create refused");
    created(&setup, &[]).await;
    // A per-unit save refused (unit_web) and a refused switch (assist_web).
    let unit = vec![
        ("csrf".to_owned(), "a".repeat(64)),
        ("base".to_owned(), "1".to_owned()),
        ("value".to_owned(), "84\u{7}".to_owned()),
    ];
    let (status, html) = setup.post("/private-sheets/1/fields/hp", unit).await;
    assert!(status.is_client_error(), "{status}");
    header(&html, "unit refused");
    let stale = vec![
        ("csrf".to_owned(), "a".repeat(64)),
        ("base".to_owned(), "1".to_owned()),
        ("note".to_owned(), "1:field:hp".to_owned()),
    ];
    let (status, html) = setup.post("/private-sheets/1/switch", stale).await;
    assert!(
        matches!(
            status,
            StatusCode::CONFLICT | StatusCode::UNPROCESSABLE_ENTITY
        ),
        "{status} {html}"
    );
    header(&html, "switch refused");
}

#[tokio::test]
async fn account_issued_page_carries_the_header() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    let form = vec![
        ("csrf".to_owned(), "a".repeat(64)),
        ("login".to_owned(), "invitee".to_owned()),
        ("display_name".to_owned(), "Invitee".to_owned()),
        ("epoch".to_owned(), "0".to_owned()),
    ];
    let response = router_request(&setup.app, &setup.auth[0], "/admin/invite", Some(form)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body(response).await;
    assert!(html.contains("Invitation ready"));
    crate::auth::nav::check::assert_header(&html, true, Some("/admin"), false, "account issued");
}
