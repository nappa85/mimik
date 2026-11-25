//! Master view slice (b2): the event stream's slots, handshake, revalidation, coalescing and
//! shutdown through the full router (CAMP-14). Timing runs on tokio's paused clock (the
//! production constants, never shortened): the paused clock does not advance while a blocking
//! database operation runs, so a test moves it with `advance` when it holds the database.
use super::*;
use crate::streams::{Limits, Refused, TokenState, WorkError, timing};
use crate::{AppError, auth::RequestAuth};
use http_body_util::BodyExt;
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};
use std::time::Duration;

const SECOND: Duration = Duration::from_secs(1);

// ---------------------------------------------------------------------------------------
// Helpers

fn events_path(setup: &Setup) -> String {
    format!("/campaigns/{}/master/events", setup.campaign)
}

/// A GET of the events route as `who`, without the long test deadline: the production 5 s
/// boundary applies to the handshake.
async fn events_request(
    setup: &Setup,
    path: &str,
    headers: &[(&str, &str)],
) -> axum::response::Response {
    use tower::ServiceExt;
    let mut request = axum::http::Request::builder()
        .uri(path)
        .header("host", "127.0.0.1:3000")
        .header("accept", "text/event-stream");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    setup
        .app
        .clone()
        .oneshot(request.body(axum::body::Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn events_as(
    setup: &Setup,
    who: usize,
    headers: &[(&str, &str)],
) -> axum::response::Response {
    let cookie = setup.auth[who].fixture_cookie();
    let mut all = vec![("cookie", cookie.as_str())];
    all.extend_from_slice(headers);
    events_request(setup, &events_path(setup), &all).await
}

#[derive(Debug, Clone, PartialEq)]
struct Ev {
    name: String,
    data: String,
    retry: bool,
    comment: bool,
}

/// An incrementally read event body. It never collects the body: one frame at a time.
struct Live {
    body: axum::body::Body,
    pending: String,
}

impl Live {
    fn new(response: axum::response::Response) -> Self {
        Self {
            body: response.into_body(),
            pending: String::new(),
        }
    }

    /// The next event or comment; `None` when the body ended.
    async fn event(&mut self) -> Option<Ev> {
        loop {
            if let Some(end) = self.pending.find("\n\n") {
                let block: String = self.pending.drain(..end + 2).collect();
                let mut ev = Ev {
                    name: String::new(),
                    data: String::new(),
                    retry: false,
                    comment: false,
                };
                for line in block.lines() {
                    if line.starts_with(':') {
                        ev.comment = true;
                    } else if let Some(name) = line.strip_prefix("event: ") {
                        ev.name = name.into();
                    } else if let Some(data) = line.strip_prefix("data: ") {
                        ev.data.push_str(data);
                    } else if line.starts_with("retry: ") {
                        ev.retry = line == "retry: 3000";
                    }
                }
                return Some(ev);
            }
            let frame = self.body.frame().await?.ok()?;
            if let Ok(data) = frame.into_data() {
                self.pending.push_str(std::str::from_utf8(&data).unwrap());
            }
        }
    }

    /// The next non-comment event.
    async fn data(&mut self) -> Option<Ev> {
        loop {
            let ev = self.event().await?;
            if !ev.comment {
                return Some(ev);
            }
        }
    }

    /// The next `snapshot` event, parsed (status events are skipped); `None` at the end.
    async fn snapshot(&mut self) -> Option<serde_json::Value> {
        loop {
            let ev = self.data().await?;
            if ev.name == "snapshot" {
                return Some(serde_json::from_str(&ev.data).unwrap());
            }
        }
    }

    /// The hello `status` event's stream token, then the first snapshot.
    async fn hello(&mut self) -> (String, serde_json::Value) {
        let hello = self.data().await.expect("hello");
        assert_eq!(hello.name, "status");
        assert!(hello.retry, "the first event advertises retry: 3000");
        let value: serde_json::Value = serde_json::from_str(&hello.data).unwrap();
        assert_eq!(value["code"], "live");
        let token = value["stream"].as_str().unwrap().to_owned();
        assert_eq!(token.len(), 32);
        let first = self.data().await.expect("first snapshot");
        assert_eq!(first.name, "snapshot");
        (token, serde_json::from_str(&first.data).unwrap())
    }

    /// Whether the body ends (without any further event) within `within`.
    async fn ends_within(&mut self, within: Duration) -> bool {
        matches!(
            tokio::time::timeout(within, async {
                while let Some(ev) = self.event().await {
                    assert!(ev.comment, "no data may follow a fail-closed close: {ev:?}");
                }
            })
            .await,
            Ok(())
        )
    }
}

async fn open_stream(setup: &Setup, who: usize) -> Live {
    let response = events_as(setup, who, &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    Live::new(response)
}

async fn probe(setup: &Setup, who: usize, query: &str) -> (StatusCode, axum::http::HeaderMap) {
    let cookie = setup.auth[who].fixture_cookie();
    let response = events_request(
        setup,
        &format!("/campaigns/{}/master/snapshot{query}", setup.campaign),
        &[("cookie", cookie.as_str())],
    )
    .await;
    (response.status(), response.headers().clone())
}

fn revision_of(snapshot: &serde_json::Value, section: &str, row: usize) -> String {
    snapshot[section]["rows"][row]["revision"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn edit(setup: &Setup, who: usize, sheet: i64, revision: i64, hp: &str) {
    let (status, html) = send(
        setup,
        who,
        &format!("/private-sheets/{sheet}"),
        Some(form(revision, &[("value-hp", hp)], &[row("r1-1", "1")])),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{html}");
}

/// Sheet 1 (player sheet of Master A), at revision 1.
async fn with_sheet() -> Setup {
    let setup = world().await;
    assert_eq!(create_as(&setup, 1, false).await, StatusCode::SEE_OTHER);
    setup
}

async fn set_clock(setup: &Setup, value: i64) -> Arc<AtomicI64> {
    let clock = Arc::new(AtomicI64::new(value));
    let shared = clock.clone();
    setup
        .store
        .run(move |db| {
            crate::store::register_auth_clock(
                db,
                Arc::new(move || Ok(shared.load(Ordering::SeqCst))),
            )
        })
        .await
        .unwrap();
    clock
}

async fn account_session(setup: &Setup, id: i64) -> RequestAuth {
    use tower_sessions::SessionStore as _;
    setup
        .store
        .run(move |db| {
            db.execute("INSERT INTO accounts(id,login,display_name,state,epoch,verifier) SELECT ?1,'player'||?1,'player'||?1,'enabled',1,verifier FROM accounts WHERE id=2", [id])?;
            Ok(())
        })
        .await
        .unwrap();
    let mut data = std::collections::HashMap::new();
    data.insert("csrf".to_owned(), json!("a".repeat(64)));
    data.insert("principal".to_owned(), json!({"id": id, "epoch": 1}));
    let mut record = tower_sessions::session::Record {
        id: tower_sessions::session::Id::default(),
        data,
        expiry_date: time::OffsetDateTime::from_unix_timestamp(1000).unwrap(),
    };
    crate::auth::SqliteSessions::new(setup.store.clone())
        .create(&mut record)
        .await
        .unwrap();
    RequestAuth::fixture(id, record.id, 1000)
}

/// Joins `target` (a new account) to `campaign` as a Master, through the domain API.
async fn add_master(setup: &Setup, creator: &RequestAuth, campaign: i64, id: i64) -> RequestAuth {
    let auth = account_session(setup, id).await;
    setup
        .store
        .request_join(auth.clone(), campaign)
        .await
        .unwrap();
    setup
        .store
        .decide_join(creator.clone(), campaign, id, 1, true)
        .await
        .unwrap();
    setup
        .store
        .change_role(creator.clone(), campaign, id, 1, true)
        .await
        .unwrap();
    auth
}

/// One hop through the blocking pool: parks the task while a blocking operation of the test
/// runs (the paused clock cannot advance meanwhile, so no time-based wait may be used here).
async fn hop() {
    tokio::task::spawn_blocking(|| std::thread::sleep(Duration::from_millis(1)))
        .await
        .unwrap();
    tokio::task::yield_now().await;
}

async fn yield_a_few() {
    for _ in 0..5 {
        hop().await;
    }
}

/// Waits (hopping through the blocking pool, never touching the paused clock) until
/// `condition` holds.
async fn settle(condition: impl Fn() -> bool) {
    for _ in 0..5000 {
        if condition() {
            return;
        }
        hop().await;
    }
    assert!(condition(), "did not settle");
}

/// Moves the paused clock forward by `total` in 100 ms steps, letting the tasks run between
/// steps (one jump would show a producer all the elapsed time at once).
async fn run_for(total: Duration) {
    let step = Duration::from_millis(100);
    let mut spent = Duration::ZERO;
    while spent < total {
        tokio::time::advance(step).await;
        hop().await;
        spent += step;
    }
}

/// Installs the handshake gate of `AFTER_ADMIT` and removes it again when dropped (also when a
/// test panics, so no other test stalls on it).
struct GateReset;

impl GateReset {
    fn set(gate: Arc<tokio::sync::Semaphore>) -> Self {
        *crate::campaigns::master_stream::AFTER_ADMIT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(gate);
        Self
    }
}

impl Drop for GateReset {
    fn drop(&mut self) {
        *crate::campaigns::master_stream::AFTER_ADMIT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

async fn session_rows(setup: &Setup) -> Vec<String> {
    setup
        .store
        .run(|db| {
            let mut q = db.prepare(
                "SELECT id||'|'||issued_at||'|'||expires_at||'|'||state||'|'||coalesce(payload,'<null>')||'|'||coalesce(account_id,-1)||'|'||coalesce(epoch,-1) FROM auth_sessions ORDER BY id",
            )?;
            Ok(q.query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .await
        .unwrap()
}

async fn seen_rows(setup: &Setup) -> Vec<String> {
    setup
        .store
        .run(|db| {
            let mut q = db.prepare(
                "SELECT master_id||'|'||sheet_id||'|'||seen_revision FROM master_sheet_seen ORDER BY 1",
            )?;
            Ok(q.query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------------------
// Handshake

#[tokio::test(start_paused = true)]
async fn the_handshake_admits_masters_alone_and_never_writes_the_session_store() {
    let _locks = lock().await;
    let setup = with_sheet().await;
    // Anonymous: 401 (not a redirect: an EventSource sends no navigation metadata).
    let anonymous = events_request(&setup, &events_path(&setup), &[]).await;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    // The Owner, a player and an outsider are refused alike; so is an unknown campaign.
    let mut bodies = Vec::new();
    for who in [0, 2, 4] {
        let response = events_as(&setup, who, &[]).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{who}");
        bodies.push(super::super::formula_tests::body(response).await);
    }
    assert!(
        bodies.windows(2).all(|pair| pair[0] == pair[1]),
        "uniform: {bodies:?}"
    );
    assert!(!bodies[0].contains("Assisted"), "no sheet metadata");
    let cookie = setup.auth[1].fixture_cookie();
    let unknown = events_request(
        &setup,
        "/campaigns/99/master/events",
        &[("cookie", cookie.as_str())],
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::FORBIDDEN);
    // Cross-site fetch metadata and a foreign or doubled Origin are refused; same-origin and
    // an exact Origin pass; cursors are validated.
    for headers in [
        vec![("sec-fetch-site", "cross-site")],
        vec![("sec-fetch-site", "same-site")],
        vec![("origin", "http://evil.example")],
        vec![
            ("origin", "http://127.0.0.1:3000"),
            ("origin", "http://127.0.0.1:3000"),
        ],
    ] {
        let response = events_as(&setup, 1, &headers).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{headers:?}");
        assert!(
            response
                .headers()
                .get("access-control-allow-origin")
                .is_none()
        );
    }
    for headers in [
        vec![("sec-fetch-site", "same-origin")],
        vec![("origin", "http://127.0.0.1:3000")],
        vec![],
    ] {
        let response = events_as(&setup, 1, &headers).await;
        assert_eq!(response.status(), StatusCode::OK, "{headers:?}");
        drop(response);
        yield_a_few().await;
    }
    for query in [
        "?players_after=-1",
        "?npcs_after=-1",
        "?other=1",
        "?players_after=x",
    ] {
        let cookie = setup.auth[1].fixture_cookie();
        let response = events_request(
            &setup,
            &format!("/campaigns/{}/master/events{query}", setup.campaign),
            &[("cookie", cookie.as_str())],
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{query}");
    }
    // A revoked session, a disabled account and an expired session: 401, nothing written.
    setup
        .store
        .run(|db| {
            db.execute(
                "UPDATE auth_sessions SET state='revoked',payload=NULL WHERE account_id=4",
                [],
            )?;
            db.execute(
                "UPDATE accounts SET state='disabled',epoch=2 WHERE id=3",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let revoked = session_rows(&setup).await;
    for who in [3, 2] {
        assert_eq!(
            events_as(&setup, who, &[]).await.status(),
            StatusCode::UNAUTHORIZED,
            "{who}"
        );
    }
    let clock = set_clock(&setup, 100).await;
    clock.store(5000, Ordering::SeqCst);
    assert_eq!(
        events_as(&setup, 1, &[]).await.status(),
        StatusCode::UNAUTHORIZED,
        "expired"
    );
    assert_eq!(session_rows(&setup).await, revoked, "nothing was written");
}

#[tokio::test(start_paused = true)]
async fn the_first_events_carry_headers_a_token_and_the_probe_projection() {
    let _locks = lock().await;
    let setup = with_sheet().await;
    assert_eq!(create_as(&setup, 3, true).await, StatusCode::SEE_OTHER);
    let response = events_as(&setup, 1, &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(headers["content-type"], "text/event-stream");
    assert_eq!(headers["x-accel-buffering"], "no");
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert!(headers.get("content-encoding").is_none());
    assert!(headers.get("access-control-allow-origin").is_none());
    let mut live = Live::new(response);
    let (token, first) = live.hello().await;
    // The same projection as the finite probe, byte for byte.
    let cookie = setup.auth[1].fixture_cookie();
    let probe = events_request(
        &setup,
        &format!("/campaigns/{}/master/snapshot", setup.campaign),
        &[("cookie", cookie.as_str())],
    )
    .await;
    let probed: serde_json::Value =
        serde_json::from_str(&super::super::formula_tests::body(probe).await).unwrap();
    assert_eq!(first, probed);
    assert_eq!(first["npcs"]["rows"].as_array().unwrap().len(), 1);
    // The token belongs to this account alone.
    assert_eq!(
        setup.store.streams().token_state(2, &token),
        TokenState::Live
    );
    assert_eq!(
        setup.store.streams().token_state(4, &token),
        TokenState::Unknown
    );
}

#[tokio::test(start_paused = true)]
async fn bytes_arrive_incrementally_after_the_boundary_with_heartbeats_and_pushes() {
    let _locks = lock().await;
    let setup = with_sheet().await;
    let mut live = open_stream(&setup, 3).await;
    let (_, first) = live.hello().await;
    assert_eq!(revision_of(&first, "players", 0), "1");
    // Well past the 5 s request boundary, the same response is still being served: a
    // comment arrives every 15 s of idleness.
    tokio::time::advance(Duration::from_secs(6)).await;
    let comment = live.event().await.unwrap();
    assert!(comment.comment, "{comment:?}");
    let comment = live.event().await.unwrap();
    assert!(comment.comment, "{comment:?}");
    // A commit pushes a snapshot to the open body, without the body ending.
    edit(&setup, 1, 1, 1, "50").await;
    let pushed = live.snapshot().await.unwrap();
    assert_eq!(revision_of(&pushed, "players", 0), "2");
    // An unchanged projection sends nothing (the hint of a no-op is absorbed): the next
    // thing on the wire is a heartbeat.
    setup.store.publish([crate::hub::Hint::Sheet {
        campaign: setup.campaign,
        sheet: 1,
        revision: 2,
    }]);
    let next = live.event().await.unwrap();
    assert!(next.comment, "{next:?}");
}

/// Reads from `socket` into `received` until it contains `needle` (real time, 20 s limit).
async fn read_until(socket: &mut tokio::net::TcpStream, received: &mut String, needle: &str) {
    use tokio::io::AsyncReadExt;
    let mut chunk = [0_u8; 4096];
    let reading = async {
        while !received.contains(needle) {
            let n = socket.read(&mut chunk).await.unwrap();
            assert!(n > 0, "the server ended the stream: {received}");
            received.push_str(&String::from_utf8_lossy(&chunk[..n]));
        }
    };
    assert!(
        tokio::time::timeout(Duration::from_secs(20), reading)
            .await
            .is_ok(),
        "timed out waiting for {needle}: {received}"
    );
}

#[tokio::test]
async fn bytes_arrive_over_a_real_socket() {
    use tokio::io::AsyncWriteExt;
    let _locks = lock().await;
    let setup = with_sheet().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            setup
                .app
                .clone()
                .into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .into_future(),
    );
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: 127.0.0.1:3000\r\nAccept: text/event-stream\r\nCookie: {}\r\n\r\n",
        events_path(&setup),
        setup.auth[3].fixture_cookie()
    );
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut received = String::new();
    read_until(&mut socket, &mut received, "event: snapshot").await;
    assert!(received.starts_with("HTTP/1.1 200"), "{received}");
    assert!(
        received
            .to_ascii_lowercase()
            .contains("content-type: text/event-stream")
    );
    assert!(received.contains("retry: 3000"));
    // A commit arrives later on the same socket (the 250 ms refresh gate, in real time).
    edit(&setup, 1, 1, 1, "50").await;
    received.clear();
    read_until(&mut socket, &mut received, "\"revision\":\"2\"").await;
    drop(socket);
    server.abort();
}

// ---------------------------------------------------------------------------------------
// Slots

#[test]
fn the_slot_caps_never_overshoot_and_release_on_drop() {
    let streams = Arc::new(crate::streams::Streams::default());
    // 16 per campaign: account 1..=16 take campaign 1; the 17th is refused.
    let mut held = Vec::new();
    for account in 1..=16 {
        held.push(streams.admit(1, account).unwrap());
    }
    assert_eq!(streams.admit(1, 17).err(), Some(Refused::Campaign));
    // 128 per instance across campaigns.
    for campaign in 2..=8_i64 {
        for n in 0..16_i64 {
            held.push(streams.admit(campaign, 100 * campaign + n).unwrap());
        }
    }
    assert_eq!(streams.counts(1).0, 128);
    assert_eq!(streams.admit(9, 999).err(), Some(Refused::Instance));
    // A Master at the caps replaces their own stream instead of overshooting.
    let again = streams.admit(1, 1).unwrap();
    assert_eq!(streams.counts(1), (128, 16));
    assert_eq!(
        streams.token_state(1, &held[0].token),
        TokenState::Replaced,
        "the evicted stream is reported as replaced"
    );
    assert_eq!(streams.token_state(1, &again.token), TokenState::Live);
    // The evicted guard releases nothing when dropped (its slot was handed over).
    drop(held.remove(0));
    assert_eq!(streams.counts(1), (128, 16));
    drop(again);
    assert_eq!(streams.counts(1), (127, 15));
    held.clear();
    assert_eq!(streams.counts(1), (0, 0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_admissions_never_overshoot() {
    let streams = Arc::new(crate::streams::Streams::default());
    let mut tasks = Vec::new();
    for account in 0..400_i64 {
        let streams = streams.clone();
        tasks.push(tokio::spawn(async move {
            streams.admit(account % 20, 1000 + account).ok()
        }));
    }
    let mut granted = Vec::new();
    for task in tasks {
        granted.extend(task.await.unwrap());
    }
    // Instance cap 128, and no campaign above 16 (20 campaigns of 20 would allow 400).
    assert_eq!(granted.len(), 128);
    for campaign in 0..20 {
        assert!(streams.counts(campaign).1 <= 16, "{campaign}");
    }
    assert_eq!(streams.counts(0).0, 128);
    drop(granted);
    assert_eq!(streams.counts(0).0, 0);
}

#[tokio::test(start_paused = true)]
async fn the_caps_answer_429_with_retry_after_6_and_503_through_the_router() {
    let _locks = lock().await;
    let setup = with_sheet().await;
    // Master C: the player promoted. A second campaign for the instance cap.
    setup
        .store
        .change_role(setup.auth[1].clone(), setup.campaign, 3, 1, true)
        .await
        .unwrap();
    let other = setup
        .store
        .create_campaign(
            setup.auth[4].clone(),
            "Second".into(),
            "synthetic-game".into(),
            "first".into(),
        )
        .await
        .unwrap();
    setup.store.streams().set_limits(Limits {
        per_campaign: 2,
        instance: 3,
    });
    let a = events_as(&setup, 1, &[]).await;
    let b = events_as(&setup, 3, &[]).await;
    assert_eq!((a.status(), b.status()), (StatusCode::OK, StatusCode::OK));
    // The campaign is full: 429, normalized, Retry-After 6, a plain body.
    let full = events_as(&setup, 2, &[]).await;
    assert_eq!(full.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(full.headers()["retry-after"], "6");
    assert_eq!(full.headers()["cache-control"], "no-store");
    assert_eq!(setup.store.streams().counts(setup.campaign), (2, 2));
    // The instance has room for one more (another campaign): taken; then 503.
    let cookie = setup.auth[4].fixture_cookie();
    let third = events_request(
        &setup,
        &format!("/campaigns/{other}/master/events"),
        &[("cookie", cookie.as_str())],
    )
    .await;
    assert_eq!(third.status(), StatusCode::OK);
    let creator = setup.auth[4].clone();
    let fourth = add_master(&setup, &creator, other, 50).await;
    let cookie = fourth.fixture_cookie();
    let refused = events_request(
        &setup,
        &format!("/campaigns/{other}/master/events"),
        &[("cookie", cookie.as_str())],
    )
    .await;
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(refused.headers().contains_key("retry-after"));
    // Refusals took no slot; dropping a response releases its slot and the next one is admitted.
    assert_eq!(setup.store.streams().counts(other).0, 3);
    drop(b);
    settle(|| setup.store.streams().counts(setup.campaign).1 == 1).await;
    assert_eq!(
        events_as(&setup, 2, &[]).await.status(),
        StatusCode::OK,
        "released slot reused"
    );
    drop((a, third));
}

#[tokio::test(start_paused = true)]
async fn slots_release_after_drop_handshake_cancellation_and_producer_failure() {
    let _locks = lock().await;
    let setup = with_sheet().await;
    let streams = setup.store.streams();
    // Drop of an unread response.
    let response = events_as(&setup, 3, &[]).await;
    assert_eq!(streams.counts(setup.campaign), (1, 1));
    drop(response);
    settle(|| streams.counts(setup.campaign).0 == 0).await;
    // A handshake cancelled after taking its slot (the 5 s boundary expires while it waits).
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let reset = GateReset::set(gate.clone());
    let cookie = setup.auth[3].fixture_cookie();
    let campaign = setup.campaign;
    let slow = tokio::spawn({
        let app = setup.app.clone();
        async move {
            use tower::ServiceExt;
            let request = axum::http::Request::builder()
                .uri(format!("/campaigns/{campaign}/master/events"))
                .header("host", "127.0.0.1:3000")
                .header("cookie", cookie)
                .body(axum::body::Body::empty())
                .unwrap();
            app.oneshot(request).await.unwrap().status()
        }
    });
    settle(|| streams.counts(setup.campaign).0 == 1).await;
    tokio::time::advance(Duration::from_secs(6)).await;
    assert_eq!(slow.await.unwrap(), StatusCode::REQUEST_TIMEOUT);
    settle(|| streams.counts(setup.campaign).0 == 0).await;
    drop(reset);
    // A handshake whose first projection fails definitively: 500, slot released.
    setup
        .store
        .run(|db| {
            db.execute_batch("ALTER TABLE master_sheet_seen RENAME TO master_sheet_seen_x")?;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        events_as(&setup, 3, &[]).await.status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(streams.counts(setup.campaign).0, 0);
    setup
        .store
        .run(|db| {
            db.execute_batch("ALTER TABLE master_sheet_seen_x RENAME TO master_sheet_seen")?;
            Ok(())
        })
        .await
        .unwrap();
    // A producer failure mid-stream: a fixed `error` notice, the body ends, the slot is free.
    let mut live = open_stream(&setup, 3).await;
    live.hello().await;
    setup
        .store
        .run(|db| {
            db.execute_batch("ALTER TABLE master_sheet_seen RENAME TO master_sheet_seen_x")?;
            Ok(())
        })
        .await
        .unwrap();
    setup.store.publish([crate::hub::Hint::Sheet {
        campaign: setup.campaign,
        sheet: 1,
        revision: 2,
    }]);
    let notice = live.data().await.unwrap();
    assert_eq!(
        (notice.name.as_str(), notice.data.as_str()),
        ("status", r#"{"code":"error"}"#)
    );
    assert!(live.event().await.is_none());
    settle(|| streams.counts(setup.campaign).0 == 0).await;
}

#[tokio::test(start_paused = true)]
async fn a_newer_tab_replaces_the_masters_older_stream_and_the_probe_says_so() {
    let _locks = lock().await;
    let setup = with_sheet().await;
    let mut old = open_stream(&setup, 3).await;
    let (old_token, _) = old.hello().await;
    let mut newest = open_stream(&setup, 3).await;
    let (new_token, snapshot) = newest.hello().await;
    assert_eq!(revision_of(&snapshot, "players", 0), "1");
    // The displaced stream flushes the fixed `replaced` notice and ends; one slot remains.
    let notice = old.data().await.unwrap();
    assert_eq!(
        (notice.name.as_str(), notice.data.as_str()),
        ("status", r#"{"code":"replaced"}"#)
    );
    assert!(old.event().await.is_none());
    assert_eq!(setup.store.streams().counts(setup.campaign), (1, 1));
    // The probe reports each tab's stream; another Master's token and a random one are unknown.
    let state =
        |headers: &axum::http::HeaderMap| headers["x-master-stream"].to_str().unwrap().to_owned();
    let (status, headers) = probe(&setup, 3, &format!("?stream={old_token}")).await;
    assert_eq!(
        (status, state(&headers).as_str()),
        (StatusCode::OK, "replaced")
    );
    assert_eq!(
        state(&probe(&setup, 3, &format!("?stream={new_token}")).await.1),
        "live"
    );
    assert_eq!(state(&probe(&setup, 3, "?stream=nope").await.1), "unknown");
    assert_eq!(
        state(&probe(&setup, 1, &format!("?stream={old_token}")).await.1),
        "unknown",
        "a token says nothing about another account"
    );
    assert!(
        probe(&setup, 3, "")
            .await
            .1
            .get("x-master-stream")
            .is_none()
    );
    assert_eq!(
        probe(&setup, 3, "?stream=").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        probe(&setup, 3, &format!("?stream={}", "x".repeat(65)))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    // The newest tab stays live and keeps receiving.
    edit(&setup, 1, 1, 1, "50").await;
    assert_eq!(
        revision_of(&newest.snapshot().await.unwrap(), "players", 0),
        "2"
    );
}

// ---------------------------------------------------------------------------------------
// Revalidation

#[derive(Clone, Copy, Debug)]
enum Revoke {
    Logout,
    Disable,
    Epoch,
    Expiry,
    Demotion,
}

async fn revoke(setup: &Setup, clock: &AtomicI64, how: Revoke) {
    match how {
        Revoke::Logout => {
            setup
                .store
                .run(|db| {
                    db.execute(
                        "UPDATE auth_sessions SET state='revoked',payload=NULL WHERE account_id=4",
                        [],
                    )?;
                    Ok(())
                })
                .await
                .unwrap();
        }
        Revoke::Disable => {
            setup
                .store
                .run(|db| {
                    db.execute(
                        "UPDATE accounts SET state='disabled',epoch=2 WHERE id=4",
                        [],
                    )?;
                    Ok(())
                })
                .await
                .unwrap();
        }
        Revoke::Epoch => {
            setup
                .store
                .run(|db| {
                    db.execute("UPDATE accounts SET epoch=2 WHERE id=4", [])?;
                    Ok(())
                })
                .await
                .unwrap();
        }
        Revoke::Expiry => clock.store(1000, Ordering::SeqCst),
        Revoke::Demotion => {
            setup
                .store
                .change_role(setup.auth[1].clone(), setup.campaign, 4, 2, false)
                .await
                .unwrap();
        }
    }
}

const ALL: [Revoke; 5] = [
    Revoke::Logout,
    Revoke::Disable,
    Revoke::Epoch,
    Revoke::Expiry,
    Revoke::Demotion,
];

#[tokio::test(start_paused = true)]
async fn an_idle_stream_closes_fail_closed_on_revocation_expiry_or_demotion() {
    let _locks = lock().await;
    for how in ALL {
        let setup = with_sheet().await;
        let clock = set_clock(&setup, 100).await;
        let mut live = open_stream(&setup, 3).await;
        live.hello().await;
        revoke(&setup, &clock, how).await;
        // A commit right after must not reach the revoked stream either (a hint is enough: the
        // Master A session itself would also be expired in the expiry case).
        setup.store.publish([crate::hub::Hint::Sheet {
            campaign: setup.campaign,
            sheet: 1,
            revision: 2,
        }]);
        assert!(
            live.ends_within(timing::RECHECK + 2 * timing::OPERATION)
                .await,
            "{how:?}"
        );
        settle(|| setup.store.streams().counts(setup.campaign).0 == 0).await;
    }
}

#[tokio::test(start_paused = true)]
async fn a_backpressured_stream_closes_fail_closed_and_discards_queued_data() {
    let _locks = lock().await;
    for how in ALL {
        let setup = with_sheet().await;
        let clock = set_clock(&setup, 100).await;
        // The body is never polled: the hello and the first snapshot stay queued.
        let response = events_as(&setup, 3, &[]).await;
        assert_eq!(response.status(), StatusCode::OK);
        revoke(&setup, &clock, how).await;
        run_for(timing::RECHECK + SECOND).await;
        settle(|| setup.store.streams().counts(setup.campaign).0 == 0).await;
        // What was queued is discarded: the body ends without the queued hello or snapshot.
        // An unread queue closes the stream after OUTPUT_BLOCK (before the 5 s recheck can
        // see the revocation), leaving at most the fixed `closing` notice in its place.
        let mut live = Live::new(response);
        let mut events = Vec::new();
        while let Some(ev) = live.event().await {
            events.push(ev);
        }
        assert!(
            events.is_empty()
                || events
                    .iter()
                    .map(|e| e.data.as_str())
                    .eq([r#"{"code":"closing"}"#]),
            "{how:?}: data after revocation: {events:?}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn the_session_expiry_is_its_own_recheck_time() {
    let _locks = lock().await;
    let setup = with_sheet().await;
    let clock = set_clock(&setup, 996).await;
    let mut live = open_stream(&setup, 3).await;
    live.hello().await;
    // Four seconds to the session's absolute expiry: the stream is closed at that instant,
    // earlier than the 5 s idle recheck.
    clock.store(1000, Ordering::SeqCst);
    run_for(Duration::from_secs(4)).await;
    assert!(live.ends_within(SECOND / 2).await);
}

#[test]
fn a_busy_database_is_transient_and_other_failures_are_definitive() {
    let busy = |code| {
        AppError::Database(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(code),
            None,
        ))
    };
    assert!(matches!(
        WorkError::from(busy(rusqlite::ffi::SQLITE_BUSY)),
        WorkError::Busy
    ));
    assert!(matches!(
        WorkError::from(busy(rusqlite::ffi::SQLITE_LOCKED)),
        WorkError::Busy
    ));
    assert!(matches!(
        WorkError::from(AppError::AuthStorageBusy),
        WorkError::Busy
    ));
    assert!(matches!(
        WorkError::from(busy(rusqlite::ffi::SQLITE_CORRUPT)),
        WorkError::Failed(_)
    ));
    assert!(matches!(
        WorkError::from(AppError::NotFound),
        WorkError::Failed(AppError::NotFound)
    ));
}

#[tokio::test(start_paused = true)]
async fn a_saturated_pool_holds_output_retries_and_recovers_within_the_grace() {
    let _locks = lock().await;
    let setup = with_sheet().await;
    let mut live = open_stream(&setup, 3).await;
    live.hello().await;
    let pool = setup.store.streams().workers();
    let held = pool
        .clone()
        .acquire_many_owned(u32::try_from(timing::WORKERS).unwrap())
        .await
        .unwrap();
    edit(&setup, 1, 1, 1, "50").await;
    // Nothing is emitted without a fresh successful recheck: for several seconds the pool is
    // out, ordinary requests still work (they use AUTH_WORKERS), and no snapshot appears.
    let pending = tokio::time::timeout(Duration::from_secs(6), live.snapshot()).await;
    assert!(
        pending.is_err(),
        "output must be held while the pool is busy"
    );
    // (`get` asserts 200.)
    get(&setup, 3, &format!("/campaigns/{}/master", setup.campaign)).await;
    // Back within the 10 s grace: the held change is delivered.
    drop(held);
    let recovered = live.snapshot().await.unwrap();
    assert_eq!(revision_of(&recovered, "players", 0), "2");
}

#[tokio::test(start_paused = true)]
async fn no_successful_recheck_for_the_grace_closes_the_stream_without_data() {
    let _locks = lock().await;
    let setup = with_sheet().await;
    let mut live = open_stream(&setup, 3).await;
    live.hello().await;
    let held = setup
        .store
        .streams()
        .workers()
        .acquire_many_owned(u32::try_from(timing::WORKERS).unwrap())
        .await
        .unwrap();
    edit(&setup, 1, 1, 1, "50").await;
    assert!(
        live.ends_within(timing::GRACE + 2 * timing::OPERATION)
            .await
    );
    drop(held);
    settle(|| setup.store.streams().counts(setup.campaign).0 == 0).await;
}

/// Holds the database mutex in a blocking task until the returned sender fires (or drops).
async fn hold_database(setup: &Setup) -> std::sync::mpsc::Sender<()> {
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let (started_tx, mut started) = tokio::sync::mpsc::unbounded_channel();
    let store = setup.store.clone();
    tokio::spawn(async move {
        store
            .run(move |_| {
                started_tx.send(()).ok();
                wait.recv().ok();
                Ok(())
            })
            .await
    });
    started.recv().await.unwrap();
    release
}

#[tokio::test(start_paused = true)]
async fn sql_contention_holds_output_and_the_stream_recovers() {
    let _locks = lock().await;
    let setup = with_sheet().await;
    let mut live = open_stream(&setup, 3).await;
    live.hello().await;
    let release = hold_database(&setup).await;
    setup.store.publish([crate::hub::Hint::Sheet {
        campaign: setup.campaign,
        sheet: 1,
        revision: 9,
    }]);
    // The projection waits for the mutex on the pool; its 1 s deadline passes and the output
    // is held while the stuck threads keep the permits.
    for _ in 0..6 {
        tokio::time::advance(Duration::from_millis(500)).await;
        yield_a_few().await;
    }
    // (No timer-based wait here: the clock is frozen while the blocking threads run.)
    tokio::select! {
        biased;
        snapshot = live.snapshot() => panic!("output while the database was held: {snapshot:?}"),
        () = yield_a_few() => {}
    }
    release.send(()).unwrap();
    // Ordinary work and the stream continue: a change reaches the stream.
    edit(&setup, 1, 1, 1, "50").await;
    assert_eq!(
        revision_of(&live.snapshot().await.unwrap(), "players", 0),
        "2"
    );
}

#[tokio::test(start_paused = true)]
async fn a_timed_out_operation_keeps_its_permit_until_it_exits_and_never_runs_its_sql() {
    let _locks = lock().await;
    let setup = with_sheet().await;
    let streams = setup.store.streams();
    let release = hold_database(&setup).await;
    let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = ran.clone();
    let (store, auth) = (setup.store.clone(), setup.auth[3].clone());
    let work = tokio::spawn({
        let streams = streams.clone();
        async move {
            streams
                .work(&store, auth, move |_| {
                    flag.store(true, Ordering::SeqCst);
                    Ok(())
                })
                .await
        }
    });
    yield_a_few().await;
    tokio::time::advance(timing::OPERATION + SECOND).await;
    assert!(matches!(work.await.unwrap(), Err(WorkError::Busy)));
    // The worker is stuck on the mutex and still owns its permit: it is not replaced.
    assert_eq!(streams.workers().available_permits(), timing::WORKERS - 1);
    release.send(()).unwrap();
    let pool = streams.workers();
    settle(|| pool.available_permits() == timing::WORKERS).await;
    assert!(
        !ran.load(Ordering::SeqCst),
        "its SQL was skipped after the deadline"
    );
}

// ---------------------------------------------------------------------------------------
// Coalescing, recovery and reconciliation

#[tokio::test(start_paused = true)]
async fn a_burst_of_hints_costs_at_most_one_projection_per_interval() {
    let _locks = lock().await;
    let setup = with_sheet().await;
    let mut live = open_stream(&setup, 3).await;
    live.hello().await;
    let before = setup.store.streams().latencies().len();
    for revision in 2..=60 {
        setup.store.publish([crate::hub::Hint::Sheet {
            campaign: setup.campaign,
            sheet: 1,
            revision,
        }]);
    }
    // The projection reads the authoritative revision (still 1), so the burst yields no
    // event, and its 59 hints cost a handful of operations, not 59.
    run_for(Duration::from_secs(3)).await;
    let operations = setup.store.streams().latencies().len() - before;
    assert!(
        (1..=3).contains(&operations),
        "{operations} operations for one burst"
    );
    // A real commit afterwards is delivered once, with the authoritative revision, even if a
    // hint for an older revision arrives after it.
    edit(&setup, 1, 1, 1, "50").await;
    setup.store.publish([crate::hub::Hint::Sheet {
        campaign: setup.campaign,
        sheet: 1,
        revision: 1,
    }]);
    let pushed = live.snapshot().await.unwrap();
    assert_eq!(revision_of(&pushed, "players", 0), "2");
    let next = live.event().await.unwrap();
    assert!(
        next.comment,
        "the stale hint produced no second snapshot: {next:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_lagged_stream_sends_a_fresh_snapshot() {
    let _locks = lock().await;
    let setup = with_sheet().await;
    let mut live = open_stream(&setup, 3).await;
    live.hello().await;
    // A change that published no hint (a name edited behind the application's back), then more
    // hints than the ring holds, all for another campaign and published without a pause, so
    // the producer finds its receiver lagged. No hint of this campaign marks it dirty: only the
    // `Lagged` recovery can deliver the change well before the 30 s reconcile.
    setup
        .store
        .run(|db| {
            // The update trigger refuses a rename; a change behind the application's back is
            // simulated without it.
            db.execute_batch("DROP TRIGGER accounts_update")?;
            db.execute("UPDATE accounts SET display_name='Renamed' WHERE id=2", [])?;
            Ok(())
        })
        .await
        .unwrap();
    for sheet in 0..i64::try_from(crate::hub::RING + 20).unwrap() {
        setup.store.publish([crate::hub::Hint::Sheet {
            campaign: 99,
            sheet,
            revision: 1,
        }]);
    }
    let fresh = tokio::time::timeout(timing::RECONCILE / 3, live.snapshot())
        .await
        .expect("a fresh snapshot long before the reconcile")
        .unwrap();
    assert_eq!(fresh["players"]["rows"][0]["custodian"], "Renamed");
}

#[tokio::test(start_paused = true)]
async fn sql_is_reconciled_every_30_seconds_without_hints() {
    let _locks = lock().await;
    let setup = with_sheet().await;
    let mut live = open_stream(&setup, 3).await;
    live.hello().await;
    setup
        .store
        .run(|db| {
            // The update trigger refuses a rename; a change behind the application's back is
            // simulated without it.
            db.execute_batch("DROP TRIGGER accounts_update")?;
            db.execute("UPDATE accounts SET display_name='Renamed' WHERE id=2", [])?;
            Ok(())
        })
        .await
        .unwrap();
    // No hint: nothing before the reconcile interval.
    assert!(
        tokio::time::timeout(
            timing::RECONCILE.checked_sub(SECOND).unwrap(),
            live.snapshot()
        )
        .await
        .is_err()
    );
    let repaired = live.snapshot().await.unwrap();
    assert_eq!(repaired["players"]["rows"][0]["custodian"], "Renamed");
}

#[tokio::test(start_paused = true)]
async fn the_stream_neither_marks_sheets_seen_nor_touches_sessions() {
    let _locks = lock().await;
    let setup = with_sheet().await;
    let seen = seen_rows(&setup).await;
    let sessions = session_rows(&setup).await;
    let mut live = open_stream(&setup, 3).await;
    live.hello().await;
    edit(&setup, 1, 1, 1, "50").await;
    let after_edit_seen = seen_rows(&setup).await;
    live.snapshot().await.unwrap();
    run_for(Duration::from_secs(40)).await;
    live.event().await.unwrap();
    assert_eq!(seen_rows(&setup).await, after_edit_seen);
    assert_ne!(
        seen,
        Vec::<String>::new(),
        "(Master A's own save marked its own sheet)"
    );
    assert_eq!(session_rows(&setup).await, sessions);
}

// ---------------------------------------------------------------------------------------
// Lifetime

#[tokio::test(start_paused = true)]
async fn a_stream_ends_with_a_notice_at_the_maximum_age() {
    let _locks = lock().await;
    let setup = with_sheet().await;
    set_clock(&setup, 1).await;
    let mut live = open_stream(&setup, 3).await;
    live.hello().await;
    let mut notice = None;
    let ended = tokio::time::timeout(timing::MAX_AGE + 2 * timing::OPERATION, async {
        while let Some(ev) = live.event().await {
            if !ev.comment {
                notice = Some(ev);
            }
        }
    })
    .await;
    assert!(ended.is_ok(), "the body ends at the maximum age");
    let notice = notice.expect("a fixed notice before the close");
    assert_eq!(notice.data, r#"{"code":"closing"}"#);
    settle(|| setup.store.streams().counts(setup.campaign).0 == 0).await;
}

#[tokio::test(start_paused = true)]
async fn an_output_blocked_for_a_second_closes_the_stream() {
    let _locks = lock().await;
    let setup = with_sheet().await;
    // A body that is never read: the first events fill the single-slot queue, a change cannot
    // be queued and the stream closes after OUTPUT_BLOCK, freeing its slot.
    let response = events_as(&setup, 3, &[]).await;
    edit(&setup, 1, 1, 1, "50").await;
    run_for(timing::REFRESH + timing::OUTPUT_BLOCK + SECOND).await;
    settle(|| setup.store.streams().counts(setup.campaign).0 == 0).await;
    let mut live = Live::new(response);
    let mut data = Vec::new();
    while let Some(ev) = live.event().await {
        data.push(ev);
    }
    // Only the closing notice replaced what was queued.
    assert_eq!(data.len(), 1, "{data:?}");
    assert_eq!(data[0].data, r#"{"code":"closing"}"#);
}

#[tokio::test(start_paused = true)]
async fn shutdown_sends_a_fixed_notice_and_closes_every_stream() {
    let _locks = lock().await;
    let setup = with_sheet().await;
    let mut a = open_stream(&setup, 1).await;
    let mut b = open_stream(&setup, 3).await;
    a.hello().await;
    b.hello().await;
    setup.store.streams().begin_shutdown();
    for live in [&mut a, &mut b] {
        let notice = live.data().await.unwrap();
        assert_eq!(notice.data, r#"{"code":"closing"}"#);
        assert!(live.event().await.is_none());
    }
    settle(|| setup.store.streams().counts(setup.campaign).0 == 0).await;
}

// ---------------------------------------------------------------------------------------
// Capacity of ordinary requests

#[tokio::test(start_paused = true)]
async fn held_streams_take_no_ordinary_request_capacity() {
    let _locks = lock().await;
    let setup = with_sheet().await;
    // More live streams than the eight REQUESTS slots, on distinct Masters and campaigns.
    let mut held = Vec::new();
    let mut accounts = Vec::new();
    for n in 0..10_i64 {
        let creator = account_session(&setup, 200 + n).await;
        let campaign = setup
            .store
            .create_campaign(
                creator.clone(),
                format!("Own {n}"),
                "synthetic-game".into(),
                "first".into(),
            )
            .await
            .unwrap();
        let cookie = creator.fixture_cookie();
        let response = events_request(
            &setup,
            &format!("/campaigns/{campaign}/master/events"),
            &[("cookie", cookie.as_str())],
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "{n}");
        held.push(Live::new(response));
        accounts.push(creator);
    }
    for live in &mut held {
        live.hello().await;
    }
    assert_eq!(setup.store.streams().counts(1).0, 10);
    // The boundary's slots and the auth workers are all free while the streams are held.
    assert_eq!(crate::auth::free_request_slots(), 8);
    assert_eq!(crate::store::AUTH_WORKERS.available_permits(), 16);
    // Eight concurrent ordinary reads, an edit and the saturated stream pool: all answered.
    let pool = setup
        .store
        .streams()
        .workers()
        .acquire_many_owned(u32::try_from(timing::WORKERS).unwrap())
        .await
        .unwrap();
    let path = format!("/campaigns/{}/master", setup.campaign);
    let reads = (0..8).map(|_| send(&setup, 1, &path, None));
    for (status, _) in futures_util::future::join_all(reads).await {
        assert_eq!(status, StatusCode::OK);
    }
    edit(&setup, 1, 1, 1, "50").await;
    drop(pool);
    assert_eq!(crate::auth::free_request_slots(), 8);
}

// ---------------------------------------------------------------------------------------
// Measurement (not part of the normal run)

fn percentile(sorted: &[Duration], per_cent: usize) -> Duration {
    sorted[(sorted.len() * per_cent / 100).min(sorted.len() - 1)]
}

fn describe(label: &str, mut samples: Vec<Duration>) {
    samples.sort();
    if samples.is_empty() {
        println!("{label}: no samples");
        return;
    }
    println!(
        "{label}: n={} p50={:?} p95={:?} max={:?}",
        samples.len(),
        percentile(&samples, 50),
        percentile(&samples, 95),
        samples[samples.len() - 1]
    );
}

/// Saturates every cap (128 live streams: 8 campaigns of 16 Masters, 40 sheets in campaign 1)
/// in real time and records the stream operations' latency, the fan-out of a commit and the
/// availability of ordinary reads, edits and the sign-in path. Run with
/// `cargo test --lib master_tests::stream::measure -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "measurement: run explicitly with --ignored --nocapture"]
#[expect(clippy::too_many_lines, reason = "one linear measurement script")]
async fn measure_saturated_caps_and_ordinary_availability() {
    use std::sync::atomic::AtomicU64;
    let _locks = lock().await;
    let setup = with_sheet().await;
    // 40 sheets (the custodian limit is 32 each): 30 of Master A, 10 of Master B.
    for (who, count) in [(1, 29), (3, 10)] {
        for _ in 0..count {
            assert_eq!(create_as(&setup, who, false).await, StatusCode::SEE_OTHER);
        }
    }
    // Campaign 1: 16 Masters (A, B, the promoted player and 13 more).
    setup
        .store
        .change_role(setup.auth[1].clone(), setup.campaign, 3, 1, true)
        .await
        .unwrap();
    let mut members: Vec<(i64, RequestAuth)> = vec![
        (setup.campaign, setup.auth[1].clone()),
        (setup.campaign, setup.auth[2].clone()),
        (setup.campaign, setup.auth[3].clone()),
    ];
    for n in 0..13 {
        let auth = add_master(&setup, &setup.auth[1].clone(), setup.campaign, 300 + n).await;
        members.push((setup.campaign, auth));
    }
    // Seven more campaigns of 16 Masters each.
    for c in 0..7_i64 {
        let creator = account_session(&setup, 400 + c).await;
        let campaign = setup
            .store
            .create_campaign(
                creator.clone(),
                format!("Load {c}"),
                "synthetic-game".into(),
                "first".into(),
            )
            .await
            .unwrap();
        members.push((campaign, creator.clone()));
        for k in 0..15 {
            let auth = add_master(&setup, &creator, campaign, 1000 + c * 20 + k).await;
            members.push((campaign, auth));
        }
    }
    assert_eq!(members.len(), 128);
    let streams = setup.store.streams();
    // Open all 128 streams; a reader task per stream drains it and records the revision of
    // the first player row (so no output queue is ever left unread).
    let mut handshakes = Vec::new();
    let mut revisions: Vec<Arc<AtomicU64>> = Vec::new();
    for (campaign, auth) in &members {
        let started = std::time::Instant::now();
        let cookie = auth.fixture_cookie();
        let response = events_request(
            &setup,
            &format!("/campaigns/{campaign}/master/events"),
            &[("cookie", cookie.as_str())],
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        handshakes.push(started.elapsed());
        let revision = Arc::new(AtomicU64::new(0));
        revisions.push(revision.clone());
        let mut live = Live::new(response);
        tokio::spawn(async move {
            while let Some(ev) = live.data().await {
                if ev.name == "snapshot" {
                    let value: serde_json::Value = serde_json::from_str(&ev.data).unwrap();
                    let r = value["players"]["rows"][0]["revision"]
                        .as_str()
                        .and_then(|r| r.parse().ok())
                        .unwrap_or(0);
                    revision.store(r, Ordering::SeqCst);
                }
            }
        });
    }
    assert_eq!(streams.counts(1).0, 128);
    describe("handshake (check, slot, first projection)", handshakes);
    let baseline = streams.latencies().len();
    // Ordinary traffic while 128 streams are held and idle rechecks run (about 26 per second
    // on two workers): 30 reads of the Master page, 30 edits of campaign 1's sheets (each one
    // wakes the 16 streams of the campaign) and the sign-in path.
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    let mut fan_out = Vec::new();
    let mut statuses = std::collections::BTreeMap::<u16, usize>::new();
    let mut revision = 1;
    for round in 0..30 {
        let started = std::time::Instant::now();
        let (status, _) = send(
            &setup,
            1,
            &format!("/campaigns/{}/master", setup.campaign),
            None,
        )
        .await;
        reads.push(started.elapsed());
        *statuses.entry(status.as_u16()).or_default() += 1;
        let started = std::time::Instant::now();
        let (status, _) = send(
            &setup,
            1,
            "/private-sheets/1",
            Some(form(
                revision,
                &[("value-hp", &format!("{}", 10 + round))],
                &[row("r1-1", "1")],
            )),
        )
        .await;
        writes.push(started.elapsed());
        *statuses.entry(status.as_u16()).or_default() += 1;
        revision += 1;
        let target = u64::try_from(revision).unwrap();
        let committed = std::time::Instant::now();
        let waited = tokio::time::timeout(Duration::from_secs(10), async {
            while revisions[..16]
                .iter()
                .any(|r| r.load(Ordering::SeqCst) < target)
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        if waited.is_ok() {
            fan_out.push(committed.elapsed());
        } else {
            println!("round {round}: fan-out did not complete in 10 s");
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let mut sign_in = Vec::new();
    for _ in 0..8 {
        let started = std::time::Instant::now();
        let response = events_request(&setup, "/auth/login", &[("accept", "text/html")]).await;
        sign_in.push(started.elapsed());
        *statuses.entry(response.status().as_u16()).or_default() += 1;
    }
    tokio::time::sleep(Duration::from_secs(12)).await;
    let operations = streams.latencies().split_off(baseline);
    describe("master page GET", reads);
    describe("sheet edit POST", writes);
    describe("sign-in form GET", sign_in);
    describe("commit to all 16 campaign streams", fan_out);
    describe("stream operations (permit wait + run)", operations);
    println!("ordinary request statuses: {statuses:?}");
    println!(
        "streams alive at the end: {}; AUTH_WORKERS free {}; REQUESTS free {}",
        streams.counts(1).0,
        crate::store::AUTH_WORKERS.available_permits(),
        crate::auth::free_request_slots()
    );
}

// ---------------------------------------------------------------------------------------
// Review additions

#[tokio::test(start_paused = true)]
async fn the_idle_recheck_alone_closes_a_revoked_stream_within_its_interval() {
    let _locks = lock().await;
    for how in [
        Revoke::Logout,
        Revoke::Disable,
        Revoke::Epoch,
        Revoke::Expiry,
        Revoke::Demotion,
    ] {
        let setup = with_sheet().await;
        let clock = set_clock(&setup, 100).await;
        let mut live = open_stream(&setup, 3).await;
        live.hello().await;
        if matches!(how, Revoke::Demotion) {
            // A demotion without its hint (as a CLI or a restore would leave it).
            setup
                .store
                .run(|db| {
                    db.execute_batch("DROP TRIGGER campaign_members_update")?;
                    db.execute(
                        "UPDATE campaign_members SET role='player' WHERE account_id=4",
                        [],
                    )?;
                    Ok(())
                })
                .await
                .unwrap();
        } else {
            revoke(&setup, &clock, how).await;
        }
        // Still open just before the idle recheck is due ...
        run_for(timing::RECHECK.checked_sub(SECOND / 2).unwrap()).await;
        tokio::select! {
            biased;
            ev = live.event() => panic!("{how:?}: ended or sent early: {ev:?}"),
            () = hop() => {}
        }
        // ... and closed, without any data, soon after it.
        run_for(SECOND / 2).await;
        assert!(live.ends_within(2 * timing::OPERATION).await, "{how:?}");
        settle(|| setup.store.streams().counts(setup.campaign).0 == 0).await;
    }
}

#[tokio::test(start_paused = true)]
async fn a_reader_that_stops_taking_events_is_closed_after_a_second_without_any_hint() {
    let _locks = lock().await;
    let setup = with_sheet().await;
    // The body is never polled: the handshake's two events stay queued. No hint, no edit.
    let response = events_as(&setup, 3, &[]).await;
    run_for(timing::OUTPUT_BLOCK / 2).await;
    assert_eq!(setup.store.streams().counts(setup.campaign).0, 1);
    run_for(timing::OUTPUT_BLOCK).await;
    settle(|| setup.store.streams().counts(setup.campaign).0 == 0).await;
    let mut live = Live::new(response);
    let mut events = Vec::new();
    while let Some(ev) = live.event().await {
        events.push(ev);
    }
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].data, r#"{"code":"closing"}"#);
}

#[tokio::test(start_paused = true)]
async fn a_handshake_that_fails_leaves_the_masters_older_stream_alone() {
    let _locks = lock().await;
    let setup = with_sheet().await;
    let mut old = open_stream(&setup, 3).await;
    let (token, _) = old.hello().await;
    let rename = |sql: &'static str| {
        let store = setup.store.clone();
        async move {
            store
                .run(move |db| {
                    db.execute_batch(sql)?;
                    Ok(())
                })
                .await
                .unwrap();
        }
    };
    rename("ALTER TABLE master_sheet_seen RENAME TO master_sheet_seen_x").await;
    // The new tab's first projection fails (500): nothing was evicted.
    assert_eq!(
        events_as(&setup, 3, &[]).await.status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    rename("ALTER TABLE master_sheet_seen_x RENAME TO master_sheet_seen").await;
    assert_eq!(
        setup.store.streams().token_state(4, &token),
        TokenState::Live
    );
    assert_eq!(setup.store.streams().counts(setup.campaign), (1, 1));
    edit(&setup, 1, 1, 1, "50").await;
    assert_eq!(
        revision_of(&old.snapshot().await.unwrap(), "players", 0),
        "2"
    );
}
