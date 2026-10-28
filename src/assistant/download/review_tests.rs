use super::*;
use crate::assistant::download::client::SHIPPED_HOSTS;
use crate::assistant::download::fake_hub::{FakeHub, Reply, Seen};
use crate::assistant::model::{DEFAULT_MODEL_SHA256, GRANITE_HF, GRANITE_SIZE, Spec, parse_spec};
use serde_json::{Value, json};

fn spec(text: &str) -> HfSpec {
    let Ok(Spec::Hf(spec)) = parse_spec(text) else {
        panic!("fixture spec {text}")
    };
    spec
}

fn granite() -> HfSpec {
    spec(GRANITE_HF)
}

fn metadata(sha: &str, size: u64) -> Value {
    json!({
        "sha": "c40945d71cd90f249a56985e8155551a9188dc30",
        "private": false,
        "gated": false,
        "cardData": {"license": "apache-2.0"},
        "siblings": [
            {"rfilename": "LICENSE", "size": 11},
            {"rfilename": "granite-4.2-3b-Q4_K_M.gguf", "size": size,
             "lfs": {"sha256": sha, "size": size, "pointerSize": 135}}
        ]
    })
}

fn names() -> Vec<&'static str> {
    let mut names = SHIPPED_HOSTS.to_vec();
    names.push("evil.example");
    names
}

/// A hub that answers the metadata with `meta` and the licence with `licence`.
async fn hub_with(meta: Value, licence: Reply) -> FakeHub {
    FakeHub::start(&names(), move |seen: &Seen| {
        if seen.path.starts_with("/api/") {
            Reply::json(&meta.to_string())
        } else if seen.path.ends_with("/LICENSE") {
            licence.clone()
        } else {
            Reply::status(404)
        }
    })
    .await
}

fn reviewer(hub: &FakeHub) -> Reviewer {
    Reviewer::new(Client::new(&hub.endpoints()).unwrap())
}

async fn finished(reviewer: &Reviewer) -> State {
    for _ in 0..500 {
        let state = reviewer.state();
        if !matches!(state, State::Running { .. }) {
            return state;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the review did not finish")
}

#[tokio::test]
async fn a_review_is_one_snapshot_of_what_was_fetched_and_asks_for_no_model_bytes() {
    let hub = hub_with(
        metadata(DEFAULT_MODEL_SHA256, GRANITE_SIZE),
        Reply::text("Apache License <script>alert(1)</script>"),
    )
    .await;
    let reviewer = reviewer(&hub);
    reviewer.start(granite()).unwrap();
    let State::Ready(snapshot) = finished(&reviewer).await else {
        panic!("must be ready")
    };
    assert_eq!(snapshot.facts.sha256, DEFAULT_MODEL_SHA256);
    assert_eq!(snapshot.licence.id.as_deref(), Some("apache-2.0"));
    assert_eq!(snapshot.licence.path.as_deref(), Some("LICENSE"));
    assert_eq!(
        snapshot.licence.text.as_deref(),
        Some("Apache License <script>alert(1)</script>")
    );
    assert_eq!(snapshot.licence.sha256.as_deref().map(str::len), Some(64));
    assert!(
        snapshot.pinned.is_some(),
        "the Granite default is a pinned file"
    );
    assert_eq!(snapshot.token.len(), 32);
    // Only the API and the licence were asked for: no model-body request before Confirm.
    let paths: Vec<String> = hub
        .requests()
        .iter()
        .map(|seen| seen.path.clone())
        .collect();
    assert_eq!(paths.len(), 2, "{paths:?}");
    assert!(
        paths.iter().all(|path| !path.contains(".gguf")),
        "{paths:?}"
    );
    // The token names the live snapshot, and only it.
    assert!(reviewer.snapshot(&snapshot.token).is_some());
    assert!(reviewer.snapshot("0".repeat(32).as_str()).is_none());
    reviewer.clear();
    assert!(reviewer.snapshot(&snapshot.token).is_none());
}

#[tokio::test]
async fn a_pinned_spec_whose_hash_or_size_differs_from_the_compiled_in_is_refused() {
    for (sha, size) in [
        ("a".repeat(64), GRANITE_SIZE),
        (DEFAULT_MODEL_SHA256.to_owned(), GRANITE_SIZE + 1),
    ] {
        let hub = hub_with(metadata(&sha, size), Reply::text("licence")).await;
        let reviewer = reviewer(&hub);
        reviewer.start(granite()).unwrap();
        let State::Failed { reason, .. } = finished(&reviewer).await else {
            panic!("must fail")
        };
        assert!(reason.contains("pinned"), "{reason}");
        assert!(
            hub.requests()
                .iter()
                .all(|seen| !seen.path.ends_with("/LICENSE"))
        );
    }
}

#[tokio::test]
async fn a_second_review_waits_for_the_first_and_a_ready_one_is_replaced() {
    let hub = FakeHub::start(&names(), |_| {
        Reply::Delayed(Duration::from_millis(300), Box::new(Reply::status(500)))
    })
    .await;
    let reviewer = reviewer(&hub);
    reviewer.start(granite()).unwrap();
    assert_eq!(reviewer.start(granite()), Err(Busy));
    assert!(matches!(reviewer.state(), State::Running { .. }));
    assert!(matches!(finished(&reviewer).await, State::Failed { .. }));
    // After a failure a new review may start.
    hub.respond(|seen: &Seen| {
        if seen.path.starts_with("/api/") {
            Reply::json(&metadata(DEFAULT_MODEL_SHA256, GRANITE_SIZE).to_string())
        } else {
            Reply::text("licence")
        }
    });
    reviewer.start(granite()).unwrap();
    let State::Ready(first) = finished(&reviewer).await else {
        panic!()
    };
    reviewer.start(granite()).unwrap();
    let State::Ready(second) = finished(&reviewer).await else {
        panic!()
    };
    assert_ne!(first.token, second.token);
    assert!(
        reviewer.snapshot(&first.token).is_none(),
        "the older token is dead"
    );
    assert!(reviewer.snapshot(&second.token).is_some());
}

#[tokio::test]
async fn a_ready_review_stops_being_confirmable_after_its_life() {
    let hub = hub_with(
        metadata(DEFAULT_MODEL_SHA256, GRANITE_SIZE),
        Reply::text("licence"),
    )
    .await;
    let reviewer = Reviewer::with_life(
        Client::new(&hub.endpoints()).unwrap(),
        Duration::from_millis(400),
    );
    assert_eq!(LIFE, Duration::from_mins(30), "the shipped life");
    reviewer.start(granite()).unwrap();
    let State::Ready(snapshot) = finished(&reviewer).await else {
        panic!()
    };
    assert!(reviewer.snapshot(&snapshot.token).is_some());
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(reviewer.snapshot(&snapshot.token).is_none());
    assert!(matches!(reviewer.state(), State::Idle));
}

#[tokio::test]
async fn the_licence_file_rules_hold_in_the_review() {
    // The card names an external link: shown as text, never fetched; the root file is used.
    let mut meta = metadata(DEFAULT_MODEL_SHA256, GRANITE_SIZE);
    meta["cardData"]["license_link"] = json!("https://evil.example/LICENSE");
    let hub = hub_with(meta, Reply::text("root licence")).await;
    let reviewer = reviewer(&hub);
    reviewer.start(granite()).unwrap();
    let State::Ready(snapshot) = finished(&reviewer).await else {
        panic!()
    };
    assert_eq!(
        snapshot.licence.link.as_deref(),
        Some("https://evil.example/LICENSE")
    );
    assert_eq!(snapshot.licence.text.as_deref(), Some("root licence"));
    assert!(
        hub.requests()
            .iter()
            .all(|seen| seen.host != "evil.example")
    );

    // Named but not listed: refused, not "none".
    let mut meta = metadata(DEFAULT_MODEL_SHA256, GRANITE_SIZE);
    meta["cardData"]["license_link"] = json!("MISSING.txt");
    let hub = hub_with(meta, Reply::text("x")).await;
    let reviewer = self::reviewer(&hub);
    reviewer.start(granite()).unwrap();
    assert!(matches!(finished(&reviewer).await, State::Failed { .. }));

    // A licence file over 1 MiB is refused, not truncated.
    let hub = hub_with(
        metadata(DEFAULT_MODEL_SHA256, GRANITE_SIZE),
        Reply::text(&"x".repeat(MAX_LICENCE_BYTES + 1)),
    )
    .await;
    let reviewer = self::reviewer(&hub);
    reviewer.start(granite()).unwrap();
    let State::Failed { reason, .. } = finished(&reviewer).await else {
        panic!()
    };
    assert!(reason.contains("licence file"), "{reason}");

    // No licence file listed: the review is ready with none, and can be confirmed.
    let mut meta = metadata(DEFAULT_MODEL_SHA256, GRANITE_SIZE);
    meta["siblings"].as_array_mut().unwrap().remove(0);
    let hub = hub_with(meta, Reply::text("x")).await;
    let reviewer = self::reviewer(&hub);
    reviewer.start(granite()).unwrap();
    let State::Ready(snapshot) = finished(&reviewer).await else {
        panic!()
    };
    assert_eq!(snapshot.licence.text, None);
    assert_eq!(snapshot.licence.sha256, None);
}

#[tokio::test]
async fn a_repository_that_answers_the_metadata_but_denies_the_file_stops_the_review() {
    // Metadata 200 with a licence file that resolves to 401: the by-hand advice, no download.
    let hub = hub_with(
        metadata(DEFAULT_MODEL_SHA256, GRANITE_SIZE),
        Reply::status(401),
    )
    .await;
    let reviewer = reviewer(&hub);
    reviewer.start(granite()).unwrap();
    let State::Failed { reason, .. } = finished(&reviewer).await else {
        panic!()
    };
    assert!(reason.contains("by hand"), "{reason}");
    // The API itself denying is the same.
    let hub = FakeHub::start(&names(), |_| Reply::status(403)).await;
    let reviewer = self::reviewer(&hub);
    reviewer.start(granite()).unwrap();
    let State::Failed { reason, .. } = finished(&reviewer).await else {
        panic!()
    };
    assert!(reason.contains("by hand"), "{reason}");
}

#[tokio::test]
async fn a_review_whose_task_panics_ends_as_failed_and_a_new_one_can_start() {
    let hub = hub_with(
        metadata(DEFAULT_MODEL_SHA256, GRANITE_SIZE),
        Reply::text("licence"),
    )
    .await;
    let reviewer = reviewer(&hub);
    reviewer
        .start_with(granite(), async { panic!("the review code failed") })
        .unwrap();
    let State::Failed { reason, .. } = finished(&reviewer).await else {
        panic!("must not stay running")
    };
    assert!(reason.contains("unexpectedly"), "{reason}");
    reviewer.start(granite()).unwrap();
    assert!(matches!(finished(&reviewer).await, State::Ready(_)));
}
