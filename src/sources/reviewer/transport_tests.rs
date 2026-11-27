//! Transport tests (docs/api-reviewer-plan.md step 2) against a local HTTPS fake with test
//! roots: redirect refusal, the public-only resolver, refused base URLs (at build time and
//! before each send), no proxy under `HTTPS_PROXY`, the 4 MiB body bound plain and streamed,
//! the page budget, and the fixed status messages. The process-wide environment is read once
//! per process, so the proxy case runs in a child process like the download tests'.
use super::*;
use crate::assistant::download::fake_hub::{FakeHub, Reply};
use crate::assistant::download::https::{
    self, AddressPolicy, BODY_LIMIT, Https, PAGE_BUDGET, ProxyMode, Roots, Transport,
};
use futures_util::StreamExt;
use rig_core::http_client::HttpClientExt;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;
use url::Url;

const KEY: &str = "dummy-key-not-a-secret";
const MODEL: &str = "reviewer-transport-model";
const HOST: &str = "provider.example";

/// The shipped transport settings of `Transport::reviewer()`: no proxy, public addresses only,
/// the [`https::CONNECT_TIMEOUT`] connect timeout and the platform roots.
#[test]
fn the_shipped_reviewer_transport_settings_are_the_reviewed_ones() {
    let transport = Transport::reviewer();
    assert!(
        matches!(transport.proxy, ProxyMode::Direct),
        "the reviewer sends no proxy"
    );
    assert_eq!(
        transport.addresses,
        AddressPolicy::Public,
        "public addresses only"
    );
    assert_eq!(transport.connect_timeout, https::CONNECT_TIMEOUT);
    assert!(
        matches!(transport.roots, Roots::Platform),
        "the platform trust store"
    );
}

fn transport(hub: &FakeHub) -> Https {
    let mut overrides = HashMap::new();
    overrides.insert(HOST.to_owned(), hub.addr);
    Https::new(&Transport {
        roots: Roots::Only(vec![hub.ca.clone()]),
        addresses: AddressPolicy::Any,
        lookup: None,
        overrides,
        proxy: ProxyMode::Direct,
        connect_timeout: Duration::from_secs(5),
        read_timeout: None,
        user_agent: "mimik-reviewer-test".to_owned(),
    })
    .unwrap()
}

fn base(hub: &FakeHub) -> String {
    format!("https://{HOST}:{}", hub.addr.port())
}

fn reviewer(hub: &FakeHub) -> Reviewer {
    Reviewer::new(Provider::Mistral, &base(hub), KEY, MODEL, transport(hub)).unwrap()
}

fn image() -> Vec<u8> {
    super::TEST_PAGE_PNG.to_vec()
}

#[tokio::test]
async fn reviewer_transport_refuses_a_redirect_and_sends_one_request() {
    let hub = FakeHub::start(&[HOST], |_| {
        Reply::redirect("https://elsewhere.example:1/x")
    })
    .await;
    let client = transport(&hub);
    let url = Url::parse(&format!("{}/v1/chat/completions", base(&hub))).unwrap();
    let result = client
        .request(http::Method::POST, &url, bytes::Bytes::from_static(b"{}"))
        .await;
    assert_eq!(result, Err(https::Error::Redirect));
    assert_eq!(hub.requests().len(), 1, "the redirect was not followed");
    // Through rig as well: the page fails with the fixed redirect message.
    let result = reviewer(&hub).review_page(&image(), "{}").await;
    assert_eq!(result, Err(ReviewError::Redirect));
    assert_eq!(
        result.unwrap_err().to_string(),
        "the provider answered with a redirect, which is refused"
    );
    assert_eq!(hub.requests().len(), 2);
}

#[tokio::test]
async fn reviewer_transport_refuses_a_base_url_that_resolves_to_loopback_or_private() {
    let hub = FakeHub::start(&[HOST], |_| Reply::json("{}")).await;
    // A resolver that answers the loopback for every name; the transport is the shipped one
    // (`Transport::reviewer()`) with only the roots and the test resolver replaced, so the
    // public-only address policy, the no-proxy mode and the connect timeout stay shipped.
    let lookup: https::Lookup = Arc::new(|_host: String| {
        Box::pin(async move { Ok(vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]) })
    });
    let mut transport = Transport::reviewer();
    transport.roots = Roots::Only(vec![hub.ca.clone()]);
    transport.lookup = Some(lookup);
    let client = Https::new(&transport).unwrap();
    let url = Url::parse(&format!(
        "https://{HOST}:{}/v1/chat/completions",
        hub.addr.port()
    ))
    .unwrap();
    let result = client
        .request(http::Method::POST, &url, bytes::Bytes::from_static(b"{}"))
        .await;
    assert_eq!(result, Err(https::Error::Address));
    assert_eq!(hub.requests().len(), 0, "nothing was sent");
}

#[test]
fn reviewer_transport_refuses_ip_literal_and_userinfo_base_urls_at_build_time() {
    for bad in [
        "https://127.0.0.1/v1",
        "https://127.1/",
        "https://0x7f.1/",
        "https://2130706433/",
        "https://[::1]/",
        "https://[2001:db8::1]:8443/",
        "https://user@api.example.com/",
        "https://user:secret@api.example.com/v1",
        "http://api.example.com/",
        "https://",
        "not a url",
    ] {
        assert!(
            https::validate_base_url(bad).is_err(),
            "{bad} must be refused"
        );
    }
    for good in [
        "https://api.mistral.ai",
        "https://api.openai.com/v1",
        "https://api.anthropic.com",
        "https://api.example.com:8443/v1",
    ] {
        assert!(
            https::validate_base_url(good).is_ok(),
            "{good} must be accepted"
        );
    }
    // And the reviewer's own constructor refuses them too.
    let plain_client = || {
        Https::new(&Transport {
            roots: Roots::Platform,
            addresses: AddressPolicy::Public,
            lookup: None,
            overrides: HashMap::new(),
            proxy: ProxyMode::Direct,
            connect_timeout: https::CONNECT_TIMEOUT,
            read_timeout: None,
            user_agent: "mimik-reviewer-test".to_owned(),
        })
        .unwrap()
    };
    let client = plain_client();
    for bad in ["https://127.0.0.1/v1", "https://user@api.example.com/"] {
        assert!(matches!(
            Reviewer::new(Provider::Mistral, bad, KEY, MODEL, client.clone()),
            Err(ReviewError::Base(_))
        ));
    }
}

#[tokio::test]
async fn reviewer_transport_refuses_a_refused_url_again_before_each_send() {
    let hub = FakeHub::start(&[HOST], |_| Reply::json("{}")).await;
    let client = transport(&hub);
    // A request rig could only build from a refused base: the transport refuses it itself.
    for raw in [
        "https://127.0.0.1:9/v1/chat/completions",
        "https://[::1]:9/v1/chat/completions",
        "https://2130706433:9/v1/chat/completions",
        "https://user@provider.example/v1/chat/completions",
        "http://provider.example/v1/chat/completions",
    ] {
        let request = http::Request::post(raw)
            .body(bytes::Bytes::from_static(b"{}"))
            .unwrap();
        let Err(error) = client.send(request).await else {
            panic!("{raw} must be refused before the send");
        };
        assert!(
            error.to_string().contains("base URL is refused"),
            "{raw}: {error}"
        );
    }
    assert_eq!(hub.requests().len(), 0, "nothing was sent");
}

/// Runs the `#[ignore]`d child test `name` of this binary with `env`, returning whether it
/// passed (the pattern of the download tests' `net_tests`).
async fn child(name: &str, env: Vec<(&'static str, String)>) -> bool {
    let name = format!("sources::reviewer::transport_tests::{name}");
    tokio::task::spawn_blocking(move || {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                &name,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env_remove("HTTPS_PROXY")
            .env_remove("https_proxy")
            .env_remove("ALL_PROXY")
            .env_remove("all_proxy")
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .envs(env);
        let output = command.output().unwrap();
        if !output.status.success() {
            eprintln!("{}", String::from_utf8_lossy(&output.stdout));
            eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        }
        output.status.success()
    })
    .await
    .unwrap()
}

fn env_of(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

/// The child's client: the shipped transport (`Transport::reviewer()`) with only the roots and
/// the test resolver's fixed address replaced, so the no-proxy mode, the public-only address
/// policy and the connect timeout stay shipped.
fn child_client() -> Https {
    let addr: std::net::SocketAddr = env_of("MIMIK_REVIEWER_CHILD_ADDR").parse().unwrap();
    let mut transport = Transport::reviewer();
    transport.roots = Roots::Only(vec![rustls::pki_types::CertificateDer::from(
        std::fs::read(env_of("MIMIK_REVIEWER_CHILD_CA")).unwrap(),
    )]);
    transport.overrides.insert(HOST.to_owned(), addr);
    Https::new(&transport).unwrap()
}

#[tokio::test]
#[ignore = "run in a child process by the parent test"]
async fn child_no_proxy_with_https_proxy_set() {
    let client = child_client();
    let addr: std::net::SocketAddr = env_of("MIMIK_REVIEWER_CHILD_ADDR").parse().unwrap();
    let url = Url::parse(&format!(
        "https://{HOST}:{}/v1/chat/completions",
        addr.port()
    ))
    .unwrap();
    client
        .request(http::Method::POST, &url, bytes::Bytes::from_static(b"{}"))
        .await
        .unwrap();
}

#[tokio::test]
async fn reviewer_transport_uses_no_proxy_with_https_proxy_set() {
    let hub = FakeHub::start(&[HOST], |_| Reply::json("{}")).await;
    let dir = tempfile::tempdir().unwrap();
    let ca = dir.path().join("ca.der");
    std::fs::write(&ca, hub.ca.as_ref()).unwrap();
    // The proxy address points at a closed port: if the client used it, the request would fail.
    assert!(
        child(
            "child_no_proxy_with_https_proxy_set",
            vec![
                ("MIMIK_REVIEWER_CHILD_ADDR", hub.addr.to_string()),
                ("MIMIK_REVIEWER_CHILD_CA", ca.display().to_string()),
                ("HTTPS_PROXY", "http://127.0.0.1:9/".to_owned()),
            ],
        )
        .await
    );
    assert_eq!(
        hub.requests().len(),
        1,
        "the request went direct, not via the proxy"
    );
}

#[tokio::test]
async fn reviewer_transport_refuses_an_oversized_body_plain_and_streamed() {
    let big = vec![b'x'; BODY_LIMIT + 1];
    // Declared length.
    let hub = FakeHub::start(&[HOST], {
        let big = big.clone();
        move |_| Reply::Full {
            status: 200,
            headers: vec![("content-type", "application/json".into())],
            body: big.clone(),
        }
    })
    .await;
    let client = transport(&hub);
    let url = Url::parse(&format!("{}/v1/chat/completions", base(&hub))).unwrap();
    let result = client
        .request(http::Method::POST, &url, bytes::Bytes::from_static(b"{}"))
        .await;
    assert_eq!(result, Err(https::Error::TooLarge));
    // Chunked, no declared length.
    hub.respond({
        let big = big.clone();
        move |_| Reply::Chunked {
            status: 200,
            headers: vec![("content-type", "application/json".into())],
            body: big.clone(),
            piece: 65_536,
            gap: Duration::ZERO,
        }
    });
    let result = client
        .request(http::Method::POST, &url, bytes::Bytes::from_static(b"{}"))
        .await;
    assert_eq!(result, Err(https::Error::TooLarge));
    // Streamed: the bound is read through.
    hub.respond({
        let big = big.clone();
        move |_| Reply::Chunked {
            status: 200,
            headers: vec![("content-type", "application/json".into())],
            body: big.clone(),
            piece: 65_536,
            gap: Duration::ZERO,
        }
    });
    let request = http::Request::post(url.as_str())
        .body(bytes::Bytes::from_static(b"{}"))
        .unwrap();
    let response = client.send_streaming(request).await.unwrap();
    assert_eq!(response.status(), http::StatusCode::OK);
    let mut stream = response.into_body();
    let mut outcome = Ok(());
    while let Some(chunk) = stream.next().await {
        if let Err(error) = chunk {
            outcome = Err(error);
            break;
        }
    }
    let error = outcome.unwrap_err();
    assert!(
        error.to_string().contains("4 MiB"),
        "the streamed body is refused through the bound: {error}"
    );
    // A body within the bound passes, plain and streamed.
    hub.respond(|_| Reply::json("{\"ok\":true}"));
    let result = client
        .request(http::Method::POST, &url, bytes::Bytes::from_static(b"{}"))
        .await;
    assert_eq!(result.as_deref(), Ok(&b"{\"ok\":true}"[..]));
}

/// After the bound is crossed the stream yields the error and then ends: it does not keep
/// reading the rest of the body.
#[tokio::test]
async fn a_streamed_error_ends_the_stream_instead_of_reading_on() {
    let piece = 1 << 20;
    // Four pieces fit the bound, the fifth crosses it, and a sixth would follow.
    let body = vec![b'x'; BODY_LIMIT + 2 * piece];
    let hub = FakeHub::start(&[HOST], {
        let body = body.clone();
        move |_| Reply::Chunked {
            status: 200,
            headers: vec![("content-type", "application/json".into())],
            body: body.clone(),
            piece,
            gap: Duration::ZERO,
        }
    })
    .await;
    let client = transport(&hub);
    let url = Url::parse(&format!("{}/v1/chat/completions", base(&hub))).unwrap();
    let request = http::Request::post(url.as_str())
        .body(bytes::Bytes::from_static(b"{}"))
        .unwrap();
    let response = client.send_streaming(request).await.unwrap();
    assert_eq!(response.status(), http::StatusCode::OK);
    let mut stream = response.into_body();
    let mut within = 0_usize;
    let error = loop {
        match stream.next().await {
            Some(Ok(chunk)) => {
                within += chunk.len();
                assert!(
                    within <= BODY_LIMIT,
                    "the bound is crossed, not skipped over"
                );
            }
            Some(Err(error)) => break error,
            None => panic!("the stream ended without crossing the bound"),
        }
    };
    assert!(error.to_string().contains("4 MiB"), "{error}");
    // Polling past the error: the stream has ended, no further item (the fake still has
    // about 2 MiB of body to send) and no hang.
    let after = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("the stream ends after the error instead of hanging");
    assert!(after.is_none(), "no item follows the error");
}

#[tokio::test]
async fn reviewer_transport_page_budget_expires_on_a_stalled_reply() {
    assert_eq!(
        PAGE_BUDGET,
        Duration::from_secs(300),
        "the exposed per-page budget"
    );
    let hub = FakeHub::start(&[HOST], |_| Reply::Stall {
        status: 200,
        headers: vec![("content-type", "application/json".into())],
        first: b"{\"partial\"".to_vec(),
        declared: None,
    })
    .await;
    let reviewer = reviewer(&hub);
    let result = reviewer
        .review_page_within(Duration::from_millis(300), &image(), "{}")
        .await;
    assert_eq!(result, Err(ReviewError::Budget));
    assert_eq!(
        result.unwrap_err().to_string(),
        "the page exceeded its 300 s budget"
    );
}

#[tokio::test]
async fn reviewer_transport_maps_statuses_to_fixed_messages_without_key_or_body() {
    for status in [401_u16, 403, 404, 413, 429, 500, 503, 418] {
        let hub = FakeHub::start(&[HOST], move |_| Reply::Full {
            status,
            headers: vec![("content-type", "application/json".into())],
            body: format!("{{\"error\":{{\"message\":\"provider secret body {KEY}\"}}}}")
                .into_bytes(),
        })
        .await;
        let result = reviewer(&hub).review_page(&image(), "{}").await;
        assert_eq!(result, Err(ReviewError::Status(status)), "{status}");
        let message = result.unwrap_err().to_string();
        assert_eq!(message, https::status_message(status), "{status}");
        assert!(!message.contains(KEY), "{status}: {message}");
        assert!(
            !message.contains("provider secret body"),
            "{status}: {message}"
        );
    }
    // 401's fixed message, spelled out.
    let hub = FakeHub::start(&[HOST], |_| Reply::status(401)).await;
    let result = reviewer(&hub).review_page(&image(), "{}").await;
    assert_eq!(
        result.unwrap_err().to_string(),
        "the provider rejected the API key (HTTP 401/403)"
    );
}
