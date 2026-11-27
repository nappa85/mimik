//! The plan's Tests -> Key, log part (docs/api-reviewer-plan.md): a capturing `tracing`
//! subscriber at TRACE runs for the whole `review_page` call against the fake server, and no
//! 8-character substring of the dummy key may appear in anything captured. `tracing-subscriber`
//! is not a dependency of Mimik, so this is a minimal `Subscriber` that records each event's and
//! each span's target and rendered field values (a span's fields at creation and again on
//! `record`), all searched below.
//!
//! rig 0.47.1 itself logs whole request bodies at TRACE under its `rig::completions` target
//! (`providers/internal::trace_json`: the base64 page image appears there). Filtering that
//! target is the only way to stop it from the subscriber side, so this test captures everything
//! and asserts that the image appears only in rig's own target's events: the reviewer emits
//! neither the key nor the request body. The limit is recorded as L-SRC-13 in
//! docs/limitations.md. The key is a dummy; no real provider is contacted and no environment
//! key is read.
use super::*;
use crate::assistant::download::fake_hub::{FakeHub, Reply};
use crate::assistant::download::https::{AddressPolicy, Https, ProxyMode, Roots, Transport};
use base64::Engine;
use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const KEY: &str = "dummy-key-logcheck-0123456789abcdef";
const MODEL: &str = "reviewer-log-model";
const HOST: &str = "provider.example";
const UNITS_JSON: &str =
    r#"{"pdf_page":22,"units":[{"id":"u-1","kind":"paragraph","text":"A rule."}]}"#;

/// Every 8-character substring of `value` (the secrecy tests' search set).
fn substrings8(value: &str) -> Vec<&str> {
    let bytes = value.as_bytes();
    (0..=bytes.len() - 8)
        .map(|start| &value[start..start + 8])
        .collect()
}

/// One captured record: its target, level and rendered fields.
struct Captured {
    target: &'static str,
    text: String,
}

/// The capturing subscriber: every callsite enabled (TRACE included), every event recorded,
/// and every span's fields recorded with the span's target — at creation (`new_span`) and
/// again on `record`.
#[derive(Clone, Default)]
struct Capture {
    events: Arc<Mutex<Vec<Captured>>>,
    spans: Arc<Mutex<HashMap<tracing::span::Id, Captured>>>,
    next_id: Arc<AtomicU64>,
}

impl Capture {
    /// Every captured record — events and spans alike — as `(target, rendered fields)`.
    fn events(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .events
            .lock()
            .unwrap()
            .iter()
            .map(|captured| (captured.target.to_owned(), captured.text.clone()))
            .collect();
        out.extend(
            self.spans
                .lock()
                .unwrap()
                .values()
                .map(|captured| (captured.target.to_owned(), captured.text.clone())),
        );
        out
    }
}

/// Renders one event's fields as `name=value` pairs.
struct Fields {
    text: String,
}

impl tracing::field::Visit for Fields {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.push(field.name(), value);
    }
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        self.push(field.name(), &format!("{value:?}"));
    }
}

impl Fields {
    fn push(&mut self, name: &str, value: &str) {
        if !self.text.is_empty() {
            self.text.push(' ');
        }
        self.text.push_str(name);
        self.text.push('=');
        self.text.push_str(value);
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        let id = tracing::span::Id::from_u64(self.next_id.fetch_add(1, Ordering::Relaxed) + 1);
        let mut fields = Fields {
            text: String::new(),
        };
        span.record(&mut fields);
        let metadata = span.metadata();
        self.spans.lock().unwrap().insert(
            id.clone(),
            Captured {
                target: metadata.target(),
                text: format!("{} {}", metadata.level(), fields.text),
            },
        );
        id
    }

    fn record(&self, span: &tracing::span::Id, values: &tracing::span::Record<'_>) {
        let mut fields = Fields {
            text: String::new(),
        };
        values.record(&mut fields);
        let mut spans = self.spans.lock().unwrap();
        if let Some(captured) = spans.get_mut(span)
            && !fields.text.is_empty()
        {
            if !captured.text.is_empty() {
                captured.text.push(' ');
            }
            captured.text.push_str(&fields.text);
        }
    }

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut fields = Fields {
            text: String::new(),
        };
        event.record(&mut fields);
        let metadata = event.metadata();
        self.events.lock().unwrap().push(Captured {
            target: metadata.target(),
            text: format!("{} {}", metadata.level(), fields.text),
        });
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

fn image() -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/reviewer/page.png"))
        .unwrap()
}

/// One `review_page` call under a TRACE subscriber: no 8-character substring of the key in any
/// captured event or span field, and the page image's base64 only in rig's own `rig::` targets'
/// records (rig logs whole request bodies at TRACE; the reviewer does not emit the body).
#[tokio::test]
async fn no_8_character_substring_of_the_key_appears_in_the_trace_of_a_review() {
    let reply = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/reviewer/mistral/reply-good.json"),
    )
    .unwrap();
    let hub = FakeHub::start(&[HOST], move |_| Reply::json(&reply)).await;
    let mut overrides = HashMap::new();
    overrides.insert(HOST.to_owned(), hub.addr);
    let transport = Transport {
        roots: Roots::Only(vec![hub.ca.clone()]),
        addresses: AddressPolicy::Any,
        lookup: None,
        overrides,
        proxy: ProxyMode::Direct,
        connect_timeout: Duration::from_secs(5),
        read_timeout: None,
        user_agent: "mimik-reviewer-log-test".to_owned(),
    };
    let base = format!("https://{HOST}:{}", hub.addr.port());
    let reviewer = Reviewer::new(
        Provider::Mistral,
        &base,
        KEY,
        MODEL,
        Https::new(&transport).unwrap(),
    )
    .unwrap();
    let image = image();
    let capture = Capture::default();
    let guard = tracing::subscriber::set_default(capture.clone());
    let result = reviewer.review_page(&image, UNITS_JSON).await;
    drop(guard);
    assert!(result.is_ok(), "{:?}", result.err());
    assert_eq!(hub.requests().len(), 1);
    let events = capture.events();
    assert!(!events.is_empty(), "the subscriber captured something");
    let all = events
        .iter()
        .map(|(target, text)| format!("{target} {text}"))
        .collect::<Vec<_>>()
        .join("\n");
    for piece in substrings8(KEY) {
        assert!(!all.contains(piece), "a captured event holds {piece:?}");
    }
    assert!(
        !all.contains(KEY),
        "the whole key appears in the captured output"
    );
    // The request body (the base64 image): rig 0.47.1 logs it at TRACE under `rig::completions`;
    // filtering that target is the only way to stop it, so it is expected there — and only
    // there. The reviewer itself emits no body (L-SRC-13 in docs/limitations.md).
    let image_b64 = base64::engine::general_purpose::STANDARD.encode(&image);
    for (target, text) in &events {
        if text.contains(&image_b64) {
            assert!(
                target.starts_with("rig::"),
                "the image appears outside rig's own target {target}: {}",
                &text[..text.len().min(200)]
            );
        }
    }
}
