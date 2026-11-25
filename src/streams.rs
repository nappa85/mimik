//! Long-lived event streams (the Master view's server-sent events, CAMP-14): the connection
//! slots, the stream-worker pool, the shutdown signal and the single-slot output queue.
//!
//! A stream owns nothing of the ordinary request machinery while it waits: no `REQUESTS`
//! permit, no `AUTH_WORKERS` permit, no mutex and no transaction. Its only shared resources are
//! a [`Slot`] (counted, released by `Drop` on every exit path) and, for the length of one short
//! operation, one of the [`timing::WORKERS`] permits of the stream-worker pool.
use crate::{AppError, Store, auth::RequestAuth};
use axum::response::sse::Event;
use futures_util::stream;
use rusqlite::Connection;
use std::{
    collections::{HashMap, VecDeque},
    convert::Infallible,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tokio::{
    sync::{Notify, Semaphore, watch},
    time::Instant,
};

/// Every timing constant of the streams, in one place. Tests run them under tokio's paused
/// clock and advance by these values, so none has a test-only twin.
pub(crate) mod timing {
    use std::time::Duration;

    /// SSE keep-alive comment interval (a proxy read timeout must exceed it).
    pub(crate) const HEARTBEAT: Duration = Duration::from_secs(15);
    /// The `retry:` field advertised to the browser's `EventSource`.
    pub(crate) const RETRY: Duration = Duration::from_millis(3000);
    /// Permits of the stream-worker pool, separate from `AUTH_WORKERS`.
    pub(crate) const WORKERS: usize = 2;
    /// Deadline of one projection or recheck, the wait for a pool permit included.
    pub(crate) const OPERATION: Duration = Duration::from_secs(1);
    /// A busy or timed-out operation is retried after this delay (output held meanwhile).
    pub(crate) const RETRY_DELAY: Duration = Duration::from_millis(250);
    /// Recheck of an idle stream.
    pub(crate) const RECHECK: Duration = Duration::from_secs(5);
    /// A stream closes when no recheck has succeeded for this long.
    pub(crate) const GRACE: Duration = Duration::from_secs(10);
    /// At most one projection per stream in this interval.
    pub(crate) const REFRESH: Duration = Duration::from_millis(250);
    /// Reconciliation from SQL without any hint (repairs a missed publication).
    pub(crate) const RECONCILE: Duration = Duration::from_secs(30);
    /// A stream closes when its output has been blocked this long.
    pub(crate) const OUTPUT_BLOCK: Duration = Duration::from_secs(1);
    /// Maximum age of one connection; the client reconnects with a full snapshot.
    pub(crate) const MAX_AGE: Duration = Duration::from_mins(15);
    /// How long the server stays up after the shutdown signal so streams can flush a notice.
    pub(crate) const SHUTDOWN_DRAIN: Duration = Duration::from_millis(500);
    /// Replaced-stream tokens remembered per account (for the snapshot probe).
    pub(crate) const REMEMBERED_REPLACED: usize = 8;
    /// Accounts whose replaced tokens are remembered at once.
    pub(crate) const REMEMBERED_ACCOUNTS: usize = 1024;
}

/// Connection caps. A Master has at most one live stream (the newest evicts the oldest).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Limits {
    pub per_campaign: usize,
    pub instance: usize,
}

impl Limits {
    /// Owner decision 5 (2026-11-05): 16 per campaign, 128 per instance.
    pub(crate) const PRODUCTION: Self = Self {
        per_campaign: 16,
        instance: 128,
    };
}

/// Why a stream was refused a slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// The campaign already has its streams: 429.
    Campaign,
    /// The instance already has its streams (or no random token could be made): 503.
    Instance,
}

/// What the snapshot probe reports for a tab's stream token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TokenState {
    /// This is the account's current stream.
    Live,
    /// A newer stream of the same account displaced it: the tab must not reclaim a slot.
    Replaced,
    /// Never seen, ended another way or forgotten: the tab may reconnect.
    Unknown,
}

struct Entry {
    id: u64,
    campaign: i64,
    token: String,
    evict: watch::Sender<bool>,
}

#[derive(Default)]
struct Registry {
    instance: usize,
    campaigns: HashMap<i64, usize>,
    by_account: HashMap<i64, Entry>,
    replaced: HashMap<i64, VecDeque<String>>,
    limits: Option<Limits>,
}

impl Registry {
    fn limits(&self) -> Limits {
        self.limits.unwrap_or(Limits::PRODUCTION)
    }

    fn release(&mut self, campaign: i64) {
        self.instance = self.instance.saturating_sub(1);
        if let Some(count) = self.campaigns.get_mut(&campaign) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.campaigns.remove(&campaign);
            }
        }
    }

    fn remember_replaced(&mut self, account: i64, token: String) {
        if !self.replaced.contains_key(&account)
            && self.replaced.len() >= timing::REMEMBERED_ACCOUNTS
        {
            self.replaced.clear();
        }
        let tokens = self.replaced.entry(account).or_default();
        tokens.push_back(token);
        while tokens.len() > timing::REMEMBERED_REPLACED {
            tokens.pop_front();
        }
    }
}

/// The streams of one store: slots, worker pool and the shutdown signal. Shared by clones of
/// the [`Store`], separate between stores.
pub(crate) struct Streams {
    registry: Mutex<Registry>,
    workers: Arc<Semaphore>,
    shutdown: watch::Sender<bool>,
    next: AtomicU64,
    #[cfg(test)]
    latencies: Mutex<Vec<std::time::Duration>>,
}

impl Default for Streams {
    fn default() -> Self {
        Self {
            registry: Mutex::default(),
            workers: Arc::new(Semaphore::new(timing::WORKERS)),
            shutdown: watch::channel(false).0,
            next: AtomicU64::new(1),
            #[cfg(test)]
            latencies: Mutex::default(),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A counted connection slot; dropping it releases the slot (unless it was evicted, which
/// already released it).
pub(crate) struct Slot {
    streams: Arc<Streams>,
    account: i64,
    id: u64,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut registry = lock(&self.streams.registry);
        if registry
            .by_account
            .get(&self.account)
            .is_some_and(|entry| entry.id == self.id)
            && let Some(entry) = registry.by_account.remove(&self.account)
        {
            registry.release(entry.campaign);
        }
    }
}

/// A granted slot with the stream's eviction signal and public token.
pub(crate) struct Admission {
    pub slot: Slot,
    pub evicted: watch::Receiver<bool>,
    pub token: String,
}

fn new_token() -> Option<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).ok()?;
    Some(hex::encode(bytes))
}

impl Streams {
    /// Admits a stream of `account` to `campaign`, atomically and without overshooting a cap.
    /// The account's previous stream, if any, is evicted in the same step (its slot is freed
    /// first, so the replacement may take it); it then only flushes a fixed `replaced` notice.
    ///
    /// # Errors
    /// [`Refused::Campaign`] or [`Refused::Instance`] when the replacement would exceed a cap.
    pub(crate) fn admit(
        self: &Arc<Self>,
        campaign: i64,
        account: i64,
    ) -> Result<Admission, Refused> {
        let token = new_token().ok_or(Refused::Instance)?;
        let (evict, evicted) = watch::channel(false);
        let mut registry = lock(&self.registry);
        let limits = registry.limits();
        let old = registry
            .by_account
            .get(&account)
            .map(|entry| entry.campaign);
        let in_campaign = registry.campaigns.get(&campaign).copied().unwrap_or(0)
            - usize::from(old == Some(campaign));
        let on_instance = registry.instance - usize::from(old.is_some());
        if in_campaign >= limits.per_campaign {
            return Err(Refused::Campaign);
        }
        if on_instance >= limits.instance {
            return Err(Refused::Instance);
        }
        if let Some(previous) = registry.by_account.remove(&account) {
            registry.release(previous.campaign);
            previous.evict.send_replace(true);
            registry.remember_replaced(account, previous.token);
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        registry.instance += 1;
        *registry.campaigns.entry(campaign).or_default() += 1;
        registry.by_account.insert(
            account,
            Entry {
                id,
                campaign,
                token: token.clone(),
                evict,
            },
        );
        drop(registry);
        Ok(Admission {
            slot: Slot {
                streams: self.clone(),
                account,
                id,
            },
            evicted,
            token,
        })
    }

    /// What the account's own `token` is: only the account's streams are consulted, so a token
    /// says nothing about anyone else.
    pub(crate) fn token_state(&self, account: i64, token: &str) -> TokenState {
        let registry = lock(&self.registry);
        if registry
            .by_account
            .get(&account)
            .is_some_and(|entry| entry.token == token)
        {
            TokenState::Live
        } else if registry
            .replaced
            .get(&account)
            .is_some_and(|tokens| tokens.iter().any(|t| t == token))
        {
            TokenState::Replaced
        } else {
            TokenState::Unknown
        }
    }

    /// Live streams on the instance and in `campaign`.
    #[cfg(test)]
    pub(crate) fn counts(&self, campaign: i64) -> (usize, usize) {
        let registry = lock(&self.registry);
        (
            registry.instance,
            registry.campaigns.get(&campaign).copied().unwrap_or(0),
        )
    }

    /// Test-only: replaces the caps.
    #[cfg(test)]
    pub(crate) fn set_limits(&self, limits: Limits) {
        lock(&self.registry).limits = Some(limits);
    }

    /// Test-only: the pool, so a test can hold its permits.
    #[cfg(test)]
    pub(crate) fn workers(&self) -> Arc<Semaphore> {
        self.workers.clone()
    }

    /// Test-only: durations (permit wait included) of the operations run so far.
    #[cfg(test)]
    pub(crate) fn latencies(&self) -> Vec<std::time::Duration> {
        lock(&self.latencies).clone()
    }

    /// Signals every stream to send its fixed notice and close.
    pub(crate) fn begin_shutdown(&self) {
        self.shutdown.send_replace(true);
    }

    pub(crate) fn shutdown_signal(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    /// One short database operation of a stream, on the stream-worker pool: a fair (FIFO)
    /// wait for a permit counted inside the [`timing::OPERATION`] deadline, then the work on a
    /// blocking thread that owns the permit until it exits (a timed-out worker is never
    /// replaced while running). The operation is skipped if its deadline passed or its caller
    /// went away before it got the database mutex. Nothing here uses `AUTH_WORKERS`.
    ///
    /// # Errors
    /// [`WorkError::Busy`] on a full pool, the deadline, or a locked database;
    /// [`WorkError::Failed`] with the operation's error otherwise.
    pub(crate) async fn work<T: Send + 'static>(
        &self,
        store: &Store,
        auth: RequestAuth,
        operation: impl FnOnce(&mut Connection) -> Result<T, AppError> + Send + 'static,
    ) -> Result<T, WorkError> {
        struct Cancel(Arc<AtomicBool>);
        impl Drop for Cancel {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let started = Instant::now();
        let Ok(Ok(permit)) =
            tokio::time::timeout(timing::OPERATION, self.workers.clone().acquire_owned()).await
        else {
            return Err(WorkError::Busy);
        };
        let cancel = Cancel(Arc::new(AtomicBool::new(false)));
        let task = store.stream_operation(permit, cancel.0.clone(), auth, operation);
        let remaining = timing::OPERATION.saturating_sub(started.elapsed());
        let result = match tokio::time::timeout(remaining, task).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(error))) => Err(WorkError::from(error)),
            Ok(Err(_)) => Err(WorkError::Failed(AppError::Worker)),
            Err(_) => Err(WorkError::Busy),
        };
        #[cfg(test)]
        lock(&self.latencies).push(started.elapsed());
        result
    }
}

/// The outcome of a failed stream operation.
#[derive(Debug)]
pub(crate) enum WorkError {
    /// Transient: hold output and retry.
    Busy,
    /// Definitive.
    Failed(AppError),
}

impl From<AppError> for WorkError {
    fn from(error: AppError) -> Self {
        match error {
            AppError::AuthStorageBusy => Self::Busy,
            AppError::Database(rusqlite::Error::SqliteFailure(code, _))
                if matches!(
                    code.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                ) =>
            {
                Self::Busy
            }
            other => Self::Failed(other),
        }
    }
}

struct OutState {
    queue: VecDeque<Event>,
    closed: bool,
}

struct OutInner {
    state: Mutex<OutState>,
    readable: Notify,
    space: Notify,
    gone: watch::Sender<bool>,
}

/// The producer's end of the single-slot output queue: it holds at most the events not yet
/// taken by the body, a close discards them, and it never depends on the body being polled.
pub(crate) struct Outbox(Arc<OutInner>);

/// The body's end: dropping it (a closed connection, a cancelled response) tells the producer.
pub(crate) struct Reader(Arc<OutInner>);

impl Drop for Reader {
    fn drop(&mut self) {
        self.0.gone.send_replace(true);
    }
}

impl Outbox {
    /// A queue seeded with the first `events` (the handshake's, already authorized).
    pub(crate) fn new(seed: impl IntoIterator<Item = Event>) -> (Self, Reader) {
        let inner = Arc::new(OutInner {
            state: Mutex::new(OutState {
                queue: seed.into_iter().collect(),
                closed: false,
            }),
            readable: Notify::new(),
            space: Notify::new(),
            gone: watch::channel(false).0,
        });
        (Self(inner.clone()), Reader(inner))
    }

    /// Whether events wait for the body.
    pub(crate) fn is_full(&self) -> bool {
        !lock(&self.0.state).queue.is_empty()
    }

    /// Queues `event` when the queue is empty (only the producer adds, so an empty queue stays
    /// empty until it does); `false` when it is not or the queue is closed.
    pub(crate) fn push(&self, event: Event) -> bool {
        let mut state = lock(&self.0.state);
        if state.closed || !state.queue.is_empty() {
            return false;
        }
        state.queue.push_back(event);
        drop(state);
        self.0.readable.notify_one();
        true
    }

    /// Discards everything queued, optionally leaves one fixed `notice`, and ends the body
    /// after it.
    pub(crate) fn close(&self, notice: Option<Event>) {
        let mut state = lock(&self.0.state);
        state.queue.clear();
        state.queue.extend(notice);
        state.closed = true;
        drop(state);
        self.0.readable.notify_one();
    }

    /// Resolves when the body took an event (a permit is kept if it did so earlier, so a
    /// spurious wake is possible and callers re-check [`Self::is_full`]).
    pub(crate) async fn space(&self) {
        self.0.space.notified().await;
    }

    pub(crate) fn gone(&self) -> watch::Receiver<bool> {
        self.0.gone.subscribe()
    }
}

impl Reader {
    /// The body stream: events in order, ending after a close. Cancel-safe.
    pub(crate) fn into_stream(
        self,
    ) -> impl futures_util::Stream<Item = Result<Event, Infallible>> + Send {
        stream::unfold(self, |reader| async move {
            loop {
                let inner = reader.0.clone();
                let readable = inner.readable.notified();
                {
                    let mut state = lock(&reader.0.state);
                    if let Some(event) = state.queue.pop_front() {
                        drop(state);
                        reader.0.space.notify_one();
                        return Some((Ok(event), reader));
                    }
                    if state.closed {
                        return None;
                    }
                }
                readable.await;
            }
        })
    }
}

/// `true` when the signal is set; a vanished sender counts as set.
pub(crate) async fn raised(signal: &mut watch::Receiver<bool>) {
    let _ = signal.wait_for(|set| *set).await;
}

/// The fixed code of a status event; the only content such an event carries besides the
/// stream token of `live`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Live,
    Replaced,
    Closing,
    Error,
}

impl Status {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Replaced => "replaced",
            Self::Closing => "closing",
            Self::Error => "error",
        }
    }

    /// The named `status` event; `token` only for `live`.
    pub(crate) fn event(self, token: Option<&str>) -> Event {
        let data = match token {
            Some(token) => serde_json::json!({"code": self.code(), "stream": token}),
            None => serde_json::json!({"code": self.code()}),
        };
        Event::default().event("status").data(data.to_string())
    }
}

/// The SSE `retry:` advertisement, carried by the first status event of a connection.
pub(crate) fn with_retry(event: Event) -> Event {
    event.retry(timing::RETRY)
}
