//! Interpretation jobs (assisted sheet plan §1.4, "Jobs", "Origin binding", "Routes"): one
//! worker, admission within the queue budget, at most 64 records, supersession, the
//! note-write map, the runtime event interface and the breaker.
//!
//! Everything here is in memory and per process. The table's lock covers every
//! check-then-enqueue and every post-commit note-write hook; it is never held across a
//! database wait, the permit wait or the model call.
use super::breaker::{self, Admission, Breaker};
use super::interpreter::{Completion, Failure, Interpreter};
use super::output::{self, Candidate, Reading};
use super::request::{self, Bounds, Draft, NotRendered};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::Instant;

/// The most job records, live and finished.
pub(crate) const MAX_RECORDS: usize = 64;
/// How long options stay applicable after their job finished.
pub(crate) const OPTION_TTL: Duration = Duration::from_secs(600);

/// The budgets a generation's `ready` carries (plan §1.4a): B, N(B), R, D, Q.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Budgets {
    pub bounds: Bounds,
    /// R: a job's run deadline.
    pub run: Duration,
    /// D: how many jobs may wait (the running one is not counted).
    pub depth: usize,
    /// Q: the most a job may wait in the queue.
    pub queue_wait: Duration,
    /// The model's `max_tokens`.
    pub max_tokens: u32,
    /// How long the server may still be evaluating a request cancelled at its run deadline
    /// (the calibrated prompt term at B): the next job waits that long, and admission counts
    /// it.
    pub busy_after_expiry: Duration,
}

/// What the worker tells the runtime manager about exchanges (plan §1.4a, "Transitions":
/// two consecutive run-deadline expiries with no completed exchange between stop the child
/// as hung).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Signal {
    Expired { generation: u64 },
    Completed { generation: u64 },
}

/// Why a generation ended (the runtime manager sends these).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EndReason {
    /// The child exited, or was stopped for a new generation.
    Exited,
    /// The owner's Restart.
    Restart,
    /// The runtime cannot run (the reason is shown).
    Unavailable(String),
}

/// The runtime event interface the worker consumes: fed by the runtime manager (and by test
/// doubles).
pub(crate) enum RuntimeEvent {
    GenerationStarted {
        generation: u64,
    },
    GenerationEnded {
        generation: u64,
        reason: EndReason,
    },
    Ready {
        generation: u64,
        budgets: Budgets,
        interpreter: Arc<dyn Interpreter>,
    },
}

/// The runtime's state as a start or a poll reports it first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeState {
    Starting,
    Ready,
    Unavailable(String),
}

enum Runtime {
    Starting {
        generation: Option<u64>,
    },
    Ready {
        generation: u64,
        budgets: Budgets,
        interpreter: Arc<dyn Interpreter>,
    },
    Unavailable(String),
}

/// A job's 128-bit identity, as 32 lowercase hex digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct JobId([u8; 16]);

impl JobId {
    fn random() -> Self {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).expect("the OS random source is available");
        Self(bytes)
    }

    pub(crate) fn parse(text: &str) -> Option<Self> {
        if text.len() != 32 || !text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return None;
        }
        let mut bytes = [0; 16];
        hex::decode_to_slice(text, &mut bytes).ok()?;
        Some(Self(bytes))
    }

    pub(crate) fn text(self) -> String {
        hex::encode(self.0)
    }
}

/// What a job is bound to (plan §1.4, "Origin binding"): the sheet, the target's typed
/// address (a row cell by host ID) and the origin note's revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Origin {
    pub sheet: i64,
    pub target: String,
    pub revision: i64,
}

/// A validated option with its bound read/write set (plan §1.4, "Proposal freshness").
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Proposal {
    pub candidate: Candidate,
    /// The target's value at the job's base revision (`None`: unknown).
    pub before: Option<i64>,
    /// The expression as shown, with `−`, `×`, `÷`.
    pub shown: String,
    /// SHA-256 of the bound content.
    pub hash: [u8; 32],
}

/// How a job ended, as a poll reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Answer {
    Options(Vec<Proposal>),
    Readings(Vec<String>),
    /// A valid reply with no option left (or no option at all).
    Nothing,
    /// The phrase and target alone exceed a bound.
    TooLong,
    /// The breaker is open, or the exchange failed.
    Unavailable,
    /// The generation changed.
    Restarted,
    /// Queue wait or run deadline expired, or the record was evicted.
    AskAgain,
    /// A newer job of the account replaced it.
    Superseded,
    /// A note write made its origin non-current.
    Stale,
}

enum State {
    Queued,
    Running { cancel: Arc<Notify> },
    Done { answer: Answer, at: Instant },
}

struct Job {
    id: JobId,
    ticket: breaker::Ticket,
    account: i64,
    origin: Origin,
    enqueued: Instant,
    started: Option<Instant>,
    state: State,
    draft: Option<Draft>,
    applied: bool,
}

impl Job {
    /// What is left of a running job's run deadline `run` at `now`.
    fn started_left(&self, run: Duration, now: Instant) -> Duration {
        self.started.map_or(run, |started| {
            (started + run).saturating_duration_since(now)
        })
    }

    fn live(&self) -> bool {
        matches!(self.state, State::Queued | State::Running { .. })
    }

    /// Finished with options that are still applicable at `now`.
    fn offering(&self, now: Instant) -> bool {
        matches!(&self.state, State::Done { answer: Answer::Options(options), at }
            if !options.is_empty() && !self.applied && now < *at + OPTION_TTL)
    }
}

/// The note-write map of one sheet (plan §1.4, "Routes"): the latest note-write revision
/// per target, and the origin revisions registered by starts in flight.
#[derive(Default)]
struct SheetWrites {
    latest: BTreeMap<String, i64>,
    registered: BTreeMap<i64, usize>,
}

#[derive(Default)]
struct Table {
    jobs: Vec<Job>,
    queue: VecDeque<JobId>,
    writes: BTreeMap<i64, SheetWrites>,
    next_ticket: breaker::Ticket,
}

/// Why a start is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// A newer note for the target exists, or the account has a newer live job on the
    /// sheet (409).
    Conflict,
    /// The queue cannot take it in time, the account has a live job on another sheet, or
    /// 64 live jobs exist (429).
    Busy,
    /// The runtime is not ready (503, with its state).
    Runtime(RuntimeState),
    /// The breaker refuses (503).
    Unavailable,
}

/// A start's registration of its origin revision, held from before its database check to
/// its end, so the note-write map keeps the entries it needs.
pub(crate) struct Registration {
    sheet: i64,
    revision: i64,
    assistant: Arc<Assistant>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        let mut table = self.assistant.table();
        if let Some(writes) = table.writes.get_mut(&self.sheet)
            && let Some(count) = writes.registered.get_mut(&self.revision)
        {
            *count -= 1;
            if *count == 0 {
                writes.registered.remove(&self.revision);
            }
        }
        prune(&mut table, self.sheet);
    }
}

/// A job as a poll sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Poll {
    Queued,
    Running,
    Done(Answer),
    /// Unknown, evicted, another account's or sheet's: not found.
    Missing,
}

/// How long a transport error waits for its generation's end to be published.
const TRANSPORT_GRACE: Duration = Duration::from_secs(1);

/// The assistant: the job table, the worker's wake-up, the runtime state and the breaker.
pub(crate) struct Assistant {
    table: Mutex<Table>,
    runtime: Mutex<Runtime>,
    breaker: Mutex<Breaker>,
    wake: Notify,
    /// Signalled after every runtime event (a generation ended).
    changed: Notify,
    /// How long a transport error waits for its generation's end to be published.
    grace: Mutex<Duration>,
    /// Until when the server is presumed busy with a request cancelled at its deadline.
    busy_until: Mutex<Option<Instant>>,
    signals: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Signal>>>,
    /// How often the breaker was reset (tests).
    #[cfg(test)]
    pub(crate) resets: std::sync::atomic::AtomicUsize,
}

impl Assistant {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            table: Mutex::new(Table::default()),
            runtime: Mutex::new(Runtime::Starting { generation: None }),
            breaker: Mutex::new(Breaker::new()),
            wake: Notify::new(),
            changed: Notify::new(),
            grace: Mutex::new(TRANSPORT_GRACE),
            busy_until: Mutex::new(None),
            signals: Mutex::new(None),
            #[cfg(test)]
            resets: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    fn table(&self) -> MutexGuard<'_, Table> {
        self.table
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn breaker(&self) -> MutexGuard<'_, Breaker> {
        self.breaker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn runtime(&self) -> MutexGuard<'_, Runtime> {
        self.runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The runtime state, reported before the breaker.
    pub(crate) fn state(&self) -> RuntimeState {
        match &*self.runtime() {
            Runtime::Starting { .. } => RuntimeState::Starting,
            Runtime::Ready { .. } => RuntimeState::Ready,
            Runtime::Unavailable(reason) => RuntimeState::Unavailable(reason.clone()),
        }
    }

    /// The current generation's budgets, when ready.
    pub(crate) fn budgets(&self) -> Option<Budgets> {
        match &*self.runtime() {
            Runtime::Ready { budgets, .. } => Some(*budgets),
            _ => None,
        }
    }

    /// Feeds one runtime event. A generation change ends every queued and running job with
    /// "the assistant restarted: ask again", neutral for the breaker; a child exit withdraws
    /// its recent counted failures. A new child does not reset the breaker.
    pub(crate) fn event(&self, event: RuntimeEvent) {
        let now = Instant::now();
        match event {
            RuntimeEvent::GenerationStarted { generation } => {
                *self.runtime() = Runtime::Starting {
                    generation: Some(generation),
                };
            }
            RuntimeEvent::Ready {
                generation,
                budgets,
                interpreter,
            } => {
                let current = match &*self.runtime() {
                    Runtime::Starting { generation } => *generation,
                    _ => None,
                };
                if current.is_none_or(|current| current == generation) {
                    *self.runtime() = Runtime::Ready {
                        generation,
                        budgets,
                        interpreter,
                    };
                    self.wake.notify_one();
                }
            }
            RuntimeEvent::GenerationEnded { generation, reason } => {
                let current = match &*self.runtime() {
                    Runtime::Starting { generation } => *generation,
                    Runtime::Ready { generation, .. } => Some(*generation),
                    Runtime::Unavailable(_) => None,
                };
                if current.is_some_and(|current| current != generation) {
                    return;
                }
                *self.runtime() = match reason {
                    EndReason::Unavailable(why) => Runtime::Unavailable(why),
                    _ => Runtime::Starting { generation: None },
                };
                self.breaker().generation_ended(generation, now.into_std());
                *self
                    .busy_until
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                let mut table = self.table();
                table.queue.clear();
                for job in table.jobs.iter_mut().filter(|job| job.live()) {
                    self.end(job, Answer::Restarted, now);
                }
            }
        }
        self.changed.notify_waiters();
    }

    /// The breaker's state, for the assistant page.
    pub(crate) fn breaker_state(&self) -> String {
        self.breaker().describe(Instant::now().into_std())
    }

    /// Where expiries and completed exchanges are reported (the runtime manager).
    pub(crate) fn subscribe(&self, sender: tokio::sync::mpsc::UnboundedSender<Signal>) {
        *self
            .signals
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sender);
    }

    /// The owner's Restart or the app's start: the breaker is reset.
    pub(crate) fn reset_breaker(&self) {
        #[cfg(test)]
        self.resets
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.breaker().reset();
    }

    /// Registers a start's origin revision before its database check.
    pub(crate) fn register(self: &Arc<Self>, sheet: i64, revision: i64) -> Registration {
        let mut table = self.table();
        *table
            .writes
            .entry(sheet)
            .or_default()
            .registered
            .entry(revision)
            .or_default() += 1;
        Registration {
            sheet,
            revision,
            assistant: Arc::clone(self),
        }
    }

    /// Starts a job after the database check found the origin note current (plan §1.4,
    /// "Routes", in its order): the note-write map, a repeat, an older origin, another
    /// sheet, the runtime, the breaker and capacity (as if the superseded job were gone),
    /// then supersession and enqueue. Returns the job and whether it is a repeat.
    pub(crate) fn start(
        &self,
        registration: &Registration,
        account: i64,
        origin: Origin,
        draft: Draft,
    ) -> Result<(JobId, bool), Refusal> {
        debug_assert_eq!(registration.sheet, origin.sheet);
        let now = Instant::now();
        let mut table = self.table();
        let newer_write = table.writes.get(&origin.sheet).is_some_and(|writes| {
            writes
                .latest
                .iter()
                .any(|(target, latest)| *latest > origin.revision && covers(target, &origin.target))
        });
        if newer_write {
            return Err(Refusal::Conflict);
        }
        if let Some(job) = table.jobs.iter().find(|job| {
            job.account == account && job.origin == origin && (job.live() || job.offering(now))
        }) {
            return Ok((job.id, true));
        }
        let live = table
            .jobs
            .iter()
            .find(|job| job.account == account && job.live());
        let superseded = match live {
            Some(job) if job.origin.sheet != origin.sheet => return Err(Refusal::Busy),
            Some(job) if job.origin.revision > origin.revision => return Err(Refusal::Conflict),
            Some(job) => Some((job.id, job.ticket)),
            None => None,
        };
        let budgets = match &*self.runtime() {
            Runtime::Ready { budgets, .. } => *budgets,
            Runtime::Starting { .. } => return Err(Refusal::Runtime(RuntimeState::Starting)),
            Runtime::Unavailable(reason) => {
                return Err(Refusal::Runtime(RuntimeState::Unavailable(reason.clone())));
            }
        };
        let ticket = table.next_ticket;
        // Without side effects, so a refused start changes nothing.
        self.admissible(&table, budgets, superseded, now)?;
        if self
            .breaker()
            .admit(ticket, now.into_std(), superseded.map(|(_, t)| t))
            == Admission::Refused
        {
            return Err(Refusal::Unavailable);
        }
        table.next_ticket += 1;
        if let Some((id, _)) = superseded {
            table.queue.retain(|queued| *queued != id);
            if let Some(job) = table.jobs.iter_mut().find(|job| job.id == id) {
                self.end(job, Answer::Superseded, now);
            }
        }
        if table.jobs.len() >= MAX_RECORDS {
            let oldest = table
                .jobs
                .iter()
                .enumerate()
                .filter(|(_, job)| !job.live())
                .min_by_key(|(_, job)| match job.state {
                    State::Done { at, .. } => at,
                    _ => now,
                })
                .map(|(index, _)| index);
            // `None` is not reached: `admissible` refused 64 live jobs (the superseded one
            // excepted, and it has just ended, so it is evictable). Defensively, a ticket the
            // breaker may have made the probe gets its neutral outcome, so no probe stays
            // reserved for a job that was never created.
            let Some(index) = oldest else {
                self.breaker()
                    .record(ticket, breaker::Outcome::Neutral, now.into_std());
                return Err(Refusal::Busy);
            };
            table.jobs.remove(index);
        }
        let id = JobId::random();
        table.jobs.push(Job {
            id,
            ticket,
            account,
            origin,
            enqueued: now,
            started: None,
            state: State::Queued,
            draft: Some(draft),
            applied: false,
        });
        table.queue.push_back(id);
        drop(table);
        self.wake.notify_one();
        Ok((id, false))
    }

    /// The post-commit hook of every note writer (saves, Undo, switch, row removal, apply):
    /// `targets` got notes in `revision`. Jobs whose origin they make non-current are
    /// cancelled (running or queued) or lose their options (finished), in one critical
    /// section with the map's update. A `row:` address covers every cell of its row.
    pub(crate) fn note_written(&self, sheet: i64, revision: i64, targets: &[String]) {
        let now = Instant::now();
        let mut table = self.table();
        let writes = table.writes.entry(sheet).or_default();
        for target in targets {
            let latest = writes.latest.entry(target.clone()).or_insert(revision);
            *latest = (*latest).max(revision);
        }
        let stale_origin = |origin: &Origin| {
            origin.sheet == sheet
                && origin.revision < revision
                && targets.iter().any(|target| covers(target, &origin.target))
        };
        let mut stale = Vec::new();
        for job in &mut table.jobs {
            if stale_origin(&job.origin) && (job.live() || job.offering(now)) {
                stale.push(job.id);
                self.end(job, Answer::Stale, now);
            }
        }
        table.queue.retain(|id| !stale.contains(id));
        prune(&mut table, sheet);
    }

    /// What a poll by `account` on `sheet` sees of job `id`.
    pub(crate) fn poll(&self, account: i64, sheet: i64, id: JobId) -> Poll {
        let now = Instant::now();
        let table = self.table();
        let Some(job) = table
            .jobs
            .iter()
            .find(|job| job.id == id && job.account == account && job.origin.sheet == sheet)
        else {
            return Poll::Missing;
        };
        match &job.state {
            State::Queued => Poll::Queued,
            State::Running { .. } => Poll::Running,
            State::Done {
                answer: Answer::Options(_),
                ..
            } if !job.offering(now) => Poll::Done(Answer::AskAgain),
            State::Done { answer, .. } => Poll::Done(answer.clone()),
        }
    }

    /// The option `n` of `account`'s finished job `id` on `sheet`, with its origin, while
    /// it is applicable; it stays applicable until [`Self::consume`].
    pub(crate) fn proposal(
        &self,
        account: i64,
        sheet: i64,
        id: JobId,
        n: usize,
    ) -> Option<(Origin, Proposal)> {
        let now = Instant::now();
        let table = self.table();
        let job = table
            .jobs
            .iter()
            .find(|job| job.id == id && job.account == account && job.origin.sheet == sheet)?;
        if !job.offering(now) {
            return None;
        }
        let State::Done {
            answer: Answer::Options(options),
            ..
        } = &job.state
        else {
            return None;
        };
        options
            .get(n)
            .map(|option| (job.origin.clone(), option.clone()))
    }

    /// Marks job `id` applied, after the apply's commit: options are alternatives.
    pub(crate) fn consume(&self, id: JobId) {
        if let Some(job) = self.table().jobs.iter_mut().find(|job| job.id == id) {
            job.applied = true;
        }
    }

    /// The worker: runs one job at a time until the process ends.
    pub(crate) async fn work(self: Arc<Self>) {
        loop {
            if let Some(until) = self.busy() {
                tokio::select! {
                    () = tokio::time::sleep_until(until) => {}
                    () = self.wake.notified() => {}
                }
                continue;
            }
            match self.next() {
                Some(run) => self.run(run).await,
                None => self.wake.notified().await,
            }
        }
    }

    /// The end of the presumed busy time after a deadline cancel, while it lasts.
    fn busy(&self) -> Option<Instant> {
        let mut busy = self
            .busy_until
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match *busy {
            Some(until) if Instant::now() < until => Some(until),
            _ => {
                *busy = None;
                None
            }
        }
    }

    fn signal(&self, signal: Signal) {
        if let Some(sender) = &*self
            .signals
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            let _ = sender.send(signal);
        }
    }

    /// Takes the next runnable job, ending the ones that can no longer run.
    fn next(&self) -> Option<Run> {
        let now = Instant::now();
        let (budgets, generation, interpreter) = match &*self.runtime() {
            Runtime::Ready {
                budgets,
                generation,
                interpreter,
            } => (*budgets, *generation, Arc::clone(interpreter)),
            _ => return None,
        };
        let mut table = self.table();
        while let Some(id) = table.queue.pop_front() {
            let Some(job) = table.jobs.iter_mut().find(|job| job.id == id) else {
                continue;
            };
            if now > job.enqueued + budgets.queue_wait {
                self.end(job, Answer::AskAgain, now);
                continue;
            }
            if !self.breaker().may_run(job.ticket) {
                self.end(job, Answer::Unavailable, now);
                continue;
            }
            let cancel = Arc::new(Notify::new());
            job.state = State::Running {
                cancel: Arc::clone(&cancel),
            };
            job.started = Some(now);
            let Some(draft) = job.draft.take() else {
                self.end(job, Answer::AskAgain, now);
                continue;
            };
            return Some(Run {
                id,
                ticket: job.ticket,
                origin: job.origin.clone(),
                draft,
                budgets,
                generation,
                interpreter,
                cancel,
            });
        }
        None
    }

    async fn run(&self, run: Run) {
        let work = attempt(&run);
        let deadline = tokio::time::sleep(run.budgets.run);
        tokio::pin!(deadline);
        let result = tokio::select! {
            result = work => Some(result),
            () = &mut deadline => None,
            () = run.cancel.notified() => {
                // A superseded or stale job's request is dropped mid-way too: the server may
                // still be evaluating it.
                self.presume_busy(&run);
                return self.neutral(&run);
            }
        };
        // A transport error may be the child's death closing the connection before its exit
        // is published: it settles nothing until the generation is seen ending, or the grace
        // passes with the generation still alive (then it counts as a failure). The wait ends
        // at once on a cancel or supersession and never runs past the job's run deadline, so
        // the admission arithmetic (run deadlines) still bounds the worker.
        if matches!(result, Some(Err(Attempt::Transport))) {
            let grace = *self
                .grace
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            tokio::select! {
                () = self.generation_ends_within(run.generation, grace) => {}
                () = &mut deadline => {}
                () = run.cancel.notified() => {
                    self.presume_busy(&run);
                    return self.neutral(&run);
                }
            }
        }
        let now = Instant::now();
        let mut table = self.table();
        // A late answer from a job ended meanwhile is discarded; whoever ended it recorded a
        // neutral outcome, and one more is harmless, so a probe never stays in flight.
        let current = table.jobs.iter_mut().find(|job| {
            job.id == run.id
                && matches!(&job.state, State::Running { cancel } if Arc::ptr_eq(cancel, &run.cancel))
        });
        let Some(job) = current else {
            drop(table);
            // Ended meanwhile (superseded or stale as the deadline fired): its request was
            // still dropped mid-way, so the server is presumed busy as on any expiry, while
            // it is still this generation's (`presume_busy` checks).
            if result.is_none() {
                self.presume_busy(&run);
            }
            return self.neutral(&run);
        };
        match &result {
            None => {
                self.presume_busy(&run);
                self.signal(Signal::Expired {
                    generation: run.generation,
                });
            }
            Some(Ok(_)) => self.signal(Signal::Completed {
                generation: run.generation,
            }),
            Some(Err(_)) => {}
        }
        let (answer, outcome) = match result {
            None => (Answer::AskAgain, breaker::Outcome::Neutral),
            Some(Err(Attempt::TooLong)) => (Answer::TooLong, breaker::Outcome::Neutral),
            Some(Err(Attempt::Failed | Attempt::Transport)) => (
                Answer::Unavailable,
                breaker::Outcome::Failure {
                    generation: run.generation,
                },
            ),
            Some(Ok(answer)) => (answer, breaker::Outcome::Success),
        };
        close(job, answer, now);
        drop(table);
        self.breaker().record(run.ticket, outcome, now.into_std());
    }

    /// Whether `generation` is no longer the ready one.
    fn generation_over(&self, generation: u64) -> bool {
        !matches!(&*self.runtime(), Runtime::Ready { generation: g, .. } if *g == generation)
    }

    /// Waits until `generation` is seen ending (its jobs are then ended `Restarted`), at most
    /// `within`.
    async fn generation_ends_within(&self, generation: u64, within: Duration) {
        let deadline = Instant::now() + within;
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.generation_over(generation) {
                return;
            }
            tokio::select! {
                () = &mut changed => {}
                () = tokio::time::sleep_until(deadline) => return,
            }
        }
    }

    /// After a request is dropped (deadline, supersession, a newer note), the server is
    /// presumed busy for the calibrated prompt term, while it is still this generation's.
    fn presume_busy(&self, run: &Run) {
        let current = matches!(&*self.runtime(), Runtime::Ready { generation, .. } if *generation == run.generation);
        if current {
            *self
                .busy_until
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(Instant::now() + run.budgets.busy_after_expiry);
        }
    }

    fn neutral(&self, run: &Run) {
        self.breaker().record(
            run.ticket,
            breaker::Outcome::Neutral,
            Instant::now().into_std(),
        );
    }

    /// Moves job `id`'s finish back past the option lifetime (route tests).
    #[cfg(test)]
    pub(crate) fn expire_for_tests(&self, id: JobId) {
        if let Some(job) = self.table().jobs.iter_mut().find(|job| job.id == id)
            && let State::Done { at, .. } = &mut job.state
            && let Some(earlier) = at.checked_sub(OPTION_TTL)
        {
            *at = earlier;
        }
    }

    #[cfg(test)]
    pub(crate) fn records(&self) -> usize {
        self.table().jobs.len()
    }

    /// Sets the transport-error grace (tests: no test may depend on real time beating it).
    #[cfg(test)]
    pub(crate) fn set_transport_grace(&self, grace: Duration) {
        *self
            .grace
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = grace;
    }

    /// Sends a signal as the worker would (manager tests).
    #[cfg(test)]
    pub(crate) fn signal_for_tests(&self, signal: Signal) {
        self.signal(signal);
    }

    #[cfg(test)]
    pub(crate) fn breaker_closed(&self) -> bool {
        self.breaker().is_closed()
    }
}

impl Assistant {
    /// Every other end of a job: a running one is cancelled (its request dropped, which
    /// closes the connection), a queued one leaves the queue, finished options are dropped,
    /// and its ticket gets a neutral breaker outcome, so a probe ended here (superseded,
    /// stale, restarted, expired) leaves the breaker half-open for the next job instead of
    /// holding the probe flag. A neutral outcome for a ticket that is not the probe, or
    /// whose outcome is already recorded, changes nothing; a successor that took the probe
    /// over on supersession keeps it.
    fn end(&self, job: &mut Job, answer: Answer, now: Instant) {
        close(job, answer, now);
        self.breaker()
            .record(job.ticket, breaker::Outcome::Neutral, now.into_std());
    }
}

/// The breaker (503) then capacity (429) of a start, computed as if `superseded` were
/// already gone (plan §1.4, "Jobs"): at most D waiting, the worst-case wait (the running
/// job's remaining run deadline, or the rest of the busy time after a deadline cancel, plus
/// R per waiting job) within Q, fewer than 64 live jobs.
impl Assistant {
    fn admissible(
        &self,
        table: &Table,
        budgets: Budgets,
        superseded: Option<(JobId, breaker::Ticket)>,
        now: Instant,
    ) -> Result<(), Refusal> {
        let gone = superseded.map(|(id, _)| id);
        let waiting = table.queue.iter().filter(|id| Some(**id) != gone).count();
        let running_left = table
            .jobs
            .iter()
            .find(|job| matches!(job.state, State::Running { .. }) && Some(job.id) != gone)
            .map_or(Duration::ZERO, |job| job.started_left(budgets.run, now));
        let busy_left = self
            .busy_until
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .map_or(Duration::ZERO, |until| until.saturating_duration_since(now));
        let worst =
            running_left.max(busy_left) + budgets.run * u32::try_from(waiting).unwrap_or(u32::MAX);
        let live_records = table
            .jobs
            .iter()
            .filter(|job| job.live() && Some(job.id) != gone)
            .count();
        let breaker_refuses = {
            let breaker = self.breaker();
            breaker.refuses(now.into_std())
                && !superseded.is_some_and(|(_, ticket)| breaker.is_probe(ticket))
        };
        if breaker_refuses {
            return Err(Refusal::Unavailable);
        }
        if waiting >= budgets.depth || worst > budgets.queue_wait || live_records >= MAX_RECORDS {
            return Err(Refusal::Busy);
        }
        Ok(())
    }
}

/// Whether a note written at address `write` makes an origin at `target` non-current: the
/// same address, or a `row:` address of the row a `cell:` target is in. One check for
/// cancellation (`note_written`) and admission (`start`).
fn covers(write: &str, target: &str) -> bool {
    write == target
        || write.strip_prefix("row:").is_some_and(|row| {
            target.strip_prefix("cell:").is_some_and(|cell| {
                cell.strip_prefix(row)
                    .is_some_and(|rest| rest.starts_with('/'))
            })
        })
}

/// Closes a job without a breaker outcome: only for the worker's own end of a run, which
/// records the exchange's outcome itself.
fn close(job: &mut Job, answer: Answer, now: Instant) {
    if let State::Running { cancel } = &job.state {
        cancel.notify_one();
    }
    job.draft = None;
    job.state = State::Done { answer, at: now };
}

/// Drops note-write entries no start or live job can need: below the smallest origin
/// revision registered or live on the sheet; the whole sheet entry when nothing is.
fn prune(table: &mut Table, sheet: i64) {
    let live = table
        .jobs
        .iter()
        .filter(|job| job.origin.sheet == sheet && job.live())
        .map(|job| job.origin.revision)
        .min();
    let Some(writes) = table.writes.get_mut(&sheet) else {
        return;
    };
    let floor = [live, writes.registered.keys().next().copied()]
        .into_iter()
        .flatten()
        .min();
    match floor {
        Some(floor) => writes.latest.retain(|_, revision| *revision >= floor),
        None => {
            table.writes.remove(&sheet);
        }
    }
}

struct Run {
    id: JobId,
    ticket: breaker::Ticket,
    origin: Origin,
    draft: Draft,
    budgets: Budgets,
    generation: u64,
    interpreter: Arc<dyn Interpreter>,
    cancel: Arc<Notify>,
}

enum Attempt {
    TooLong,
    /// A failure that counts at once (an HTTP status, a tokenizer failure).
    Failed,
    /// No well-formed exchange (the connection closed, the body or stream broke): possibly
    /// the child's death, so it waits for the generation's end before it counts.
    Transport,
}

/// Renders, calls the model once and reads the reply.
async fn attempt(run: &Run) -> Result<Answer, Attempt> {
    let rendered = request::render(&run.draft, run.budgets.bounds, run.interpreter.as_ref())
        .await
        .map_err(|error| match error {
            NotRendered::TooLong => Attempt::TooLong,
            NotRendered::Tokenizer(_) => Attempt::Failed,
        })?;
    let completed = run
        .interpreter
        .complete(&Completion {
            dynamic: rendered.dynamic,
            ref_ids: rendered.ref_ids,
            max_tokens: run.budgets.max_tokens,
        })
        .await
        .map_err(|failure| match failure {
            Failure::Transport => Attempt::Transport,
            Failure::Status(_) => Attempt::Failed,
        })?;
    Ok(match output::read(&completed.text, &rendered.checks) {
        Err(_) => Answer::Nothing,
        Ok(Reading::Ambiguous(readings)) => Answer::Readings(readings),
        Ok(Reading::Options { shown, .. }) if shown.is_empty() => Answer::Nothing,
        Ok(Reading::Options { shown, .. }) => Answer::Options(
            shown
                .into_iter()
                .enumerate()
                .map(|(n, candidate)| proposal(run, n, candidate))
                .collect(),
        ),
    })
}

/// Binds an option: its content hash covers the job, the option's index, the origin, the
/// write (target, value before, value after) and every reference (address and value).
fn proposal(run: &Run, n: usize, candidate: Candidate) -> Proposal {
    let hash = content_hash(run.id, n, &run.origin, run.draft.target.value, &candidate);
    let before = run.draft.target.value;
    let labels: BTreeMap<&str, &str> = run
        .draft
        .items
        .iter()
        .map(|(item, _)| (item.id.as_str(), item.label.as_str()))
        .collect();
    let name = |alias: &str| {
        if alias == super::contract::TARGET_ALIAS {
            before.map_or_else(|| "?".to_owned(), |value| value.to_string())
        } else {
            // Mimik's own value beside the label (ASSIST-56): a number in the model's
            // explanation that disagrees with it is visibly contradicted.
            let label = labels.get(alias).copied().unwrap_or(alias);
            candidate
                .refs
                .get(alias)
                .and_then(|supplied| supplied.value)
                .map_or_else(|| label.to_owned(), |value| format!("{label} ({value})"))
        }
    };
    let shown = output::render(&candidate.expr, &name);
    Proposal {
        candidate,
        before,
        shown,
        hash,
    }
}

/// Whether `proposal`, option `n` of job `id` bound to `origin`, still has the content
/// hash it was stored with (the apply's check).
///
/// It guards the apply against a stored proposal that no longer matches what was shown:
/// another index, job or origin, a changed value, value before, expression, explanation or
/// reference. It does not guard against changes on the sheet (the apply's live rechecks of
/// the origin note, the target and every reference do), against a client (the apply route
/// takes only `csrf`, never proposal content), or against the model's judgement of
/// relevance.
pub(crate) fn verify(id: JobId, n: usize, origin: &Origin, proposal: &Proposal) -> bool {
    content_hash(id, n, origin, proposal.before, &proposal.candidate) == proposal.hash
}

fn content_hash(
    id: JobId,
    n: usize,
    origin: &Origin,
    before: Option<i64>,
    candidate: &Candidate,
) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"mimik-proposal-v1\0");
    let mut field = |bytes: &[u8]| {
        hash.update(u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
        hash.update(bytes);
    };
    field(&id.0);
    field(&u64::try_from(n).unwrap_or(u64::MAX).to_be_bytes());
    field(&origin.sheet.to_be_bytes());
    field(origin.target.as_bytes());
    field(&origin.revision.to_be_bytes());
    field(
        before
            .map_or_else(|| "unknown".to_owned(), |v| v.to_string())
            .as_bytes(),
    );
    field(candidate.value.to_string().as_bytes());
    for supplied in candidate.refs.values() {
        field(supplied.address.as_bytes());
        field(
            supplied
                .value
                .map_or_else(|| "unknown".to_owned(), |v| v.to_string())
                .as_bytes(),
        );
    }
    field(
        serde_json::to_string(&candidate.expr)
            .unwrap_or_default()
            .as_bytes(),
    );
    field(candidate.explanation.as_bytes());
    hash.finalize().into()
}

#[cfg(test)]
#[path = "jobs_tests.rs"]
mod tests;
