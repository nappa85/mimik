//! Interpretation jobs (assisted sheet plan §1.4, "Jobs", "Origin binding", "Routes"): one
//! worker, admission within the queue budget, at most 64 records, supersession, the
//! note-write map, the runtime event interface and the breaker.
//!
//! Everything here is in memory and per process. The table's lock covers every
//! check-then-enqueue and every post-commit note-write hook; it is never held across a
//! database wait, the permit wait or the model call.
use super::breaker::{self, Admission, Breaker};
use super::interpreter::{Completion, Interpreter};
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
}

/// Why a generation ended. The development endpoint has one fixed generation; the 2c-2
/// runtime manager sends these.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    all(feature = "dev-endpoint", not(test)),
    expect(dead_code, reason = "sent by the 2c-2 runtime manager")
)]
pub(crate) enum EndReason {
    /// The child exited, or was stopped for a new generation.
    Exited,
    /// The owner's Restart.
    Restart,
    /// The runtime cannot run (the reason is shown).
    Unavailable(String),
}

/// The runtime event interface the worker consumes (plan §2 2c-1): fed in 2c-1 by the
/// development endpoint's fixed configuration and by test doubles, in 2c-2 by the runtime
/// manager.
pub(crate) enum RuntimeEvent {
    GenerationStarted {
        generation: u64,
    },
    #[cfg_attr(
        all(feature = "dev-endpoint", not(test)),
        expect(dead_code, reason = "sent by the 2c-2 runtime manager")
    )]
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

/// The assistant: the job table, the worker's wake-up, the runtime state and the breaker.
pub(crate) struct Assistant {
    table: Mutex<Table>,
    runtime: Mutex<Runtime>,
    breaker: Mutex<Breaker>,
    wake: Notify,
}

impl Assistant {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            table: Mutex::new(Table::default()),
            runtime: Mutex::new(Runtime::Starting { generation: None }),
            breaker: Mutex::new(Breaker::new()),
            wake: Notify::new(),
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
                let mut table = self.table();
                table.queue.clear();
                for job in table.jobs.iter_mut().filter(|job| job.live()) {
                    self.end(job, Answer::Restarted, now);
                }
            }
        }
    }

    /// The owner's Restart or the app's start: the breaker is reset.
    pub(crate) fn reset_breaker(&self) {
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
            match oldest {
                Some(index) => {
                    table.jobs.remove(index);
                }
                None => return Err(Refusal::Busy),
            }
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
            match self.next() {
                Some(run) => self.run(run).await,
                None => self.wake.notified().await,
            }
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
        let result = tokio::select! {
            result = work => Some(result),
            () = deadline => None,
            () = run.cancel.notified() => return self.neutral(&run),
        };
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
            return self.neutral(&run);
        };
        let (answer, outcome) = match result {
            None => (Answer::AskAgain, breaker::Outcome::Neutral),
            Some(Err(Attempt::TooLong)) => (Answer::TooLong, breaker::Outcome::Neutral),
            Some(Err(Attempt::Failed)) => (
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
/// job's remaining run deadline plus R per waiting job) within Q, fewer than 64 live jobs.
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
        let worst = running_left + budgets.run * u32::try_from(waiting).unwrap_or(u32::MAX);
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
    Failed,
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
            max_tokens: run.budgets.max_tokens,
        })
        .await
        .map_err(|_| Attempt::Failed)?;
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
        .map(|(item, _)| (item.alias.as_str(), item.label.as_str()))
        .collect();
    let name = |alias: &str| {
        if alias == super::contract::TARGET_ALIAS {
            before.map_or_else(|| "?".to_owned(), |value| value.to_string())
        } else {
            labels
                .get(alias)
                .map_or_else(|| alias.to_owned(), |label| (*label).to_owned())
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
