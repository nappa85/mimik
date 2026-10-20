//! Step 2a "JavaScript foundation" (`docs/assisted-entry.md`, "Script on sheet pages"; D-G,
//! D-AA): sheet pages carry a strict CSP set in `normalize` and load one module; a per-unit
//! save asked for `application/json` answers the saved unit, the derived values, the checks
//! and the validity flag of the live revision, with every integer and revision a string.
//! Without that header the PRG path is unchanged.
use super::formula_tests::{Setup, body, setup};
use super::notes_tests::{assist, created, saved};
use axum::http::{StatusCode, header};
use serde_json::{Value, json};

/// The CSP of every sheet page and sheet JSON answer.
const STRICT: &str = "default-src 'self'; script-src 'self'; style-src 'self'; \
    object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'; \
    require-trusted-types-for 'script'; trusted-types 'none'";
/// The CSP of every other answer.
const DEFAULT: &str = "frame-ancestors 'none'";
/// The one script a saved, editable sheet page loads.
pub(super) const MODULE_TAG: &str =
    r#"<script type="module" src="/assets/sheet-assist.js"></script>"#;

type Pairs = Vec<(String, String)>;

fn unit(base: &str, value: &str, extra: &[(&str, &str)]) -> Pairs {
    let mut pairs = vec![
        ("csrf".to_owned(), "a".repeat(64)),
        ("base".to_owned(), base.to_owned()),
        ("value".to_owned(), value.to_owned()),
    ];
    pairs.extend(
        extra
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
    );
    pairs
}

/// A POST as the module sends it (URL-encoded, `Accept: application/json`, CORS-mode fetch
/// metadata), or as a plain form when `json` is false.
async fn send(
    setup: &Setup,
    path: &str,
    pairs: &Pairs,
    json: bool,
    content_type: &str,
) -> axum::response::Response {
    use tower::ServiceExt;
    let mut request = axum::http::Request::builder()
        .uri(path)
        .method("POST")
        .header("host", "127.0.0.1:3000")
        .header("origin", "http://127.0.0.1:3000")
        .header("content-type", content_type)
        .header("cookie", setup.auth[1].fixture_cookie());
    if json {
        request = request
            .header("accept", "application/json")
            .header("sec-fetch-mode", "cors")
            .header("sec-fetch-dest", "empty");
    }
    let body = serde_urlencoded::to_string(pairs).unwrap();
    setup
        .app
        .clone()
        .oneshot(request.body(axum::body::Body::from(body)).unwrap())
        .await
        .unwrap()
}

async fn post_json(setup: &Setup, path: &str, pairs: &Pairs) -> axum::response::Response {
    send(
        setup,
        path,
        pairs,
        true,
        "application/x-www-form-urlencoded;charset=UTF-8",
    )
    .await
}

async fn get(setup: &Setup, path: &str) -> axum::response::Response {
    use tower::ServiceExt;
    let request = axum::http::Request::builder()
        .uri(path)
        .header("host", "127.0.0.1:3000")
        .header("cookie", setup.auth[1].fixture_cookie())
        .body(axum::body::Body::empty())
        .unwrap();
    setup.app.clone().oneshot(request).await.unwrap()
}

fn csp(response: &axum::response::Response) -> &str {
    let mut values = response
        .headers()
        .get_all(header::CONTENT_SECURITY_POLICY)
        .iter();
    let first = values.next().expect("a CSP header").to_str().unwrap();
    assert!(values.next().is_none(), "one CSP header");
    first
}

/// The final headers every sheet JSON answer carries after `normalize`.
fn assert_json_headers(response: &axum::response::Response, status: StatusCode) {
    assert_eq!(response.status(), status);
    let headers = response.headers();
    assert_eq!(headers[header::CONTENT_TYPE], "application/json");
    assert_eq!(csp(response), STRICT);
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
    assert_eq!(headers[header::REFERRER_POLICY], "same-origin");
    assert!(!headers.contains_key(header::LOCATION));
}

/// Fails on any JSON number, at any depth: integers and revisions are strings.
fn assert_no_numbers(value: &Value, at: &str) {
    match value {
        Value::Number(number) => panic!("JSON number {number} at {at}"),
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                assert_no_numbers(item, &format!("{at}[{index}]"));
            }
        }
        Value::Object(members) => {
            for (key, item) in members {
                assert_no_numbers(item, &format!("{at}.{key}"));
            }
        }
        _ => {}
    }
}

async fn json_body(response: axum::response::Response) -> Value {
    let text = body(response).await;
    let value: Value = serde_json::from_str(&text).unwrap_or_else(|_| panic!("JSON: {text}"));
    assert_no_numbers(&value, "$");
    value
}

async fn sheet(setup: &Setup) -> String {
    let response = get(setup, "/private-sheets/1").await;
    assert_eq!(response.status(), StatusCode::OK);
    body(response).await
}

#[tokio::test]
async fn sheet_pages_get_the_strict_csp_and_only_the_module() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    let create = format!(
        "/campaigns/{}/sheets/new?version={}",
        setup.campaign, setup.version
    );
    for path in ["/private-sheets/1", "/private-sheets/1/history/1"] {
        if path == "/private-sheets/1" {
            let response = get(&setup, &create).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(csp(&response), STRICT, "create page");
            let html = body(response).await;
            assert!(!html.contains("<script"), "the create page loads no script");
            assert!(
                !html.contains(" style="),
                "the create page has a style attribute"
            );
            created(&setup, &[]).await;
        }
        let response = get(&setup, path).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(csp(&response), STRICT, "{path}");
        let html = body(response).await;
        assert!(!html.contains(" style="), "{path} has a style attribute");
        if path == "/private-sheets/1" {
            // The editable saved page loads the one module and nothing else.
            assert_eq!(html.matches("<script").count(), 1, "{path}");
            assert!(html.contains(MODULE_TAG), "{path}");
        } else {
            assert!(!html.contains("<script"), "{path} loads a script");
        }
    }
    // Every other route keeps the default header.
    for path in [
        "/campaigns",
        "/private-sheets",
        "/private-sheets/1/history",
        "/auth/session",
        "/assets/app.css",
        "/assets/sheet-assist.js",
    ] {
        let response = get(&setup, path).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(csp(&response), DEFAULT, "{path}");
    }
}

#[tokio::test]
async fn the_module_is_served_from_the_asset_route() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    let response = get(&setup, "/assets/sheet-assist.js").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/javascript; charset=utf-8"
    );
    assert_eq!(
        response.headers()[header::X_CONTENT_TYPE_OPTIONS],
        "nosniff"
    );
    assert_eq!(body(response).await, module_source());
}

#[tokio::test]
async fn a_json_save_answers_the_projection_of_the_new_revision() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let response = post_json(
        &setup,
        "/private-sheets/1/fields/hp",
        &unit("1", "84 -15", &[]),
    )
    .await;
    assert_json_headers(&response, StatusCode::OK);
    let answer = json_body(response).await;
    assert_eq!(answer["status"], "saved");
    assert_eq!(answer["revision"], "2");
    assert_eq!(answer["message"], "Current HP saved.");
    assert_eq!(
        answer["unit"],
        json!({
            "address": "field:hp",
            "action": "/private-sheets/1/fields/hp",
            "value": "69",
            "state": null,
            "note": {"kind": "sum", "shown": "84 \u{2212} 15", "value": "69", "unknown": false},
            "actions": {"note": "2:field:hp", "offer": null, "undo": "84"},
        })
    );
    assert_eq!(
        answer["derived"],
        json!([{"id": "load", "text": "1", "code": null}])
    );
    assert_eq!(answer["not_computed"], Value::Null);
    assert_eq!(answer["unavailable"], Value::Null);
    assert_eq!(
        answer["checks"],
        json!({"violated": "0", "not_evaluated": "0", "passed": "1", "items": [{
            "id": "hp-cap", "status": "pass", "word": "Pass",
            "message": "Current HP is at most Max HP.", "code": null, "explanation": null,
            "expr": "hp <= max_hp", "rule_ids": "synthetic-2",
        }]})
    );
    assert_eq!(
        answer["validity"],
        json!({"validated": true, "reasons": []})
    );
    assert_eq!(saved(&setup, 1).await.0, 2);
    assert_eq!(saved(&setup, 1).await.1["hp"], json!(69));

    // Above Max HP: the check and the validity flag move with the save.
    let response = post_json(&setup, "/private-sheets/1/fields/hp", &unit("2", "90", &[])).await;
    assert_json_headers(&response, StatusCode::OK);
    let answer = json_body(response).await;
    assert_eq!(answer["revision"], "3");
    assert_eq!(answer["checks"]["violated"], "1");
    assert_eq!(answer["checks"]["items"][0]["status"], "violated");
    assert_eq!(answer["checks"]["items"][0]["word"], "Violated");
    assert_eq!(
        answer["validity"],
        json!({"validated": false, "reasons": [
            {"text": "1 check violated", "anchor": "advisory-checks"}
        ]})
    );

    // A blank HP: the check is not evaluated, with its reason and code.
    let response = post_json(&setup, "/private-sheets/1/fields/hp", &unit("3", "", &[])).await;
    let answer = json_body(response).await;
    let item = &answer["checks"]["items"][0];
    assert_eq!(answer["unit"]["value"], "");
    assert_eq!(item["status"], "not_evaluated");
    assert_eq!(item["code"], "null_value");
    assert_eq!(item["explanation"], "a value it uses is unknown (null)");
    assert_eq!(
        answer["validity"]["reasons"],
        json!([{"text": "1 check not evaluated", "anchor": "advisory-checks"}])
    );

    // The same value again: "No change", nothing written, the live projection answered.
    let response = post_json(&setup, "/private-sheets/1/fields/hp", &unit("1", "", &[])).await;
    assert_json_headers(&response, StatusCode::OK);
    let answer = json_body(response).await;
    assert_eq!(answer["status"], "unchanged");
    assert_eq!(answer["revision"], "4");
    assert_eq!(
        answer["message"],
        "No change: Current HP already holds this value, so nothing was saved."
    );
    assert_eq!(saved(&setup, 1).await.0, 4);
}

#[tokio::test]
async fn a_json_cell_save_answers_its_row_and_the_new_load() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let response = post_json(
        &setup,
        "/private-sheets/1/cells/4/0/2",
        &unit("1", "2+3", &[]),
    )
    .await;
    assert_json_headers(&response, StatusCode::OK);
    let answer = json_body(response).await;
    assert_eq!(answer["unit"]["address"], "cell:equipment/r1-1/weight");
    assert_eq!(answer["unit"]["action"], "/private-sheets/1/cells/4/0/2");
    assert_eq!(answer["unit"]["value"], "5");
    assert_eq!(answer["unit"]["note"]["shown"], "2 + 3");
    assert_eq!(
        answer["derived"][0],
        json!({"id": "load", "text": "5", "code": null})
    );
    // An optional property answers its Set/Not set state.
    let response = post_json(
        &setup,
        "/private-sheets/1/cells/4/0/3",
        &unit("2", "true", &[("set", "present")]),
    )
    .await;
    let answer = json_body(response).await;
    assert_eq!(answer["unit"]["value"], "true");
    assert_eq!(answer["unit"]["state"], "present");
    // A nullable field answers Known/Unknown.
    let response = post_json(
        &setup,
        "/private-sheets/1/fields/luck",
        &unit("3", "4", &[("null", "known")]),
    )
    .await;
    let answer = json_body(response).await;
    assert_eq!(answer["unit"]["value"], "4");
    assert_eq!(answer["unit"]["state"], "known");
    // Name and General notes answer their text.
    let response = post_json(
        &setup,
        "/private-sheets/1/meta/name",
        &unit("4", "Renamed", &[]),
    )
    .await;
    let answer = json_body(response).await;
    assert_eq!(answer["unit"]["address"], "meta:name");
    assert_eq!(answer["unit"]["value"], "Renamed");
    assert_eq!(answer["unit"]["note"], Value::Null);
}

#[tokio::test]
async fn json_refusals_keep_the_draft_and_the_strict_headers() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    // A base above L is 409, answered with L's projection.
    let response = post_json(&setup, "/private-sheets/1/fields/hp", &unit("5", "60", &[])).await;
    assert_json_headers(&response, StatusCode::CONFLICT);
    let answer = json_body(response).await;
    assert_eq!(answer["status"], "conflict");
    assert_eq!(answer["revision"], "1");
    assert_eq!(answer["unit"]["action"], "/private-sheets/1/fields/hp");
    assert!(answer["unit"].get("value").is_none(), "{answer}");
    assert!(
        answer["message"]
            .as_str()
            .unwrap()
            .starts_with("Current HP not saved: it changed since this page was loaded"),
        "{answer}"
    );
    assert_eq!(answer["checks"]["passed"], "1");
    // The refusal shows the saved value next to the kept draft.
    assert_eq!(answer["unit"]["current"], "\u{201c}84\u{201d}");
    // A control character is 422.
    let response = post_json(
        &setup,
        "/private-sheets/1/fields/hp",
        &unit("1", "6\u{7}", &[]),
    )
    .await;
    assert_json_headers(&response, StatusCode::UNPROCESSABLE_ENTITY);
    let answer = json_body(response).await;
    assert_eq!(answer["status"], "invalid");
    assert_eq!(answer["revision"], "1");
    assert_eq!(saved(&setup, 1).await.0, 1);
    // Without the JSON header the same refusals re-render the page, under the strict CSP.
    for (pairs, status) in [
        (unit("5", "60", &[]), StatusCode::CONFLICT),
        (unit("1", "6\u{7}", &[]), StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        let response = send(
            &setup,
            "/private-sheets/1/fields/hp",
            &pairs,
            false,
            "application/x-www-form-urlencoded",
        )
        .await;
        assert_eq!(response.status(), status);
        assert_eq!(csp(&response), STRICT);
        assert!(
            response.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/html")
        );
    }
    // A plain form save still redirects (PRG), with the default header on the redirect.
    let response = send(
        &setup,
        "/private-sheets/1/fields/hp",
        &unit("1", "70", &[]),
        false,
        "application/x-www-form-urlencoded",
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers()[header::LOCATION],
        "/private-sheets/1#label-0"
    );
    assert_eq!(csp(&response), DEFAULT);
}

#[tokio::test]
async fn refusals_of_the_unit_routes_keep_the_default_csp() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let mut forged = unit("1", "60", &[]);
    forged[0].1 = "b".repeat(64);
    for json in [true, false] {
        for (path, pairs, content_type, status) in [
            (
                "/private-sheets/1/fields/hp",
                unit("x", "60", &[]),
                "application/x-www-form-urlencoded",
                StatusCode::BAD_REQUEST,
            ),
            (
                "/private-sheets/1/fields/nope",
                unit("1", "60", &[]),
                "application/x-www-form-urlencoded",
                StatusCode::BAD_REQUEST,
            ),
            (
                "/private-sheets/1/fields/hp",
                forged.clone(),
                "application/x-www-form-urlencoded",
                StatusCode::FORBIDDEN,
            ),
            // A multipart body never reaches the route.
            (
                "/private-sheets/1/fields/hp",
                unit("1", "60", &[]),
                "multipart/form-data; boundary=x",
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
            (
                "/private-sheets/1/cells/4/0/2",
                unit("1", "60", &[]),
                "multipart/form-data; boundary=x",
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
        ] {
            let response = send(&setup, path, &pairs, json, content_type).await;
            assert_eq!(
                response.status(),
                status,
                "{path} {content_type} json={json}"
            );
            assert_eq!(csp(&response), DEFAULT, "{path} {status}");
        }
    }
    assert_eq!(saved(&setup, 1).await.0, 1);
}

#[tokio::test]
async fn a_large_integer_round_trips_exactly_as_a_string() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let large = "9007199254740993";
    let response = post_json(
        &setup,
        "/private-sheets/1/fields/armour",
        &unit("1", large, &[]),
    )
    .await;
    assert_json_headers(&response, StatusCode::OK);
    let text = body(response).await;
    assert!(!text.contains("9007199254740992"), "{text}");
    let answer: Value = serde_json::from_str(&text).unwrap();
    assert_no_numbers(&answer, "$");
    assert_eq!(answer["unit"]["value"], large);
    assert_eq!(
        saved(&setup, 1).await.1["armour"],
        json!(9_007_199_254_740_993_i64)
    );
    // Its note and Undo carry it as text too.
    let response = post_json(
        &setup,
        "/private-sheets/1/fields/armour",
        &unit("2", "-5", &[]),
    )
    .await;
    let answer = json_body(response).await;
    assert_eq!(answer["unit"]["value"], "-5");
    assert_eq!(answer["unit"]["actions"]["undo"], large);
    // "-5" is sign-led (D-Q): the switch offers the relative reading, still exact.
    assert_eq!(
        answer["unit"]["actions"]["offer"],
        json!({"text": "9007199254740993 \u{2212} 5", "value": "9007199254740988"})
    );
}

#[tokio::test]
async fn two_tabs_a_json_save_moves_only_its_own_unit() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    // Tab B saves Armour; tab A, loaded at revision 1, saves HP.
    let response = post_json(
        &setup,
        "/private-sheets/1/fields/armour",
        &unit("1", "3", &[]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = post_json(&setup, "/private-sheets/1/fields/hp", &unit("1", "60", &[])).await;
    assert_json_headers(&response, StatusCode::OK);
    let answer = json_body(response).await;
    assert_eq!(answer["revision"], "3");
    // The answer names HP alone: no other unit's value or base is in it.
    assert_eq!(answer["unit"]["address"], "field:hp");
    assert!(!body_mentions(&answer, "armour"), "{answer}");
    // Tab A's dirty Armour still carries base 1: a conflict.
    let response = post_json(
        &setup,
        "/private-sheets/1/fields/armour",
        &unit("1", "5", &[]),
    )
    .await;
    assert_json_headers(&response, StatusCode::CONFLICT);
    let answer = json_body(response).await;
    assert_eq!(answer["status"], "conflict");
    assert_eq!(answer["revision"], "3");
    assert_eq!(saved(&setup, 1).await.1["armour"], json!(3));
}

/// Whether any string in `value` contains `needle`.
fn body_mentions(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(text) => text.contains(needle),
        Value::Array(items) => items.iter().any(|item| body_mentions(item, needle)),
        Value::Object(members) => members
            .iter()
            .any(|(key, item)| key.contains(needle) || body_mentions(item, needle)),
        _ => false,
    }
}

#[tokio::test]
async fn the_inline_style_moved_to_app_css() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let html = sheet(&setup).await;
    assert!(!html.contains(" style="));
    for template in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/templates")).unwrap() {
        let path = template.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.starts_with("private_") {
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(!text.contains("style="), "{name} has a style attribute");
        }
    }
    let css = include_str!("../../assets/app.css");
    assert!(css.contains(".sheet-fieldset { min-width: 0; }"));
}

fn module_source() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/sheet-assist.js"
    ))
    .expect("assets/sheet-assist.js")
}

/// The module builds DOM with `createElement` and `textContent` only: no HTML parsing sink,
/// no inline style, no code from strings (the CSP and Trusted Types would refuse them, and
/// this keeps them out of the source).
#[test]
fn the_module_uses_no_html_sinks_or_style_attributes() {
    let source = module_source();
    assert!(source.len() > 1000, "the module is present");
    for forbidden in [
        "innerHTML",
        "outerHTML",
        "insertAdjacentHTML",
        "document.write",
        "DOMParser",
        "createContextualFragment",
        "srcdoc",
        ".style",
        "'style'",
        "\"style\"",
        "cssText",
        "eval(",
        "Function(",
        "setTimeout('",
        "setTimeout(\"",
        "javascript:",
        "trustedTypes",
    ] {
        assert!(!source.contains(forbidden), "the module uses {forbidden}");
    }
    assert!(!source.contains("<script"));
    // No styling at all: no `style` key passed to `element`, no `style` attribute or
    // property by any spelling.
    let words = regex::Regex::new(r"\bstyle\b").unwrap();
    assert!(!words.is_match(&source), "the module names style");
    // Attribute names are literals, except in `element`, which only sets names from its
    // allowlist; the allowlist holds no event handler, style or loading attribute.
    let dynamic = regex::Regex::new(r#"setAttribute\(\s*[^'"\s]"#).unwrap();
    assert_eq!(
        dynamic.find_iter(&source).count(),
        1,
        "one dynamic setAttribute"
    );
    assert!(source.contains("if (!ATTRIBUTES.has(name)) {"));
    let start = source.find("const ATTRIBUTES = new Set([").unwrap();
    let list = &source[start..start + source[start..].find("]);").unwrap()];
    for name in list.split('\'').skip(1).step_by(2) {
        assert!(
            !name.starts_with("on")
                && !["src", "srcdoc", "data", "formaction", "style"].contains(&name),
            "the allowlist holds {name}"
        );
    }
    assert!(!source.contains("formAction") && !source.contains(".src ="));
}

/// Only exactly one `Accept: application/json` header asks for the JSON answer; anything
/// else, including a list that also names JSON, keeps the PRG redirect.
#[tokio::test]
async fn only_an_exact_json_accept_header_gets_json() {
    use tower::ServiceExt;
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let accepts: [&[&str]; 5] = [
        &["text/html"],
        &["application/json, text/html"],
        &["application/xml"],
        &["application/json", "application/json"],
        &["*/*"],
    ];
    for (n, accept) in accepts.iter().enumerate() {
        let base = (n + 1).to_string();
        let value = (60 + n).to_string();
        let mut request = axum::http::Request::builder()
            .uri("/private-sheets/1/fields/hp")
            .method("POST")
            .header("host", "127.0.0.1:3000")
            .header("origin", "http://127.0.0.1:3000")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("cookie", setup.auth[1].fixture_cookie());
        for value in *accept {
            request = request.header("accept", *value);
        }
        let body = serde_urlencoded::to_string(unit(&base, &value, &[])).unwrap();
        let response = setup
            .app
            .clone()
            .oneshot(request.body(axum::body::Body::from(body)).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{accept:?}");
    }
}

/// A cell whose row was removed since its page: the 409 answer names no route, so the
/// module can never post the entry to the row now at that index.
#[tokio::test]
async fn an_orphaned_cell_answer_names_no_route() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    super::cell_tests::three_rows(&setup).await;
    let response = send(
        &setup,
        "/private-sheets/1/lists/4/rows/0/remove",
        &vec![
            ("csrf".to_owned(), "a".repeat(64)),
            ("base".to_owned(), "1".to_owned()),
        ],
        false,
        "application/x-www-form-urlencoded",
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let response = post_json(
        &setup,
        "/private-sheets/1/cells/4/0/2",
        &unit("1", "9", &[]),
    )
    .await;
    assert_json_headers(&response, StatusCode::CONFLICT);
    let answer = json_body(response).await;
    assert_eq!(answer["unit"]["address"], "cell:equipment/r1-1/weight");
    assert_eq!(answer["unit"]["action"], Value::Null);
    assert_eq!(answer["unit"]["current"], Value::Null);
    assert!(
        answer["message"]
            .as_str()
            .unwrap()
            .contains("its row was removed"),
        "{answer}"
    );
    // Nothing was written to the row now at index 0 (Bow).
    let (revision, data) = saved(&setup, 1).await;
    assert_eq!(revision, 2);
    assert_eq!(data["equipment"][0]["weight"], json!(1));
}
