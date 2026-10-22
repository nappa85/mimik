//! A fake llama-server for tests: `/v1/chat/completions` (streamed like llama.cpp's SSE),
//! `/tokenize` and `/health`, on TCP or a Unix socket, recording every request and noticing
//! when a client goes away.
use axum::{
    Router,
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

/// How the next completion is answered.
#[derive(Clone, Debug)]
pub(crate) enum Reply {
    /// The content, streamed in small chunks, then a stop with usage.
    Content(String),
    /// A plain status with a short body.
    Status(u16),
    /// A redirect to `location`.
    Redirect(String),
    /// A body of this many bytes (plain JSON, not a stream).
    Huge(usize),
    /// An endless stream of SSE comment lines.
    Endless,
    /// A gzip-encoded body.
    Gzip,
    /// Data that is not an SSE completion.
    Nonsense,
    /// Waits this long before every chunk of the content.
    Slow(Duration, String),
}

#[derive(Clone, Debug)]
pub(crate) struct Seen {
    pub path: String,
    pub headers: HeaderMap,
    pub body: Bytes,
}

#[derive(Default)]
pub(crate) struct Shared {
    pub replies: Mutex<VecDeque<Reply>>,
    pub seen: Mutex<Vec<Seen>>,
    /// Completions whose stream was dropped by the client before the end.
    pub disconnected: AtomicUsize,
    /// Completions currently being streamed.
    pub open: AtomicUsize,
    /// Tokens per request of `/tokenize`, when set (otherwise one per 3 bytes).
    pub tokens: Mutex<Option<usize>>,
}

impl Shared {
    pub(crate) fn push(&self, reply: Reply) {
        self.replies.lock().unwrap().push_back(reply);
    }

    pub(crate) fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    pub(crate) fn completions(&self) -> usize {
        self.seen()
            .iter()
            .filter(|seen| seen.path == "/v1/chat/completions")
            .count()
    }
}

/// Decrements `open` and counts a disconnect when dropped before `done`.
struct Guard {
    shared: Arc<Shared>,
    done: std::sync::atomic::AtomicBool,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.shared.open.fetch_sub(1, Ordering::SeqCst);
        if !self.done.load(Ordering::SeqCst) {
            self.shared.disconnected.fetch_add(1, Ordering::SeqCst);
        }
    }
}

fn chunk(content: &str) -> String {
    let frame = json!({"object": "chat.completion.chunk", "model": "test",
        "choices": [{"index": 0, "delta": {"content": content}, "finish_reason": null}]});
    format!("data: {frame}\n\n")
}

fn stop() -> String {
    let frame = json!({"object": "chat.completion.chunk", "model": "test",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 1500, "completion_tokens": 40, "total_tokens": 1540,
            "prompt_tokens_details": {"cached_tokens": 1290}},
        "timings": {"cache_n": 1290, "prompt_n": 210, "predicted_n": 40}});
    format!("data: {frame}\n\ndata: [DONE]\n\n")
}

fn stream(shared: Arc<Shared>, delay: Duration, content: &str) -> Response {
    shared.open.fetch_add(1, Ordering::SeqCst);
    let pieces: Vec<String> = content
        .chars()
        .collect::<Vec<_>>()
        .chunks(8)
        .map(|piece| chunk(&piece.iter().collect::<String>()))
        .collect();
    let body = futures_util::stream::unfold(
        (
            Guard {
                shared,
                done: std::sync::atomic::AtomicBool::new(false),
            },
            pieces.into_iter(),
            delay,
            false,
        ),
        |(guard, mut pieces, delay, ended)| async move {
            if ended {
                guard.done.store(true, Ordering::SeqCst);
                return None;
            }
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let (text, ended) = match pieces.next() {
                Some(piece) => (piece, false),
                None => (stop(), true),
            };
            Some((
                Ok::<_, std::io::Error>(Bytes::from(text)),
                (guard, pieces, delay, ended),
            ))
        },
    );
    (
        [(header::CONTENT_TYPE, "text/event-stream")],
        Body::from_stream(body),
    )
        .into_response()
}

async fn completions(
    State(shared): State<Arc<Shared>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    shared.seen.lock().unwrap().push(Seen {
        path: "/v1/chat/completions".into(),
        headers,
        body,
    });
    let reply = shared
        .replies
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or(Reply::Content(r#"{"kind":"options","options":[]}"#.into()));
    match reply {
        Reply::Content(content) => stream(shared, Duration::ZERO, &content),
        Reply::Slow(delay, content) => stream(shared, delay, &content),
        Reply::Status(code) => (StatusCode::from_u16(code).unwrap(), "no").into_response(),
        Reply::Redirect(location) => {
            (StatusCode::FOUND, [(header::LOCATION, location)], "").into_response()
        }
        Reply::Huge(bytes) => {
            let body = futures_util::stream::iter(
                (0..bytes.div_ceil(65_536))
                    .map(|_| Ok::<_, std::io::Error>(Bytes::from(vec![b' '; 65_536]))),
            );
            (
                [(header::CONTENT_TYPE, "text/event-stream")],
                Body::from_stream(body),
            )
                .into_response()
        }
        Reply::Endless => {
            let body = futures_util::stream::repeat_with(|| {
                Ok::<_, std::io::Error>(Bytes::from_static(&[b':'; 8192]))
            });
            (
                [(header::CONTENT_TYPE, "text/event-stream")],
                Body::from_stream(body),
            )
                .into_response()
        }
        Reply::Gzip => (
            [
                (header::CONTENT_TYPE, "text/event-stream"),
                (header::CONTENT_ENCODING, "gzip"),
            ],
            // A gzip member header and garbage: decoding it would fail loudly either way.
            Bytes::from_static(b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\x03data"),
        )
            .into_response(),
        Reply::Nonsense => (
            [(header::CONTENT_TYPE, "text/event-stream")],
            "data: {\"not\": \"a completion\"}\n\ndata: [DONE]\n\n",
        )
            .into_response(),
    }
}

async fn tokenize(State(shared): State<Arc<Shared>>, headers: HeaderMap, body: Bytes) -> Response {
    let content = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|value| value["content"].as_str().map(str::len))
        .unwrap_or_default();
    shared.seen.lock().unwrap().push(Seen {
        path: "/tokenize".into(),
        headers,
        body,
    });
    let count = shared.tokens.lock().unwrap().unwrap_or(content.div_ceil(3));
    axum::Json(json!({"tokens": vec![1; count]})).into_response()
}

pub(crate) fn router(shared: Arc<Shared>) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(completions))
        .route("/tokenize", post(tokenize))
        .route("/health", get(|| async { "ok" }))
        .with_state(shared)
}

/// Serves on `127.0.0.1` at a free port; the URL is `http://127.0.0.1:<port>`.
pub(crate) async fn tcp() -> (String, Arc<Shared>) {
    let shared = Arc::new(Shared::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = router(Arc::clone(&shared));
    tokio::spawn(async move { axum::serve(listener, app).await });
    (format!("http://{address}"), shared)
}

/// Serves on a Unix socket at `path`.
#[cfg(unix)]
pub(crate) fn unix(path: &std::path::Path) -> Arc<Shared> {
    let shared = Arc::new(Shared::default());
    let listener = tokio::net::UnixListener::bind(path).unwrap();
    let app = router(Arc::clone(&shared));
    tokio::spawn(async move { axum::serve(listener, app).await });
    shared
}

/// A plain TCP listener that only counts the connections it accepts (a proxy or redirect
/// trap).
pub(crate) async fn trap() -> (std::net::SocketAddr, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&count);
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            counted.fetch_add(1, Ordering::SeqCst);
            drop(socket);
        }
    });
    (address, count)
}
