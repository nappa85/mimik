//! Loopback stubs for the 2b crate probes: a recording OpenAI-compatible server, a trap
//! listener that counts any connection, and a raw "hang" server that reports when the
//! client closes the connection. Also the §1.4 destination guard and the policy client.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Json,
    Sse,
    Redirect(u16),
    Big(usize),
    BigSse(usize),
    Status(u16),
}

#[derive(Debug, Clone)]
pub struct Recorded {
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

#[derive(Clone)]
pub struct Stub {
    pub mode: Arc<Mutex<Mode>>,
    pub seen: Arc<Mutex<Vec<Recorded>>>,
}

impl Stub {
    pub fn set(&self, mode: Mode) {
        *self.mode.lock().unwrap() = mode;
    }
    pub fn last(&self) -> Option<Recorded> {
        self.seen.lock().unwrap().last().cloned()
    }
    pub fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

pub const REPLY: &str = r#"{"kind":"options","options":[{"expr":{"op":"sub","l":{"ref":"hp"},"r":{"lit":5}},"explanation":"20 - 5"}]}"#;

fn completion(content: &str) -> Value {
    json!({
        "id": "chatcmpl-stub", "object": "chat.completion", "created": 0, "model": "probe",
        "choices": [{"index": 0, "finish_reason": "stop",
            "message": {"role": "assistant", "content": content}}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
    })
}

fn chunk(delta: &Value, finish: Option<&str>) -> String {
    let c = json!({"id": "chatcmpl-stub", "object": "chat.completion.chunk", "created": 0,
        "model": "probe", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]});
    format!("data: {c}\n\n")
}

async fn handle(
    State(stub): State<Stub>,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    stub.seen.lock().unwrap().push(Recorded {
        path: uri.path().to_string(),
        headers: headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("?").to_string()))
            .collect(),
        body: parsed,
    });
    let mode = *stub.mode.lock().unwrap();
    match mode {
        Mode::Json => axum::Json(completion(REPLY)).into_response(),
        Mode::Sse => {
            let mut text = chunk(&json!({"role": "assistant", "content": ""}), None);
            for part in REPLY.as_bytes().chunks(16) {
                text += &chunk(&json!({"content": String::from_utf8_lossy(part)}), None);
            }
            text += &chunk(&json!({}), Some("stop"));
            text += "data: [DONE]\n\n";
            ([(header::CONTENT_TYPE, "text/event-stream")], text).into_response()
        }
        Mode::Redirect(port) => (
            StatusCode::FOUND,
            [(header::LOCATION, format!("http://127.0.0.1:{port}/v1/chat/completions"))],
        )
            .into_response(),
        Mode::Big(n) => axum::Json(completion(&"x".repeat(n))).into_response(),
        Mode::Status(code) => (
            StatusCode::from_u16(code).unwrap(),
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"error":{"message":"stub failure","type":"server_error"}}"#,
        )
            .into_response(),
        Mode::BigSse(n) => {
            // One SSE line of n bytes with no delimiter, streamed in 64 KiB chunks.
            let stream = futures_util::stream::unfold((n, true), |(left, first)| async move {
                if left == 0 {
                    return None;
                }
                let size = left.min(64 * 1024);
                let mut data = Vec::with_capacity(size + 6);
                if first {
                    data.extend_from_slice(b"data: ");
                }
                data.resize(data.len() + size, b'x');
                Some((Ok::<_, std::io::Error>(Bytes::from(data)), (left - size, false)))
            });
            Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from_stream(stream))
                .unwrap()
        }
    }
}

/// The C3 stub in its own process: Big on 127.0.0.1:18081, BigSse on 127.0.0.1:18082.
pub async fn serve_c3() {
    for (port, mode) in [(18081u16, Mode::Big(BIG)), (18082, Mode::BigSse(BIG))] {
        let stub = Stub { mode: Arc::new(Mutex::new(mode)), seen: Arc::new(Mutex::new(Vec::new())) };
        let app = Router::new().fallback(any(handle)).with_state(stub);
        let listener = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    }
    std::future::pending::<()>().await;
}

/// Address of the external C3 stub for a scenario ending in "plain" or "stream".
pub fn c3_addr(scenario: &str) -> SocketAddr {
    if scenario.ends_with("stream") { "127.0.0.1:18082" } else { "127.0.0.1:18081" }.parse().unwrap()
}

/// Counts the requests one call makes when the server answers `code` (retries).
pub async fn attempts_on_status<F, Fut, T>(stub: &Stub, code: u16, call: F) -> Value
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    stub.set(Mode::Status(code));
    let before = stub.count();
    let started = Instant::now();
    let _ = tokio::time::timeout(Duration::from_secs(60), call()).await;
    let attempts = stub.count() - before;
    stub.set(Mode::Json);
    json!({"status": code, "requests_for_one_call": attempts,
           "elapsed_ms": started.elapsed().as_millis()})
}

pub async fn start_stub() -> (SocketAddr, Stub) {
    let stub = Stub { mode: Arc::new(Mutex::new(Mode::Json)), seen: Arc::new(Mutex::new(Vec::new())) };
    let app = Router::new().fallback(any(handle)).with_state(stub.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, stub)
}

/// Counts every accepted connection (and answers 502).
pub async fn start_trap() -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            c.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut buf)).await;
                let _ = socket
                    .write_all(b"HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                    .await;
            });
        }
    });
    (addr, count)
}

pub fn trap_count(c: &AtomicUsize) -> usize {
    c.load(Ordering::SeqCst)
}

/// Reads one request, optionally sends SSE headers only, then waits for the client to close.
/// Records the instant at which the read side saw end-of-stream (or an error).
pub async fn start_hang(send_headers: bool) -> (SocketAddr, Arc<Mutex<Option<Instant>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let closed = Arc::new(Mutex::new(None));
    let c = closed.clone();
    tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else { return };
        let mut request = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            let n = socket.read(&mut buf).await.unwrap_or(0);
            if n == 0 {
                *c.lock().unwrap() = Some(Instant::now());
                return;
            }
            request.extend_from_slice(&buf[..n]);
            if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                let len = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if request.len() >= end + 4 + len {
                    break;
                }
            }
        }
        if send_headers {
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n")
                .await;
        }
        loop {
            match socket.read(&mut buf).await {
                Ok(0) | Err(_) => {
                    *c.lock().unwrap() = Some(Instant::now());
                    break;
                }
                Ok(_) => {}
            }
        }
    });
    (addr, closed)
}

/// The probe's output schema: the frozen 2c schema from tools/assist_eval.py (as JSON).
pub fn schema() -> Value {
    serde_json::from_str(include_str!("../../schema.json")).unwrap()
}

/// Destination guard of §1.4: raw-string and parsed checks; IP literal 127.0.0.1 or ::1 only.
pub fn endpoint_allowed(raw: &str) -> Result<url::Url, &'static str> {
    let rest = raw.strip_prefix("http://").ok_or("scheme must be http")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err("userinfo");
    }
    if authority.contains('%') {
        return Err("zone id");
    }
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().ok_or("bad ipv6")?
    } else {
        authority.rsplit_once(':').map_or(authority, |(h, _)| h)
    };
    if host != "127.0.0.1" && host != "::1" {
        return Err("host must be the literal 127.0.0.1 or ::1");
    }
    let url = url::Url::parse(raw).map_err(|_| "unparsable")?;
    match url.host() {
        Some(url::Host::Ipv4(ip)) if ip == std::net::Ipv4Addr::LOCALHOST => {}
        Some(url::Host::Ipv6(ip)) if ip == std::net::Ipv6Addr::LOCALHOST => {}
        _ => return Err("parsed host mismatch"),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("userinfo");
    }
    Ok(url)
}

pub fn guard_report() -> Value {
    let cases = [
        ("http://127.0.0.1:8091/v1", true),
        ("http://[::1]:8091/v1", true),
        ("http://localhost:8091/v1", false),
        ("http://127.1:8091/v1", false),
        ("http://[::ffff:127.0.0.1]:8091/v1", false),
        ("http://user@127.0.0.1:8091/v1", false),
        ("http://user:pw@127.0.0.1:8091/v1", false),
        ("http://[::1%25lo]:8091/v1", false),
        ("http://example.com:8091/v1", false),
        ("http://0x7f.1:8091/v1", false),
        ("http://2130706433:8091/v1", false),
        ("https://127.0.0.1:8091/v1", false),
        ("http://127.0.0.1@evil:8091/v1", false),
        ("http://127.0.0.1.:8091/v1", false),
    ];
    let rows: Vec<Value> = cases
        .iter()
        .map(|(raw, expect)| {
            let got = endpoint_allowed(raw);
            let allowed = got.is_ok();
            json!({"url": raw, "expected_allowed": expect, "allowed": allowed,
                   "reason": got.err(), "ok": allowed == *expect})
        })
        .collect();
    let whatwg: Vec<Value> = cases
        .iter()
        .map(|(raw, _)| {
            json!({"url": raw, "parsed_host": url::Url::parse(raw)
                .map_or_else(|e| format!("error {e}"), |u| format!("{:?}", u.host()))})
        })
        .collect();
    json!({"guard": rows, "whatwg_parse": whatwg})
}

/// Peak resident set size of this process in KiB (Linux).
pub fn vm_hwm_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("VmHWM:"))
                .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        })
        .unwrap_or(0)
}

pub fn set_proxy_env(trap: SocketAddr) {
    for key in ["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy", "HTTPS_PROXY", "https_proxy"] {
        // SAFETY: called at the start of main, before any other thread exists.
        unsafe { std::env::set_var(key, format!("http://{trap}")) };
    }
    for key in ["NO_PROXY", "no_proxy"] {
        // SAFETY: as above.
        unsafe { std::env::remove_var(key) };
    }
}

/// The host-built client of §1.4: no proxies, no redirects, no decompression (no features),
/// no TLS (no features), a fresh connection per request.
pub fn policy_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .pool_max_idle_per_host(0)
        .connect_timeout(Duration::from_secs(2))
        .build()
        .unwrap()
}

/// Reads a reqwest response body with a byte bound, chunk by chunk.
pub async fn bounded_body(response: reqwest::Response, limit: usize) -> Result<Bytes, String> {
    use futures_util::StreamExt;
    let mut stream = response.bytes_stream();
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        if out.len() + chunk.len() > limit {
            return Err(format!("body over {limit} bytes"));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(out))
}

/// Measures whether dropping `fut` after 1 s closes the connection of the hang server.
pub async fn cancellation<F: std::future::Future>(closed: Arc<Mutex<Option<Instant>>>, fut: F) -> Value {
    let started = Instant::now();
    let timed_out = tokio::time::timeout(Duration::from_secs(1), fut).await.is_err();
    let dropped = Instant::now();
    for _ in 0..40 {
        if closed.lock().unwrap().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let seen = *closed.lock().unwrap();
    json!({
        "timed_out": timed_out,
        "dropped_after_ms": (dropped - started).as_millis(),
        "server_saw_close": seen.is_some(),
        "close_after_drop_ms": seen.map(|t| t.saturating_duration_since(dropped).as_millis()),
        "ok": timed_out && seen.is_some_and(|t| t.saturating_duration_since(dropped) < Duration::from_secs(1)),
    })
}

pub const BODY_LIMIT: usize = 1 << 20;
pub const BIG: usize = 64 << 20;
