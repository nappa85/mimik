use super::super::fake_server::{self, Reply};
use super::super::interpreter::{Completion, Failure, Interpreter, RigInterpreter};
use super::*;
use std::sync::atomic::Ordering;

#[test]
fn tcp_endpoint_guard() {
    for refused in [
        "http://example.com:8080",
        "http://localhost:8080",
        "http://127.1:8080",
        "http://0x7f.1:8080",
        "http://2130706433:8080",
        "http://[::ffff:127.0.0.1]:8080",
        "http://user@127.0.0.1:8080",
        "http://user:pw@127.0.0.1:8080",
        "http://[::1%25lo]:8080",
        "https://127.0.0.1:8080",
        "http://127.0.0.1",
        "http://127.0.0.1:",
        "http://127.0.0.1:0",
        "http://127.0.0.1:08080",
        "http://127.0.0.1:65536",
        "http://127.0.0.1:8080/v1",
        "http://127.0.0.1:8080/?x=1",
        "http://127.0.0.1:8080#x",
        "http://127.0.0.2:8080",
        "http://0.0.0.0:8080",
        "ftp://127.0.0.1:8080",
    ] {
        assert!(tcp(refused, true).is_err(), "{refused}");
    }
    assert!(tcp("http://[::1]:8080", false).is_err());
    assert!(tcp("http://[::1]:8080", true).is_ok());
    assert!(tcp("http://127.0.0.1:8080", false).is_ok());
    assert!(tcp("http://127.0.0.1:8080/", false).is_ok());
}

fn interpreter(endpoint: Endpoint) -> RigInterpreter {
    RigInterpreter::new(
        endpoint,
        "dummy-token-not-a-secret".into(),
        "granite-test".into(),
    )
    .unwrap()
}

fn completion() -> Completion {
    Completion {
        dynamic: "<data>\n{}\n</data>".into(),
        max_tokens: 192,
    }
}

async fn tcp_interpreter() -> (RigInterpreter, std::sync::Arc<fake_server::Shared>) {
    let (url, shared) = fake_server::tcp().await;
    (
        interpreter(Endpoint::Tcp(tcp(&url, false).unwrap())),
        shared,
    )
}

#[tokio::test]
async fn a_completion_carries_the_prefix_schema_and_token() {
    let (client, shared) = tcp_interpreter().await;
    shared.push(Reply::Content(r#"{"kind":"options","options":[]}"#.into()));
    let completed = client.complete(&completion()).await.unwrap();
    assert_eq!(completed.text, r#"{"kind":"options","options":[]}"#);
    assert_eq!(completed.cached_tokens, Some(1290));
    let seen = shared.seen();
    assert_eq!(seen.len(), 1);
    let raw = std::str::from_utf8(&seen[0].body).unwrap();
    // The schema arrives byte-exact.
    assert!(raw.contains(super::super::contract::SCHEMA_TEXT.trim_end()));
    let body: serde_json::Value = serde_json::from_slice(&seen[0].body).unwrap();
    assert_eq!(body["stream"], true);
    assert_eq!(body["cache_prompt"], true);
    assert_eq!(body["top_k"], 1);
    assert_eq!(body["seed"], 0);
    assert_eq!(body["temperature"], 0.0);
    assert_eq!(body["max_tokens"], 192);
    assert_eq!(body["model"], "granite-test");
    assert_eq!(body["response_format"]["type"], "json_schema");
    assert_eq!(
        body["response_format"]["json_schema"]["name"],
        "mimik_interpretation_v2"
    );
    assert_eq!(body["messages"][0]["role"], "system");
    // rig sends message content as one text part; llama-server joins the parts.
    assert_eq!(
        body["messages"][0]["content"],
        serde_json::json!([{"type": "text", "text": super::super::contract::FIXED_PREFIX.as_str()}])
    );
    assert_eq!(body["messages"][1]["role"], "user");
    let user = &body["messages"][1]["content"];
    let user = user.as_str().or_else(|| user[0]["text"].as_str()).unwrap();
    assert_eq!(user, "<data>\n{}\n</data>");
    let auth = seen[0].headers.get("authorization").unwrap();
    assert_eq!(auth, "Bearer dummy-token-not-a-secret");
    assert!(seen[0].headers.get("accept-encoding").is_none());
}

#[tokio::test]
async fn tokenize_counts_the_server_tokens() {
    let (client, shared) = tcp_interpreter().await;
    *shared.tokens.lock().unwrap() = Some(17);
    assert_eq!(client.tokens("abc").await, Ok(17));
    let seen = shared.seen();
    assert_eq!(seen[0].path, "/tokenize");
    assert_eq!(
        seen[0].headers.get("authorization").unwrap(),
        "Bearer dummy-token-not-a-secret"
    );
}

#[tokio::test]
async fn one_request_per_call_on_500_and_429() {
    let (client, shared) = tcp_interpreter().await;
    shared.push(Reply::Status(500));
    assert_eq!(
        client.complete(&completion()).await,
        Err(Failure::Status(500))
    );
    assert_eq!(shared.completions(), 1);
    shared.push(Reply::Status(429));
    assert_eq!(
        client.complete(&completion()).await,
        Err(Failure::Status(429))
    );
    assert_eq!(shared.completions(), 2);
}

#[tokio::test]
async fn a_redirect_is_never_followed() {
    let (client, shared) = tcp_interpreter().await;
    let (trap, count) = fake_server::trap().await;
    shared.push(Reply::Redirect(format!(
        "http://{trap}/v1/chat/completions"
    )));
    assert!(client.complete(&completion()).await.is_err());
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert_eq!(shared.completions(), 1);
}

#[tokio::test]
async fn a_gzip_reply_is_not_decoded() {
    let (client, shared) = tcp_interpreter().await;
    shared.push(Reply::Gzip);
    assert_eq!(
        client.complete(&completion()).await,
        Err(Failure::Transport)
    );
}

#[tokio::test]
async fn bodies_over_the_bound_are_refused() {
    let (client, shared) = tcp_interpreter().await;
    shared.push(Reply::Huge(2 * BODY_LIMIT));
    assert_eq!(
        client.complete(&completion()).await,
        Err(Failure::Transport)
    );
    shared.push(Reply::Endless);
    assert_eq!(
        client.complete(&completion()).await,
        Err(Failure::Transport)
    );
}

#[tokio::test]
async fn a_plain_body_over_the_bound_is_refused_without_buffering() {
    let (url, shared) = fake_server::tcp().await;
    shared.push(Reply::Huge(2 * BODY_LIMIT));
    let transport = Bounded::new(Endpoint::Tcp(tcp(&url, false).unwrap())).unwrap();
    let request = http::Request::post(format!("{url}/v1/chat/completions"))
        .body(Bytes::from_static(b"{}"))
        .unwrap();
    let error = transport.send::<_, Bytes>(request).await.err().unwrap();
    assert!(matches!(error, http_client::Error::Instance(_)), "{error}");
    assert!(error.to_string().contains("over the bound"), "{error}");
}

#[tokio::test]
async fn a_malformed_stream_is_a_failure() {
    let (client, shared) = tcp_interpreter().await;
    shared.push(Reply::Nonsense);
    let result = client.complete(&completion()).await;
    // Either no well-formed completion or an empty one; never a reply with content.
    assert!(
        result.as_ref().map_or(true, |done| done.text.is_empty()),
        "{result:?}"
    );
}

#[tokio::test]
async fn a_request_for_another_destination_is_refused() {
    let (url, shared) = fake_server::tcp().await;
    let (other, other_shared) = fake_server::tcp().await;
    let transport = Bounded::new(Endpoint::Tcp(tcp(&url, false).unwrap())).unwrap();
    let request = http::Request::post(format!("{other}/v1/chat/completions"))
        .body(Bytes::from_static(b"{}"))
        .unwrap();
    assert!(transport.send::<_, Bytes>(request).await.is_err());
    assert!(other_shared.seen().is_empty());
    assert!(shared.seen().is_empty());
}

#[tokio::test]
async fn dropping_a_streamed_call_closes_the_connection() {
    let (client, shared) = tcp_interpreter().await;
    shared.push(Reply::Slow(
        std::time::Duration::from_millis(100),
        "x".repeat(4000),
    ));
    let request = completion();
    let call = client.complete(&request);
    let _ = tokio::time::timeout(std::time::Duration::from_millis(400), call).await;
    for _ in 0..100 {
        if shared.disconnected.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(shared.disconnected.load(Ordering::SeqCst), 1);
    assert_eq!(shared.open.load(Ordering::SeqCst), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn the_unix_socket_connector_keeps_the_policy() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("llama.sock");
    let other = directory.path().join("other.sock");
    let shared = fake_server::unix(&socket);
    let other_shared = fake_server::unix(&other);
    let client = interpreter(Endpoint::Unix(socket.clone()));
    shared.push(Reply::Content(r#"{"kind":"options","options":[]}"#.into()));
    let completed = client.complete(&completion()).await.unwrap();
    assert_eq!(completed.text, r#"{"kind":"options","options":[]}"#);
    let seen = shared.seen();
    assert_eq!(seen.len(), 1);
    assert!(
        std::str::from_utf8(&seen[0].body)
            .unwrap()
            .contains(super::super::contract::SCHEMA_TEXT.trim_end())
    );
    assert_eq!(
        seen[0].headers.get("authorization").unwrap(),
        "Bearer dummy-token-not-a-secret"
    );
    // Nothing reaches another socket.
    assert!(other_shared.seen().is_empty());
    shared.push(Reply::Status(500));
    assert_eq!(
        client.complete(&completion()).await,
        Err(Failure::Status(500))
    );
    assert_eq!(shared.completions(), 2);
    let (trap, count) = fake_server::trap().await;
    shared.push(Reply::Redirect(format!("http://{trap}/")));
    assert!(client.complete(&completion()).await.is_err());
    assert_eq!(count.load(Ordering::SeqCst), 0);
    shared.push(Reply::Endless);
    assert_eq!(
        client.complete(&completion()).await,
        Err(Failure::Transport)
    );
    // A transport for a missing socket path reaches nothing.
    let missing = interpreter(Endpoint::Unix(directory.path().join("missing.sock")));
    assert_eq!(
        missing.complete(&completion()).await,
        Err(Failure::Transport)
    );
    assert!(other_shared.seen().is_empty());
}

/// Run in a child process with every proxy variable pointing at a trap: the policy client
/// ignores them all (`no_proxy`), so only the endpoint sees the request.
#[test]
fn proxy_settings_are_ignored() {
    const CHILD: &str = "MIMIK_TEST_PROXY_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (client, shared) = tcp_interpreter().await;
            client.complete(&completion()).await.unwrap();
            assert_eq!(shared.completions(), 1);
            let unix_dir = tempfile::tempdir().unwrap();
            let socket = unix_dir.path().join("llama.sock");
            let unix_shared = fake_server::unix(&socket);
            interpreter(Endpoint::Unix(socket))
                .complete(&completion())
                .await
                .unwrap();
            assert_eq!(unix_shared.completions(), 1);
        });
        return;
    }
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (trap, count) = runtime.block_on(fake_server::trap());
    let proxy = format!("http://{trap}");
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "assistant::transport::tests::proxy_settings_are_ignored",
            "--nocapture",
        ])
        .env(CHILD, "1");
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env(name, &proxy);
    }
    command.env("NO_PROXY", "").env("no_proxy", "");
    let status = command.status().unwrap();
    assert!(status.success());
    assert_eq!(count.load(Ordering::SeqCst), 0);
}
