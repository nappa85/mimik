//! `/admin/reviewer` through the full auth router (docs/api-reviewer-plan.md step 3): Owner
//! only (players and anonymous refused), CSRF-checked forms, the settings row with its
//! validation, the key file's modes, replace and delete, the key never shown back, and a
//! backup of the database plus the `sources/` folder excluding `secrets/`. The key is a
//! dummy; no environment key is read.
use super::*;
use crate::sources::reviewer::Provider;
use crate::sources::reviewer::key::{DIRECTORY, KEY_FILE, key_identity};
use axum::http::{StatusCode, header};
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const KEY: &str = "dummy-key-not-a-secret";
const OTHER: &str = "another-dummy-key-2";
const CSRF: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

async fn body(response: axum::response::Response) -> String {
    String::from_utf8(
        axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

/// A request with no session cookie.
async fn anonymous(app: &axum::Router, path: &str) -> axum::response::Response {
    use tower::ServiceExt;
    let request = axum::http::Request::builder()
        .uri(path)
        .header("host", "127.0.0.1:3000")
        .header(crate::auth::TEST_DEADLINE_HEADER, "120")
        .body(axum::body::Body::empty())
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

fn settings_form(provider: &str, base_url: &str, model: &str) -> Vec<(String, String)> {
    vec![
        ("csrf".to_owned(), CSRF.to_owned()),
        ("provider".to_owned(), provider.to_owned()),
        ("base_url".to_owned(), base_url.to_owned()),
        ("model".to_owned(), model.to_owned()),
    ]
}

fn key_form(key: &str) -> Vec<(String, String)> {
    vec![
        ("csrf".to_owned(), CSRF.to_owned()),
        ("key".to_owned(), key.to_owned()),
    ]
}

fn csrf_form() -> Vec<(String, String)> {
    vec![("csrf".to_owned(), CSRF.to_owned())]
}

/// Every 8-character substring of `value` (the secrecy tests' search set).
fn substrings8(value: &str) -> Vec<&str> {
    let bytes = value.as_bytes();
    if bytes.len() < 8 {
        return Vec::new();
    }
    (0..=bytes.len() - 8)
        .map(|start| &value[start..start + 8])
        .collect()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            walk(&entry.path(), out);
        } else {
            out.push(entry.path());
        }
    }
}

#[tokio::test]
async fn the_reviewer_page_is_owner_only_and_anonymous_is_refused() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture_with_owner(1).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    assert_eq!(
        router_request(&app, &auth[1], "/admin/reviewer", None)
            .await
            .status(),
        StatusCode::FORBIDDEN,
        "a player is refused"
    );
    assert_eq!(
        anonymous(&app, "/admin/reviewer").await.status(),
        StatusCode::UNAUTHORIZED,
        "anonymous is refused"
    );
    for (path, form) in [
        (
            "/admin/reviewer/settings",
            settings_form("mistral", "https://api.mistral.ai", "m"),
        ),
        ("/admin/reviewer/key", key_form(KEY)),
        ("/admin/reviewer/key/delete", csrf_form()),
    ] {
        assert_eq!(
            router_request(&app, &auth[1], path, Some(form))
                .await
                .status(),
            StatusCode::FORBIDDEN,
            "a player cannot use {path}"
        );
    }
    let response = router_request(&app, &auth[0], "/admin/reviewer", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = body(response).await;
    assert!(page.contains("API reviewer"), "{page}");
    assert!(page.contains("https://api.mistral.ai"), "{page}");
    assert!(page.contains("https://api.openai.com/v1"), "{page}");
    assert!(page.contains("https://api.anthropic.com"), "{page}");
    assert!(page.contains("not set"), "{page}");
    assert!(page.contains("/admin/reviewer/settings"), "{page}");
    assert!(page.contains("/admin/reviewer/key"), "{page}");
    // The per-provider path rules are the page's help text.
    assert!(page.contains("/v1/chat/completions"), "{page}");
    assert!(page.contains("/responses"), "{page}");
    assert!(page.contains("/v1/messages"), "{page}");
    // The menu carries the page.
    let (links, _) = crate::auth::nav::check::menu(&page);
    assert!(
        links.iter().any(|(href, _)| href == "/admin/reviewer"),
        "{links:?}"
    );
}

#[tokio::test]
async fn reviewer_forms_need_the_csrf_token() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture_with_owner(1).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let wrong = "b".repeat(64);
    for (path, form) in [
        (
            "/admin/reviewer/settings",
            vec![
                ("csrf".to_owned(), wrong.clone()),
                ("provider".to_owned(), "mistral".to_owned()),
                ("base_url".to_owned(), "https://api.mistral.ai".to_owned()),
                ("model".to_owned(), "m".to_owned()),
            ],
        ),
        (
            "/admin/reviewer/key",
            vec![
                ("csrf".to_owned(), wrong.clone()),
                ("key".to_owned(), KEY.to_owned()),
            ],
        ),
        (
            "/admin/reviewer/key/delete",
            vec![("csrf".to_owned(), wrong.clone())],
        ),
        // A missing token is refused as well.
        (
            "/admin/reviewer/settings",
            vec![
                ("provider".to_owned(), "mistral".to_owned()),
                ("base_url".to_owned(), "https://api.mistral.ai".to_owned()),
                ("model".to_owned(), "m".to_owned()),
            ],
        ),
    ] {
        let response = router_request(&app, &auth[0], path, Some(form)).await;
        assert!(
            response.status().is_client_error(),
            "{path} refused {}",
            response.status()
        );
    }
    // Nothing was stored or written.
    assert_eq!(store.reviewer_settings().await.unwrap().model, "");
    assert_eq!(store.secrets().status().unwrap(), None);
}

#[tokio::test]
async fn settings_are_saved_after_validation_and_refused_with_a_message() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture_with_owner(1).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let response = router_request(
        &app,
        &auth[0],
        "/admin/reviewer/settings",
        Some(settings_form(
            "openai",
            "https://api.openai.com/v1",
            "gpt-5",
        )),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers()[header::LOCATION],
        "/admin/reviewer?notice=settings-saved"
    );
    let settings = store.reviewer_settings().await.unwrap();
    assert_eq!(settings.provider, Provider::OpenAi);
    assert_eq!(settings.base_url, "https://api.openai.com/v1");
    assert_eq!(settings.model, "gpt-5");
    assert!(settings.changed_at > 0);
    let page = body(router_request(&app, &auth[0], "/admin/reviewer", None).await).await;
    assert!(page.contains("gpt-5"), "{page}");
    assert!(page.contains("https://api.openai.com/v1"), "{page}");
    // Refusals: the page again with 422, the fixed message and the submitted values kept,
    // and the stored row unchanged. A base URL that carries a user name or password is never
    // echoed back (its field is re-rendered empty).
    for (provider, url, model, expected, echoed) in [
        ("mistral", "http://api.mistral.ai", "m", "HTTPS", true),
        ("mistral", "https://127.0.0.1", "m", "domain", true),
        ("mistral", "https://[::1]/v1", "m", "domain", true),
        (
            "openai",
            "https://user:secret@api.openai.com/v1",
            "m",
            "user name",
            false,
        ),
        (
            "openai",
            "https://user:se cret@api.openai.com/v1",
            "m",
            "user name",
            false,
        ),
        ("openai", "https://api.openai.com", "m", "bare host", true),
        ("mistral", "https://api.mistral.ai/v1", "m", "/v1", true),
        (
            "anthropic",
            "https://api.anthropic.com/v1",
            "m",
            "bare host",
            true,
        ),
        ("google", "https://api.google.com", "m", "provider", true),
        ("mistral", "https://api.mistral.ai", "", "empty", true),
        (
            "mistral",
            "https://api.mistral.ai",
            &"m".repeat(92),
            "91",
            true,
        ),
    ] {
        let response = router_request(
            &app,
            &auth[0],
            "/admin/reviewer/settings",
            Some(settings_form(provider, url, model)),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{provider} {url} {model}"
        );
        let text = body(response).await;
        assert!(text.contains(expected), "{url}: {text}");
        if echoed {
            assert!(
                text.contains(url),
                "the submitted base URL is kept in its field: {text}"
            );
        } else {
            assert!(
                !text.contains(url) && !text.contains("user:secret") && !text.contains("se cret"),
                "the submitted base URL with credentials is not echoed: {text}"
            );
        }
        assert_eq!(store.reviewer_settings().await.unwrap().model, "gpt-5");
    }
}

/// A refused base URL that carries a `@` without userinfo, or does not parse at all, is not
/// echoed either: the base URL field is re-rendered empty and the submitted text appears
/// nowhere in the page.
#[tokio::test]
async fn a_refused_base_url_with_at_or_unparseable_is_blanked_not_echoed() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture_with_owner(1).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    router_request(
        &app,
        &auth[0],
        "/admin/reviewer/settings",
        Some(settings_form(
            "openai",
            "https://api.openai.com/v1",
            "gpt-5",
        )),
    )
    .await;
    for (provider, url, expected) in [
        ("openai", "https://api.openai.com/v1@relay", "the /v1 path"),
        (
            "mistral",
            "https://api.mistral.ai is not a url",
            "malformed",
        ),
    ] {
        let response = router_request(
            &app,
            &auth[0],
            "/admin/reviewer/settings",
            Some(settings_form(provider, url, "m")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY, "{url}");
        let text = body(response).await;
        assert!(text.contains(expected), "{url}: {text}");
        assert!(
            !text.contains(url) && !text.contains("@relay"),
            "the submitted base URL is not echoed: {text}"
        );
        assert!(
            text.contains("name=\"base_url\" value=\"\""),
            "the base URL field is re-rendered empty: {text}"
        );
        assert_eq!(store.reviewer_settings().await.unwrap().model, "gpt-5");
    }
}

#[tokio::test]
async fn the_key_is_set_replaced_and_deleted_with_0600_and_never_shown() {
    let _lock = crate::auth::test_resource_lock().await;
    let (dir, store, auth, _clock) = fixture_with_owner(1).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let secrets = dir.path().join(DIRECTORY);
    let key_path = secrets.join(KEY_FILE);
    // Set.
    let response = router_request(&app, &auth[0], "/admin/reviewer/key", Some(key_form(KEY))).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers()[header::LOCATION],
        "/admin/reviewer?notice=key-set"
    );
    assert_eq!(mode(&secrets), 0o700, "the secrets folder is 0700");
    assert_eq!(mode(&key_path), 0o600, "the key file is 0600");
    assert_eq!(std::fs::read(&key_path).unwrap(), KEY.as_bytes());
    // The page shows "set" and the change time, never the key.
    let page = body(router_request(&app, &auth[0], "/admin/reviewer", None).await).await;
    assert!(page.contains("<strong>set</strong>"), "{page}");
    assert!(page.contains("changed 20"), "{page}");
    for piece in substrings8(KEY) {
        assert!(!page.contains(piece), "the page holds {piece:?}");
    }
    // The identity is exposed in memory only.
    let identity = key_identity(&store).unwrap().expect("the key is set");
    assert_eq!(
        identity.digest.to_vec(),
        Sha256::digest(KEY.as_bytes()).to_vec()
    );
    assert!(identity.changed_at > 0);
    // Replace.
    let response =
        router_request(&app, &auth[0], "/admin/reviewer/key", Some(key_form(OTHER))).await;
    assert_eq!(
        response.headers()[header::LOCATION],
        "/admin/reviewer?notice=key-replaced"
    );
    assert_eq!(std::fs::read(&key_path).unwrap(), OTHER.as_bytes());
    assert_eq!(mode(&key_path), 0o600);
    assert_ne!(
        key_identity(&store).unwrap().unwrap().digest,
        identity.digest
    );
    // Delete.
    let response = router_request(
        &app,
        &auth[0],
        "/admin/reviewer/key/delete",
        Some(csrf_form()),
    )
    .await;
    assert_eq!(
        response.headers()[header::LOCATION],
        "/admin/reviewer?notice=key-deleted"
    );
    assert!(!key_path.exists());
    assert_eq!(key_identity(&store).unwrap(), None);
    let page = body(router_request(&app, &auth[0], "/admin/reviewer", None).await).await;
    assert!(page.contains("<strong>not set</strong>"), "{page}");
    // Delete again.
    let response = router_request(
        &app,
        &auth[0],
        "/admin/reviewer/key/delete",
        Some(csrf_form()),
    )
    .await;
    assert_eq!(
        response.headers()[header::LOCATION],
        "/admin/reviewer?notice=key-not-set"
    );
    // Refused keys: a message, 422, no file, and the submitted key never echoed. The save rule:
    // visible ASCII only after trimming the ends, so a line break, a space or a non-ASCII
    // character in the middle is refused too.
    for bad in [
        "",
        &"k".repeat(513),
        "ab\ncd",
        "ab cd",
        "ab\u{e9}cd",
        "ab\x7Fcd",
    ] {
        let response =
            router_request(&app, &auth[0], "/admin/reviewer/key", Some(key_form(bad))).await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{bad:?}"
        );
        let text = body(response).await;
        assert!(
            text.contains("1 to 512 visible ASCII characters"),
            "{bad:?}: {text}"
        );
        assert!(text.contains("nothing was saved"), "{bad:?}: {text}");
        assert!(!key_path.exists());
        for piece in substrings8(bad) {
            assert!(!text.contains(piece), "the refused form echoes {piece:?}");
        }
    }
}

/// A key file that is not private (a wrong mode or a link) does not break the page: it answers
/// 200 with the fixed message and keeps both the replace and the delete form; replacing works
/// anyway (the rename replaces the file) and leaves a 0600 file, and deleting works.
#[tokio::test]
async fn a_key_file_that_is_not_private_still_renders_and_can_be_replaced_and_deleted() {
    let _lock = crate::auth::test_resource_lock().await;
    let (dir, store, auth, _clock) = fixture_with_owner(1).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let key_path = dir.path().join(DIRECTORY).join(KEY_FILE);
    // Set a key, then make the file group-readable.
    let response = router_request(&app, &auth[0], "/admin/reviewer/key", Some(key_form(KEY))).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    // The page still renders 200, with the fixed message and both forms.
    let response = router_request(&app, &auth[0], "/admin/reviewer", None).await;
    assert_eq!(response.status(), StatusCode::OK, "the page renders");
    let page = body(response).await;
    assert!(
        page.contains("The key file is not private: replace or delete it"),
        "{page}"
    );
    assert!(
        page.contains("action=\"/admin/reviewer/key\""),
        "the replace form is kept: {page}"
    );
    assert!(
        page.contains("action=\"/admin/reviewer/key/delete\""),
        "the delete form is kept: {page}"
    );
    for piece in substrings8(KEY) {
        assert!(!page.contains(piece), "the page holds {piece:?}");
    }
    // Replace succeeds anyway and leaves a 0600 file.
    let response =
        router_request(&app, &auth[0], "/admin/reviewer/key", Some(key_form(OTHER))).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers()[header::LOCATION],
        "/admin/reviewer?notice=key-replaced"
    );
    assert_eq!(std::fs::read(&key_path).unwrap(), OTHER.as_bytes());
    assert_eq!(mode(&key_path), 0o600, "the replaced file is 0600 again");
    // A link in place of the file: the page renders the same way.
    let link = dir.path().join(DIRECTORY).join("a-link");
    std::os::unix::fs::symlink(&key_path, &link).unwrap();
    std::fs::remove_file(&key_path).unwrap();
    std::os::unix::fs::symlink(&link, &key_path).unwrap();
    let response = router_request(&app, &auth[0], "/admin/reviewer", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = body(response).await;
    assert!(
        page.contains("The key file is not private: replace or delete it"),
        "{page}"
    );
    assert!(
        page.contains("action=\"/admin/reviewer/key/delete\""),
        "{page}"
    );
    // Delete succeeds (the link is unlinked).
    let response = router_request(
        &app,
        &auth[0],
        "/admin/reviewer/key/delete",
        Some(csrf_form()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers()[header::LOCATION],
        "/admin/reviewer?notice=key-deleted"
    );
    assert!(!key_path.exists(), "the link is gone");
    assert!(
        std::fs::symlink_metadata(&link).is_ok(),
        "the link's target stays"
    );
}

/// A key file whose read fails (here: 0600 but 513 bytes, over the byte bound) does not break
/// the page either: it answers 200 with the fixed "cannot be used" message, is not ready, keeps
/// both the replace and the delete form, shows none of the file's content, and delete works.
#[tokio::test]
async fn a_key_file_that_cannot_be_read_still_renders_and_can_be_deleted() {
    let _lock = crate::auth::test_resource_lock().await;
    let (dir, store, auth, _clock) = fixture_with_owner(1).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let key_path = dir.path().join(DIRECTORY).join(KEY_FILE);
    // A trailing line break (a hand-written file) is read but breaks the save rule: the
    // same fixed message.
    std::fs::write(&key_path, format!("{KEY}\n")).unwrap();
    std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let response = router_request(&app, &auth[0], "/admin/reviewer", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = body(response).await;
    assert!(
        page.contains("The key file cannot be used: replace or delete it"),
        "{page}"
    );
    assert!(page.contains("A reviewer cannot be built yet"), "{page}");
    std::fs::write(&key_path, "k".repeat(513)).unwrap();
    std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let response = router_request(&app, &auth[0], "/admin/reviewer", None).await;
    assert_eq!(response.status(), StatusCode::OK, "the page renders");
    let page = body(response).await;
    assert!(
        page.contains("The key file cannot be used: replace or delete it"),
        "{page}"
    );
    assert!(
        page.contains("action=\"/admin/reviewer/key\""),
        "the replace form is kept: {page}"
    );
    assert!(
        page.contains("action=\"/admin/reviewer/key/delete\""),
        "the delete form is kept: {page}"
    );
    assert!(
        page.contains("A reviewer cannot be built yet"),
        "the page is not ready: {page}"
    );
    assert!(
        !page.contains("kkkkkkkk"),
        "the file's content is not shown: {page}"
    );
    // Delete works.
    let response = router_request(
        &app,
        &auth[0],
        "/admin/reviewer/key/delete",
        Some(csrf_form()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers()[header::LOCATION],
        "/admin/reviewer?notice=key-deleted"
    );
    assert!(!key_path.exists(), "the file is gone");
}

#[tokio::test]
async fn no_8_character_substring_of_the_key_appears_in_any_response_error_or_database_row() {
    let _lock = crate::auth::test_resource_lock().await;
    let (dir, store, auth, _clock) = fixture_with_owner(1).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    router_request(&app, &auth[0], "/admin/reviewer/key", Some(key_form(KEY))).await;
    router_request(
        &app,
        &auth[0],
        "/admin/reviewer/settings",
        Some(settings_form(
            "mistral",
            "https://api.mistral.ai",
            "mistral-large",
        )),
    )
    .await;
    let mut bodies = Vec::new();
    for path in [
        "/admin/reviewer",
        "/admin/reviewer?notice=settings-saved",
        "/admin/reviewer?notice=key-set",
        "/admin/reviewer?notice=key-replaced",
        "/admin/reviewer?notice=key-deleted",
        "/admin/reviewer?notice=key-not-set",
    ] {
        bodies.push(body(router_request(&app, &auth[0], path, None).await).await);
    }
    // A refused settings form (error re-render), a refused key form, a CSRF refusal, a
    // player's refusal and a redirect location.
    let refused = router_request(
        &app,
        &auth[0],
        "/admin/reviewer/settings",
        Some(settings_form("mistral", "http://x.example", "m")),
    )
    .await;
    bodies.push(body(refused).await);
    let refused = router_request(&app, &auth[0], "/admin/reviewer/key", Some(key_form(""))).await;
    bodies.push(body(refused).await);
    let wrong_csrf = router_request(
        &app,
        &auth[0],
        "/admin/reviewer/key",
        Some(vec![
            ("csrf".to_owned(), "b".repeat(64)),
            ("key".to_owned(), KEY.to_owned()),
        ]),
    )
    .await;
    bodies.push(body(wrong_csrf).await);
    let player = router_request(&app, &auth[1], "/admin/reviewer", None).await;
    bodies.push(body(player).await);
    let redirect = router_request(&app, &auth[0], "/admin/reviewer/key", Some(key_form(KEY))).await;
    bodies.push(
        redirect.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_owned(),
    );
    for text in &bodies {
        for piece in substrings8(KEY) {
            assert!(
                !text.contains(piece),
                "a response holds {piece:?}: {}",
                &text[..text.len().min(400)]
            );
        }
    }
    // The database file and the settings row hold no piece of the key.
    let database = std::fs::read(dir.path().join("private.sqlite3")).unwrap();
    for piece in substrings8(KEY) {
        assert!(
            !contains(&database, piece.as_bytes()),
            "the database file holds {piece:?}"
        );
    }
    let row = store
        .run(|db| {
            Ok(db.query_row(
                "SELECT provider || ' ' || base_url || ' ' || model FROM reviewer_settings WHERE id=1",
                [],
                |r| r.get::<_, String>(0),
            )?)
        })
        .await
        .unwrap();
    for piece in substrings8(KEY) {
        assert!(!row.contains(piece), "the settings row holds {piece:?}");
    }
    // The key file itself is the key's one place.
    assert_eq!(
        std::fs::read(dir.path().join(DIRECTORY).join(KEY_FILE)).unwrap(),
        KEY.as_bytes()
    );
}

#[tokio::test]
async fn a_backup_of_the_database_and_the_sources_folder_excludes_the_secrets_folder() {
    let _lock = crate::auth::test_resource_lock().await;
    let (dir, store, auth, _clock) = fixture_with_owner(1).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    router_request(&app, &auth[0], "/admin/reviewer/key", Some(key_form(KEY))).await;
    // The documented backup (docs/release-scope.md): the database file plus the sources/
    // folder beside it, copied with the server stopped.
    let backup = tempfile::tempdir().unwrap();
    std::fs::copy(
        dir.path().join("private.sqlite3"),
        backup.path().join("private.sqlite3"),
    )
    .unwrap();
    copy_dir(&dir.path().join("sources"), &backup.path().join("sources"));
    assert!(
        !backup.path().join(DIRECTORY).exists(),
        "the backup has no secrets folder"
    );
    let mut files = Vec::new();
    walk(backup.path(), &mut files);
    assert!(!files.is_empty(), "the backup copied something");
    for file in &files {
        let bytes = std::fs::read(file).unwrap();
        for piece in substrings8(KEY) {
            assert!(
                !contains(&bytes, piece.as_bytes()),
                "{} holds {piece:?}",
                file.display()
            );
        }
    }
    // The data folder does hold the secrets folder (so the exclusion is real).
    let mut names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    for expected in ["private.sqlite3", "secrets", "sources"] {
        assert!(names.iter().any(|n| n == expected), "{names:?}");
    }
}
