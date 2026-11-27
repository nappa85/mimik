//! Protocol fixtures (docs/api-reviewer-plan.md step 1): each enabled provider runs against a
//! local HTTPS fake server, and the recorded request body, the headers and the replies are
//! asserted. The key is a dummy; no real provider is contacted and no environment key is read.
use super::*;
use crate::Store;
use crate::assistant::download::fake_hub::{FakeHub, Reply};
use crate::assistant::download::https::{AddressPolicy, Https, ProxyMode, Roots, Transport};
use base64::Engine;
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

const KEY: &str = "dummy-key-not-a-secret";
const MODEL: &str = "reviewer-fixture-model";
/// The page's units as JSON text, the second half of every user message.
const UNITS_JSON: &str = r#"{"pdf_page":22,"units":[{"id":"u-1","kind":"paragraph","text":"A rule, mis-extracted as 1d6+3."},{"id":"u-2","kind":"table","html":"<table><tr><td>1d6</td><td>6</td></tr></table>"}]}"#;
/// The findings the good replies carry, split across text chunks where the wire allows them.
const GOOD_FINDINGS: &str = r#"{"summary":"Page 22: one table cell mis-extracted.","issues":[{"block_id":"u-2","kind":"table_cell","description":"The damage column reads 6 instead of 1d6.","suggested_correction":"1d6"}],"needs_visual_review":false}"#;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/reviewer")
}

fn tiny_png() -> Vec<u8> {
    std::fs::read(fixtures().join("page.png")).unwrap()
}

fn image_data_url() -> String {
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(tiny_png())
    )
}

#[derive(Debug)]
struct Case {
    provider: Provider,
    dir: &'static str,
    host: &'static str,
    base_path: &'static str,
    path: &'static str,
    auth_header: &'static str,
}

const MISTRAL: Case = Case {
    provider: Provider::Mistral,
    dir: "mistral",
    host: "api.mistral.ai",
    base_path: "",
    path: "/v1/chat/completions",
    auth_header: "authorization",
};
const OPENAI: Case = Case {
    provider: Provider::OpenAi,
    dir: "openai",
    host: "api.openai.com",
    base_path: "/v1",
    path: "/v1/responses",
    auth_header: "authorization",
};
const ANTHROPIC: Case = Case {
    provider: Provider::Anthropic,
    dir: "anthropic",
    host: "api.anthropic.com",
    base_path: "",
    path: "/v1/messages",
    auth_header: "x-api-key",
};
const ALL: [&Case; 3] = [&MISTRAL, &OPENAI, &ANTHROPIC];

fn transport(hub: &FakeHub, host: &str) -> Https {
    let mut overrides = HashMap::new();
    overrides.insert(host.to_owned(), hub.addr);
    let transport = Transport {
        roots: Roots::Only(vec![hub.ca.clone()]),
        addresses: AddressPolicy::Any,
        lookup: None,
        overrides,
        proxy: ProxyMode::Direct,
        connect_timeout: Duration::from_secs(5),
        read_timeout: None,
        user_agent: "mimik-reviewer-test".to_owned(),
    };
    Https::new(&transport).unwrap()
}

async fn run(case: &Case, reply_file: &str) -> (FakeHub, Result<Findings, ReviewError>) {
    let reply = std::fs::read_to_string(fixtures().join(case.dir).join(reply_file)).unwrap();
    let hub = FakeHub::start(&[case.host], move |_| Reply::json(&reply)).await;
    let base = format!(
        "https://{}:{}{}",
        case.host,
        hub.addr.port(),
        case.base_path
    );
    let reviewer =
        Reviewer::new(case.provider, &base, KEY, MODEL, transport(&hub, case.host)).unwrap();
    let result = reviewer.review_page(&tiny_png(), UNITS_JSON).await;
    (hub, result)
}

fn expected_findings() -> Findings {
    serde_json::from_str(GOOD_FINDINGS).unwrap()
}

/// The recorded request body, as parsed JSON.
fn fixture_request(case: &Case) -> serde_json::Value {
    let text = std::fs::read_to_string(fixtures().join(case.dir).join("request.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// Runs one provider against its good reply and asserts the recorded request, the headers and
/// the parsed findings.
async fn assert_good(case: &Case) {
    let (hub, result) = run(case, "reply-good.json").await;
    assert_eq!(result.as_ref().ok(), Some(&expected_findings()), "{case:?}");
    let seen = hub.requests();
    assert_eq!(seen.len(), 1, "one request per page: {seen:?}");
    let request = &seen[0];
    assert_eq!(request.path, case.path, "{case:?}");
    assert_eq!(request.host, case.host, "{case:?}");
    // The key is carried only by the provider's auth header, nowhere else.
    let auth = request
        .headers
        .get(case.auth_header)
        .and_then(|value| value.to_str().ok());
    let expected_auth = match case.provider {
        Provider::Anthropic => KEY.to_owned(),
        _ => format!("Bearer {KEY}"),
    };
    assert_eq!(auth, Some(expected_auth.as_str()), "{case:?}");
    for (name, value) in &request.headers {
        let value = value.to_str().unwrap_or("");
        if name.as_str() != case.auth_header {
            assert!(!value.contains(KEY), "{case:?}: {name} carries the key");
        }
    }
    if case.provider == Provider::Anthropic {
        assert!(request.headers.get("authorization").is_none(), "{case:?}");
        assert!(
            request.headers.get("anthropic-version").is_some(),
            "{case:?}"
        );
    }
    // The recorded request body matches as parsed JSON, image data included exactly once.
    let actual: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(actual, fixture_request(case), "{case:?}");
    let body_text = String::from_utf8_lossy(&request.body);
    // The image goes out as base64, once: a data URL on the OpenAI-shaped wires, raw base64 on
    // Anthropic's.
    let image_token = match case.provider {
        Provider::Anthropic => base64::engine::general_purpose::STANDARD.encode(tiny_png()),
        _ => image_data_url(),
    };
    assert_eq!(body_text.matches(&image_token).count(), 1, "{case:?}");
    // The fixed instruction is the preamble, and the units are the text part, exactly.
    let parts: Vec<&serde_json::Value> = match case.provider {
        Provider::Anthropic => actual["messages"][0]["content"]
            .as_array()
            .unwrap()
            .iter()
            .collect(),
        Provider::Mistral => actual["messages"][1]["content"]
            .as_array()
            .unwrap()
            .iter()
            .collect(),
        Provider::OpenAi => actual["input"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|item| item["content"].as_array().unwrap().iter())
            .collect(),
    };
    let text = parts
        .iter()
        .find(|part| matches!(part["type"].as_str(), Some("text" | "input_text")))
        .unwrap();
    assert_eq!(text["text"].as_str().unwrap(), UNITS_JSON, "{case:?}");
    let preamble = match case.provider {
        Provider::OpenAi => actual["instructions"].as_str().unwrap(),
        Provider::Mistral => actual["messages"][0]["content"][0]["text"]
            .as_str()
            .unwrap(),
        Provider::Anthropic => actual["system"][0]["text"].as_str().unwrap(),
    };
    assert_eq!(preamble, prompt::INSTRUCTION, "{case:?}");
}

#[tokio::test]
async fn reviewer_mistral_request_matches_the_fixture_and_the_good_reply_parses() {
    assert_good(&MISTRAL).await;
    let (hub, _) = run(&MISTRAL, "reply-good.json").await;
    let body: serde_json::Value = serde_json::from_slice(&hub.requests()[0].body).unwrap();
    assert_eq!(body["response_format"]["type"], "json_schema", "{body}");
    assert_eq!(
        body["response_format"]["json_schema"]["strict"], true,
        "{body}"
    );
    let image = &body["messages"][1]["content"][1];
    assert_eq!(image["type"], "image_url", "{image}");
    assert_eq!(
        image["image_url"]["url"].as_str().unwrap(),
        image_data_url(),
        "the image is the base64 data URL, compared exactly"
    );
}

#[tokio::test]
async fn reviewer_openai_request_matches_the_fixture_and_always_carries_store_false() {
    assert_good(&OPENAI).await;
    let (hub, _) = run(&OPENAI, "reply-good.json").await;
    let body: serde_json::Value = serde_json::from_slice(&hub.requests()[0].body).unwrap();
    assert_eq!(
        body["store"], false,
        "every OpenAI request carries store: false: {body}"
    );
    assert_eq!(body["text"]["format"]["type"], "json_schema", "{body}");
    assert_eq!(body["text"]["format"]["strict"], true, "{body}");
    let image = body["input"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|item| item["content"].as_array().unwrap())
        .find(|part| part["type"] == "input_image")
        .unwrap();
    assert_eq!(
        image["image_url"].as_str().unwrap(),
        image_data_url(),
        "the image is the base64 data URL, compared exactly"
    );
}

#[tokio::test]
async fn reviewer_anthropic_request_matches_the_fixture_and_the_good_reply_parses() {
    assert_good(&ANTHROPIC).await;
    let (hub, _) = run(&ANTHROPIC, "reply-good.json").await;
    let body: serde_json::Value = serde_json::from_slice(&hub.requests()[0].body).unwrap();
    assert_eq!(
        body["output_config"]["format"]["type"], "json_schema",
        "{body}"
    );
    assert!(body["max_tokens"].as_u64().is_some(), "{body}");
    let system = body["system"].as_array().unwrap();
    assert!(
        system.iter().any(|part| part["text"]
            .as_str()
            .is_some_and(|text| text.contains("untrusted document data"))),
        "{body}"
    );
    let image = &body["messages"][0]["content"][1];
    assert_eq!(image["type"], "image", "{image}");
    assert_eq!(image["source"]["type"], "base64", "{image}");
    assert_eq!(image["source"]["media_type"], "image/png", "{image}");
    assert_eq!(
        image["source"]["data"].as_str().unwrap(),
        base64::engine::general_purpose::STANDARD.encode(tiny_png()),
        "the image is the base64 data, compared exactly"
    );
}

#[tokio::test]
async fn reviewer_refusal_replies_fail_the_page_with_the_fixed_message() {
    for case in ALL {
        let (_hub, result) = run(case, "reply-refusal.json").await;
        assert_eq!(result, Err(ReviewError::InvalidReply), "{case:?}");
        assert_eq!(
            result.unwrap_err().to_string(),
            "the model's reply was not valid findings JSON (a refusal lands here too)",
            "{case:?}"
        );
    }
}

/// Anthropic's `stop_reason: "refusal"` fixtures: with empty content and with a short text, the
/// page fails the same fixed way (rig folds a refusal into the answer text, so it cannot be told
/// apart from any non-JSON reply and is never shown).
#[tokio::test]
async fn reviewer_anthropic_refusal_stop_reason_fails_the_page_with_the_fixed_message() {
    for reply in ["reply-refusal-empty.json", "reply-refusal-text.json"] {
        let (_hub, result) = run(&ANTHROPIC, reply).await;
        assert_eq!(result, Err(ReviewError::InvalidReply), "{reply}");
        assert_eq!(
            result.unwrap_err().to_string(),
            "the model's reply was not valid findings JSON (a refusal lands here too)",
            "{reply}"
        );
    }
}

#[tokio::test]
async fn reviewer_non_json_replies_fail_the_page_with_the_fixed_message() {
    for case in ALL {
        let (_hub, result) = run(case, "reply-nonjson.json").await;
        assert_eq!(result, Err(ReviewError::InvalidReply), "{case:?}");
    }
}

/// `build` (step 4's entry): no reviewer until a key and a model are set, then one over the
/// shipped transport; a stored base URL that is refused again at build time fails with a fixed
/// message. No request is sent here.
#[tokio::test]
async fn build_returns_none_until_a_key_and_a_model_are_set_then_a_reviewer() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("private.sqlite3")).unwrap();
    assert!(build(&store).await.unwrap().is_none(), "no key, no model");
    store.secrets().write(KEY.as_bytes()).unwrap();
    assert!(build(&store).await.unwrap().is_none(), "no model yet");
    let valid = settings::validate("mistral", "https://api.mistral.ai", MODEL).unwrap();
    store
        .run(move |db| settings::Settings::save(db, 1, &valid))
        .await
        .unwrap();
    assert!(
        build(&store).await.unwrap().is_some(),
        "key and model set: a reviewer is built"
    );
    // A stored base URL the application refuses is refused again at build time.
    store
        .run(|db| {
            db.execute(
                "UPDATE reviewer_settings SET base_url='https://127.0.0.1'",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let error = match build(&store).await {
        Err(error) => error.to_string(),
        Ok(_) => panic!("a refused stored base URL fails the build"),
    };
    assert!(error.contains("domain"), "{error}");
    assert!(!error.contains(KEY), "{error}");
}

/// One key rule for readiness and sending: the stored bytes must themselves satisfy the save
/// rule, with no trimming at read time — a 0600 file holding `key\n` is not ready, and `build`
/// refuses the same bytes with a fixed message that never carries the key.
#[tokio::test]
async fn build_refuses_a_stored_key_that_breaks_the_save_rule() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("private.sqlite3")).unwrap();
    let valid = settings::validate("mistral", "https://api.mistral.ai", MODEL).unwrap();
    store
        .run(move |db| settings::Settings::save(db, 1, &valid))
        .await
        .unwrap();
    let key_path = dir.path().join(key::DIRECTORY).join(key::KEY_FILE);
    for stored in [format!("{KEY}\n"), format!(" {KEY}"), format!("{KEY}\r\n")] {
        std::fs::write(&key_path, &stored).unwrap();
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(!store.secrets().ready().unwrap(), "{stored:?}: not ready");
        let error = match build(&store).await {
            Err(error) => error.to_string(),
            Ok(_) => panic!("a stored key that breaks the save rule refuses the build: {stored:?}"),
        };
        assert!(
            error.contains("does not hold a valid key"),
            "{stored:?}: {error}"
        );
        assert!(!error.contains(KEY), "{stored:?}: {error}");
    }
}

#[test]
fn reviewer_findings_schema_is_strict_compatible() {
    let schema = findings::schema_value();
    let object = schema.as_object().unwrap();
    assert_eq!(object["additionalProperties"], false);
    assert_eq!(
        object["required"],
        serde_json::json!(["summary", "issues", "needs_visual_review"])
    );
    let issue = &schema["$defs"]["issue"];
    assert_eq!(issue["additionalProperties"], false);
    assert_eq!(
        issue["required"],
        serde_json::json!(["block_id", "kind", "description", "suggested_correction"])
    );
    assert_eq!(
        issue["properties"]["block_id"]["type"],
        serde_json::json!(["string", "null"])
    );
    assert_eq!(
        issue["properties"]["suggested_correction"]["type"],
        serde_json::json!(["string", "null"])
    );
    // No length or count bounds are sent; they stay in validate_page (step 6).
    let text = schema.to_string();
    for keyword in [
        "maxLength",
        "minLength",
        "maxItems",
        "minItems",
        "minimum",
        "maximum",
    ] {
        assert!(!text.contains(keyword), "{keyword} is not sent: {text}");
    }
    // rig accepts the schema.
    let _ = findings::schema();
}

#[test]
fn reviewer_findings_parse_rejects_extra_and_missing_keys() {
    assert!(findings::parse(GOOD_FINDINGS).is_ok());
    assert_eq!(
        findings::parse(r#"{"summary":"s","issues":[],"needs_visual_review":false,"extra":1}"#),
        Err(ReviewError::InvalidReply)
    );
    assert_eq!(
        findings::parse(r#"{"summary":"s","needs_visual_review":false}"#),
        Err(ReviewError::InvalidReply)
    );
}
