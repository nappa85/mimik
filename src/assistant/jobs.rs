//! Interpretation jobs (assisted sheet plan §1.4, "Jobs", "Origin binding", "Routes"): one
//! worker, admission within the queue budget, at most 64 records, supersession, the
//! note-write map, the runtime event interface and the breaker.
//!
//! Everything here is in memory and per process. The table's lock covers every
//! check-then-enqueue and every post-commit note-write hook; it is never held across a
//! database wait, the permit wait or the model call.
use super::breaker::{self, Admission, Breaker};
use super::interpreter::{Completion, Failure, Interpreter, RequestKind};
use super::output::{self, Candidate, Reading};
pub(crate) use super::proposal::{Change, Edit, Kind, Proposal, verify};
use super::proposal::{bind, bind_with_reads};
use super::reminder::{self, ReminderRequest};
use super::request::{self, Bounds, Draft, NotRendered};
use super::runtime::Path;
use super::runtime::reminder_budgets::ReminderBudgets;
use crate::campaigns::NoteValue;
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
        /// Which path the generation runs on (the reminder setting's default depends on it).
        path: Path,
        /// What the generation's measurements allow for reminders (plan 3c-1).
        reminders: Reminders,
    },
}

/// Whether reminders can run in a generation, from its measurements.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Reminders {
    /// Not possible, with the reason the page states.
    Unavailable(String),
    Available(ReminderBudgets),
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
        path: Path,
        reminders: Reminders,
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

    pub(super) fn bytes(self) -> [u8; 16] {
        self.0
    }
}

/// What a job is bound to (plan §1.4, "Origin binding"): the sheet, the typed address of
/// the origin note (a row cell by host ID), that note's revision, and the job's kind, which
/// decides what the apply checks of the origin note (plan §1.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Origin {
    pub sheet: i64,
    pub target: String,
    pub revision: i64,
    pub kind: Kind,
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
    /// A reminder's valid reply: the hint, the cited row's name and the option that passed
    /// every check (none, or one).
    Reminder {
        hint: String,
        row: Option<String>,
        options: Vec<Proposal>,
    },
}

impl Answer {
    /// The options a job offers, if any.
    fn options(&self) -> &[Proposal] {
        match self {
            Self::Options(options) | Self::Reminder { options, .. } => options,
            _ => &[],
        }
    }
}

enum State {
    Queued,
    Running { cancel: Arc<Notify> },
    Done { answer: Answer, at: Instant },
}

/// A reminder's request as the sheet built it: rendered by the worker within B and N(B).
#[derive(Clone, Debug)]
pub(crate) struct ReminderDraft {
    pub request: ReminderRequest,
}

/// Why a reminder start is refused (plan 3c-1, "Scheduling").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReminderRefusal {
    /// Off, unavailable for the generation or not ready: the module stays quiet (404).
    NotAvailable,
    /// The breaker is not closed (a reminder never takes the half-open probe): transient, so
    /// the module may try again on a later decrease (503).
    BreakerNotClosed,
    /// A newer note for the target exists (409).
    Conflict,
    /// 8 live reminders, or no finished record to evict (429).
    Busy,
}

/// Live reminders at most.
pub(crate) const MAX_LIVE_REMINDERS: usize = 8;
/// Reminder records at most; a start evicts the oldest finished one.
pub(crate) const MAX_REMINDER_RECORDS: usize = 32;
/// How long a reminder may wait for the worker.
pub(crate) const REMINDER_QUEUE_WAIT: Duration = Duration::from_secs(60);

struct Job {
    /// A reminder's request (`None` for an interpretation).
    reminder: Option<ReminderDraft>,
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
        matches!(&self.state, State::Done { answer, at }
            if !answer.options().is_empty() && !self.applied && now < *at + OPTION_TTL)
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
    /// Reminder records, outside the interpretations' one-live-job rule, repeat detection and
    /// capacity.
    reminders: Vec<Job>,
    reminder_queue: VecDeque<JobId>,
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

/// A pending re-prime of the interpretation prefix after a started reminder.
struct Reprime {
    generation: u64,
    interpreter: Arc<dyn Interpreter>,
    /// How long the server is busy with it (the time priming measured).
    wait: Duration,
}

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
    /// Until when the presumed busy time is a reminder's (its dropped prompt or its re-prime):
    /// the interpretation queued behind it does not count toward D.
    reminder_busy: Mutex<Option<Instant>>,
    /// Set after a reminder reached the server: the next interpretation's completion must show
    /// the prefix read back from the cache (plan 3c-0, "runtime detection"; 3c-1b, both paths).
    cache_check: std::sync::atomic::AtomicBool,
    /// GPU path: the cache check failed, so every started reminder is followed by a re-prime
    /// of the interpretation prefix (reminders stay available). On the CPU path a failed check
    /// sets [`Self::cache_off`] instead.
    cache_failed: std::sync::atomic::AtomicBool,
    /// Why reminders were switched off for the generation after the cache check failed.
    cache_off: Mutex<Option<String>>,
    signals: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Signal>>>,
    /// After the cache check failed on the GPU path, a started reminder leaves the
    /// interpretation prefix evicted: the worker re-primes it before the next job (plan 3c-0,
    /// choice (a), the fallback of 3c-1b). Not installed while the cache restores it.
    reprime: Mutex<Option<Reprime>>,
    /// The owner's reminder setting as stored (`None` until chosen), kept here so the save
    /// answer and the route read the effective value without a database read.
    reminder_choice: Mutex<Option<bool>>,
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
            reminder_busy: Mutex::new(None),
            cache_check: std::sync::atomic::AtomicBool::new(false),
            cache_failed: std::sync::atomic::AtomicBool::new(false),
            cache_off: Mutex::new(None),
            reprime: Mutex::new(None),
            signals: Mutex::new(None),
            reminder_choice: Mutex::new(None),
            #[cfg(test)]
            resets: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    fn table(&self) -> MutexGuard<'_, Table> {
        self.table
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn cache_off(&self) -> MutexGuard<'_, Option<String>> {
        self.cache_off
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
                self.forget_reminder_state();
            }
            RuntimeEvent::Ready {
                generation,
                budgets,
                interpreter,
                path,
                reminders,
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
                        path,
                        reminders,
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
                *self
                    .reprime
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                self.forget_reminder_state();
                let mut table = self.table();
                table.queue.clear();
                for job in table.jobs.iter_mut().filter(|job| job.live()) {
                    self.end(job, Answer::Restarted, now);
                }
                table.reminder_queue.clear();
                for job in table.reminders.iter_mut().filter(|job| job.live()) {
                    close(job, Answer::Restarted, now);
                }
            }
        }
        self.changed.notify_waiters();
    }

    /// What a generation's reminders leave behind ends with it: the reminder busy time, the
    /// pending cache check and its verdict.
    fn forget_reminder_state(&self) {
        *self
            .reminder_busy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.cache_check
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.cache_failed
            .store(false, std::sync::atomic::Ordering::SeqCst);
        *self.cache_off() = None;
    }

    /// Sets the owner's stored reminder choice (`None`: not chosen). Called at start from the
    /// database and by the assistant page on a change; no restart follows.
    pub(crate) fn set_reminder_choice(&self, choice: Option<bool>) {
        *self
            .reminder_choice
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = choice;
        self.changed.notify_waiters();
    }

    /// The stored choice.
    pub(crate) fn reminder_choice(&self) -> Option<bool> {
        *self
            .reminder_choice
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The reminder budgets while the runtime is ready and the generation allows reminders,
    /// whether or not the owner turned them on.
    #[cfg(test)]
    pub(crate) fn reminder_budgets(&self) -> Option<ReminderBudgets> {
        match &*self.runtime() {
            Runtime::Ready {
                reminders: Reminders::Available(budgets),
                ..
            } => Some(*budgets),
            _ => None,
        }
    }

    /// Whether reminders run now (ASSIST-60): the runtime is ready, the generation allows
    /// them (3c-1, "Budgets") and the setting is on; unset means on for the GPU path and off
    /// for the CPU path. Not ready, unavailable or off: no reminder key anywhere.
    pub(crate) fn reminders_effective(&self) -> bool {
        let Runtime::Ready {
            path, reminders, ..
        } = &*self.runtime()
        else {
            return false;
        };
        matches!(reminders, Reminders::Available(_))
            && self.cache_off().is_none()
            && self.reminder_choice().unwrap_or(*path == Path::Gpu)
    }

    /// What the page states of the generation's reminders: its path and what they allow.
    pub(crate) fn reminder_generation(&self) -> Option<(Path, Reminders)> {
        match &*self.runtime() {
            Runtime::Ready {
                path, reminders, ..
            } => Some((
                *path,
                match self.cache_off().as_ref() {
                    Some(why) => Reminders::Unavailable(why.clone()),
                    None => reminders.clone(),
                },
            )),
            _ => None,
        }
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
        // An admitted interpretation cancels a running reminder (plan 3c-1, "Scheduling"): the
        // worker's busy term for it was counted by `admissible`.
        let cancelled = table
            .reminders
            .iter_mut()
            .filter(|job| matches!(job.state, State::Running { .. }))
            .map(|job| close(job, Answer::Superseded, now))
            .count();
        if cancelled > 0 {
            // Set here, under the table's lock, so the next admission counts it before the
            // worker has seen the cancel.
            let until = now + self.reminder_busy_term();
            self.presume_busy_until(until, true);
        }
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
            reminder: None,
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

    /// Starts a reminder after the database check found the decrease note current (plan 3c-1,
    /// "Scheduling"): outside the one-live-job rule and the interpretation records. Refused
    /// unless reminders run now and the breaker is closed; a newer note for the target
    /// conflicts; the account's older live reminder is superseded; at most 8 live reminders
    /// and 32 records.
    pub(crate) fn start_reminder(
        &self,
        registration: &Registration,
        account: i64,
        origin: Origin,
        draft: ReminderDraft,
    ) -> Result<JobId, ReminderRefusal> {
        debug_assert_eq!(registration.sheet, origin.sheet);
        debug_assert_eq!(origin.kind, Kind::Reminder);
        if !self.reminders_effective() {
            return Err(ReminderRefusal::NotAvailable);
        }
        if !self.breaker().is_closed() {
            return Err(ReminderRefusal::BreakerNotClosed);
        }
        let now = Instant::now();
        let mut table = self.table();
        // Again under the table's lock: a generation that ended meanwhile has already ended
        // the reminders (the event takes this lock after setting the state), so none is queued
        // after it.
        if !self.reminders_effective() {
            return Err(ReminderRefusal::NotAvailable);
        }
        let newer_write = table.writes.get(&origin.sheet).is_some_and(|writes| {
            writes
                .latest
                .iter()
                .any(|(target, latest)| *latest > origin.revision && covers(target, &origin.target))
        });
        if newer_write {
            return Err(ReminderRefusal::Conflict);
        }
        let superseded = table
            .reminders
            .iter()
            .find(|job| job.account == account && job.live())
            .map(|job| job.id);
        let live = table
            .reminders
            .iter()
            .filter(|job| job.live() && Some(job.id) != superseded)
            .count();
        if live >= MAX_LIVE_REMINDERS {
            return Err(ReminderRefusal::Busy);
        }
        if let Some(id) = superseded {
            table.reminder_queue.retain(|queued| *queued != id);
            if let Some(job) = table.reminders.iter_mut().find(|job| job.id == id) {
                close(job, Answer::Superseded, now);
            }
        }
        if table.reminders.len() >= MAX_REMINDER_RECORDS {
            let oldest = table
                .reminders
                .iter()
                .enumerate()
                .filter(|(_, job)| !job.live())
                .min_by_key(|(_, job)| match job.state {
                    State::Done { at, .. } => at,
                    _ => now,
                })
                .map(|(index, _)| index);
            let Some(index) = oldest else {
                return Err(ReminderRefusal::Busy);
            };
            table.reminders.remove(index);
        }
        let id = JobId::random();
        // Reminders hold no breaker ticket of their own: a reminder never takes the probe and
        // its expiries are neutral; failures are recorded with this number.
        let ticket = table.next_ticket;
        table.next_ticket += 1;
        table.reminders.push(Job {
            reminder: Some(draft),
            id,
            ticket,
            account,
            origin,
            enqueued: now,
            started: None,
            state: State::Queued,
            draft: None,
            applied: false,
        });
        table.reminder_queue.push_back(id);
        drop(table);
        self.wake.notify_one();
        Ok(id)
    }

    /// Reminder records held (tests).
    #[cfg(test)]
    pub(crate) fn reminder_records(&self) -> usize {
        self.table().reminders.len()
    }

    /// A reminder's state as a poll sees it, or `None` when it is not one (tests).
    #[cfg(test)]
    pub(crate) fn poll_reminder_for_tests(
        &self,
        account: i64,
        sheet: i64,
        id: JobId,
    ) -> Option<Poll> {
        let known = self
            .table()
            .reminders
            .iter()
            .any(|job| job.id == id && job.account == account && job.origin.sheet == sheet);
        known.then(|| self.poll(account, sheet, id))
    }

    /// Records a counted failure as a completed exchange's opposite would (tests).
    #[cfg(test)]
    pub(crate) fn breaker_failure_for_tests(&self, ticket: breaker::Ticket) {
        self.breaker().record(
            ticket,
            breaker::Outcome::Failure { generation: 1 },
            Instant::now().into_std(),
        );
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
        // Reminders end silently: the module shows nothing for a stale one.
        let mut stale_reminders = Vec::new();
        for job in &mut table.reminders {
            if stale_origin(&job.origin) && (job.live() || job.offering(now)) {
                stale_reminders.push(job.id);
                close(job, Answer::Stale, now);
            }
        }
        table
            .reminder_queue
            .retain(|id| !stale_reminders.contains(id));
        prune(&mut table, sheet);
    }

    /// What a poll by `account` on `sheet` sees of job `id`.
    pub(crate) fn poll(&self, account: i64, sheet: i64, id: JobId) -> Poll {
        let now = Instant::now();
        let table = self.table();
        let Some(job) = table
            .jobs
            .iter()
            .chain(&table.reminders)
            .find(|job| job.id == id && job.account == account && job.origin.sheet == sheet)
        else {
            return Poll::Missing;
        };
        match &job.state {
            State::Queued => Poll::Queued,
            State::Running { .. } => Poll::Running,
            State::Done { answer, .. } if !answer.options().is_empty() && !job.offering(now) => {
                Poll::Done(Answer::AskAgain)
            }
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
            .chain(&table.reminders)
            .find(|job| job.id == id && job.account == account && job.origin.sheet == sheet)?;
        if !job.offering(now) {
            return None;
        }
        let State::Done { answer, .. } = &job.state else {
            return None;
        };
        answer
            .options()
            .get(n)
            .map(|option| (job.origin.clone(), option.clone()))
    }

    /// Test producer: a finished job of `account` on `origin` offering one option per entry of
    /// `options` (its changes and explanation), standing in for the reminder producer of step
    /// 3c. Options that fail [`bind`] are left out, as a reply's would be.
    #[cfg(test)]
    pub(crate) fn offer(
        &self,
        account: i64,
        origin: Origin,
        options: Vec<(Vec<Change>, String)>,
    ) -> JobId {
        let now = Instant::now();
        let id = JobId::random();
        let mut kept: Vec<Proposal> = Vec::new();
        for (changes, explanation) in options {
            if let Some(option) = bind(id, kept.len(), &origin, changes, explanation) {
                kept.push(option);
            }
        }
        let answer = if kept.is_empty() {
            Answer::Nothing
        } else {
            Answer::Options(kept)
        };
        let mut table = self.table();
        let ticket = table.next_ticket;
        table.next_ticket += 1;
        table.jobs.push(Job {
            reminder: None,
            id,
            ticket,
            account,
            origin,
            enqueued: now,
            started: Some(now),
            state: State::Done { answer, at: now },
            draft: None,
            applied: false,
        });
        id
    }

    /// Marks job `id` applied, after the apply's commit: options are alternatives.
    pub(crate) fn consume(&self, id: JobId) {
        let mut table = self.table();
        let table = &mut *table;
        if let Some(job) = table
            .jobs
            .iter_mut()
            .chain(table.reminders.iter_mut())
            .find(|job| job.id == id)
        {
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
            if self.reprime().await {
                continue;
            }
            match self.next() {
                Some(run) => {
                    // Installed as the reminder starts, so admission counts it while the
                    // reminder runs.
                    let sent = Arc::clone(&run.sent);
                    let reminder = run.work.is_reminder();
                    let generation = run.generation;
                    let cache_failed = self.cache_failed.load(std::sync::atomic::Ordering::SeqCst);
                    // While the cache restores the prefix there is nothing to re-prime.
                    if let Some(after) = run.reprime().filter(|_| cache_failed) {
                        *self
                            .reprime
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(after);
                    }
                    self.run(run).await;
                    if reminder && !sent.load(std::sync::atomic::Ordering::SeqCst) {
                        // The reminder never reached the server (too long, the tokenizer
                        // failed, cancelled first): the slot's cache is untouched.
                        let mut pending = self
                            .reprime
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if pending.as_ref().is_some_and(|p| p.generation == generation) {
                            *pending = None;
                        }
                    } else if reminder && !cache_failed {
                        self.cache_check
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                }
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
                ..
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
                work: Work::Interpretation(draft),
                deadline: budgets.run,
                busy: budgets.busy_after_expiry,
                budgets,
                generation,
                interpreter,
                cancel,
                sent: Arc::default(),
                cached: Arc::default(),
            });
        }
        // A reminder only when no interpretation is queued (the loop above emptied the queue).
        let (path, reminders) = match &*self.runtime() {
            Runtime::Ready {
                path, reminders, ..
            } => (*path, reminders.clone()),
            _ => return None,
        };
        while let Some(id) = table.reminder_queue.pop_front() {
            let Some(job) = table.reminders.iter_mut().find(|job| job.id == id) else {
                continue;
            };
            let Reminders::Available(allowed) = &reminders else {
                close(job, Answer::Unavailable, now);
                continue;
            };
            if let Some(answer) = self.reminder_not_run(job.enqueued, now) {
                close(job, answer, now);
                continue;
            }
            let Some(draft) = job.reminder.take() else {
                close(job, Answer::AskAgain, now);
                continue;
            };
            let cancel = Arc::new(Notify::new());
            job.state = State::Running {
                cancel: Arc::clone(&cancel),
            };
            job.started = Some(now);
            return Some(Run {
                id,
                ticket: job.ticket,
                origin: job.origin.clone(),
                work: Work::Reminder {
                    request: draft.request,
                    reply_tokens: allowed.reply_tokens,
                    gpu: path == Path::Gpu,
                    reprime: allowed.reprime,
                },
                deadline: allowed.run,
                busy: allowed.busy,
                budgets,
                generation,
                interpreter,
                cancel,
                sent: Arc::default(),
                cached: Arc::default(),
            });
        }
        None
    }

    /// Why a queued reminder does not run now: switched off meanwhile (the cache check, the
    /// owner's setting), waited too long, or the breaker is not closed (a reminder never takes
    /// the half-open probe).
    fn reminder_not_run(&self, enqueued: Instant, now: Instant) -> Option<Answer> {
        if !self.reminders_effective() || !self.breaker().is_closed() {
            Some(Answer::Unavailable)
        } else if now > enqueued + REMINDER_QUEUE_WAIT {
            Some(Answer::AskAgain)
        } else {
            None
        }
    }

    /// Re-primes the interpretation prefix after a started reminder on the GPU path, before any
    /// next job; `busy_until` covers it and ends when the request completes. Returns whether
    /// one ran.
    async fn reprime(&self) -> bool {
        let Some(pending) = self
            .reprime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        else {
            return false;
        };
        if self.generation_over(pending.generation) {
            return false;
        }
        let bound = pending.wait * 2 + Duration::from_secs(10);
        // Presumed busy for the restore time the Ready rules count; the worker awaits the request
        // (within its bound) before it starts any job, so a slower one delays the next job, not
        // the admission arithmetic.
        self.presume_busy_until(Instant::now() + pending.wait, true);
        // A failure or a timeout changes nothing: the next interpretation evaluates its own
        // prefix, which its run deadline's margin covers.
        let _ = tokio::time::timeout(bound, pending.interpreter.reprime()).await;
        if !self.generation_over(pending.generation) {
            *self
                .busy_until
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            *self
                .reminder_busy
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
        true
    }

    async fn run(&self, run: Run) {
        let work = attempt(&run);
        let deadline = tokio::time::sleep(run.deadline);
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
        let mut guard = self.table();
        // A late answer from a job ended meanwhile is discarded; whoever ended it recorded a
        // neutral outcome, and one more is harmless, so a probe never stays in flight.
        let table = &mut *guard;
        let current = table.jobs.iter_mut().chain(table.reminders.iter_mut()).find(|job| {
            job.id == run.id
                && matches!(&job.state, State::Running { cancel } if Arc::ptr_eq(cancel, &run.cancel))
        });
        let Some(job) = current else {
            drop(guard);
            // Ended meanwhile (superseded or stale as the deadline fired): its request was
            // still dropped mid-way, so the server is presumed busy as on any expiry, while
            // it is still this generation's (`presume_busy` checks).
            if result.is_none() {
                self.presume_busy(&run);
            }
            return self.neutral(&run);
        };
        if !run.work.is_reminder() {
            self.check_restored_prefix(&run, result.as_ref());
        }
        match &result {
            None => {
                self.presume_busy(&run);
                // A reminder's expiry never feeds the hung detector.
                if !run.work.is_reminder() {
                    self.signal(Signal::Expired {
                        generation: run.generation,
                    });
                }
            }
            Some(Ok(_) | Err(Attempt::Invalid)) => self.signal(Signal::Completed {
                generation: run.generation,
            }),
            Some(Err(_)) => {}
        }
        let (answer, outcome) = match result {
            None => (Answer::AskAgain, breaker::Outcome::Neutral),
            Some(Err(Attempt::TooLong)) => (Answer::TooLong, breaker::Outcome::Neutral),
            // An invalid reminder reply is `Nothing` and counts for nothing.
            Some(Err(Attempt::Invalid)) => (Answer::Nothing, breaker::Outcome::Neutral),
            Some(Err(Attempt::Failed | Attempt::Transport)) => (
                Answer::Unavailable,
                breaker::Outcome::Failure {
                    generation: run.generation,
                },
            ),
            Some(Ok(answer)) => (answer, breaker::Outcome::Success),
        };
        close(job, answer, now);
        drop(guard);
        self.breaker().record(run.ticket, outcome, now.into_std());
    }

    /// After a reminder, the first interpretation that reached the server must have read at
    /// least 75 % of the prefix from the prompt cache (the calibration rule); otherwise the
    /// host-RAM cache does not restore it: on the CPU path reminders are unavailable for the rest
    /// of the generation, with the reason; on the GPU path they stay available and every later
    /// reminder is followed by a re-prime. An expiry or a transport or HTTP failure of that request
    /// counts as a failed check, and so does a completion without the figure (unverified). A
    /// request that never reached the server (too long, tokenizer) settles nothing.
    fn check_restored_prefix(&self, run: &Run, result: Option<&Result<Answer, Attempt>>) {
        if !self.cache_check.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let sent = run.sent.load(std::sync::atomic::Ordering::SeqCst);
        let cached = *run
            .cached
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (path, prefix, reprime) = match &*self.runtime() {
            Runtime::Ready {
                generation,
                path,
                reminders: Reminders::Available(budgets),
                ..
            } if *generation == run.generation => (*path, budgets.prefix_tokens, budgets.reprime),
            _ => return,
        };
        let failed = match result {
            Some(Ok(_)) => match cached {
                None => Some(
                    "the server reported no cached-token figure, so the restore is unverified"
                        .to_owned(),
                ),
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "token counts of a few thousand, exact in f64"
                )]
                Some(cached) if (cached as f64) < 0.75 * prefix as f64 => Some(format!(
                    "{cached} of {prefix} prefix tokens were read from the cache"
                )),
                Some(_) => None,
            },
            None if sent => Some("the interpretation expired".to_owned()),
            Some(Err(Attempt::Failed | Attempt::Transport)) if sent => {
                Some("the interpretation request failed".to_owned())
            }
            _ => return,
        };
        self.cache_check
            .store(false, std::sync::atomic::Ordering::SeqCst);
        if let Some(why) = failed {
            match path {
                Path::Gpu => {
                    self.cache_failed
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    // Without a completed interpretation (no figure, an expiry, a failure) the
                    // slot may be cold: re-prime once now, so the next one is not.
                    if !matches!(result, Some(Ok(_))) || cached.is_none() {
                        *self
                            .reprime
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Reprime {
                            generation: run.generation,
                            interpreter: Arc::clone(&run.interpreter),
                            wait: reprime,
                        });
                        self.wake.notify_one();
                    }
                }
                Path::Cpu => {
                    *self.cache_off() = Some(format!(
                        "reminders are off for this generation: the prompt cache did not restore the interpretation prefix after a reminder ({why}); more threads or the GPU path could make them available"
                    ));
                    self.changed.notify_waiters();
                }
            }
        }
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
            self.presume_busy_until(Instant::now() + run.busy, run.work.is_reminder());
        }
    }

    /// Presumes the server busy until `until` (never shortening what is presumed), noting
    /// whether it is a reminder's.
    fn presume_busy_until(&self, until: Instant, reminder: bool) {
        let mut busy = self
            .busy_until
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *busy = Some(busy.map_or(until, |earlier| earlier.max(until)));
        if reminder {
            *self
                .reminder_busy
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = *busy;
        }
    }

    /// The presumed busy time left (tests).
    #[cfg(test)]
    pub(crate) fn busy_for_tests(&self) -> Duration {
        self.busy_until
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .map_or(Duration::ZERO, |until| {
                until.saturating_duration_since(Instant::now())
            })
    }

    /// Makes the GPU path fall back to re-priming, as a failed cache check does (tests).
    #[cfg(test)]
    pub(crate) fn fail_cache_for_tests(&self) {
        self.cache_failed
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Whether a re-prime is pending (tests).
    #[cfg(test)]
    pub(crate) fn reprime_pending_for_tests(&self) -> bool {
        self.reprime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
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
        let mut table = self.table();
        let table = &mut *table;
        if let Some(job) = table
            .jobs
            .iter_mut()
            .chain(table.reminders.iter_mut())
            .find(|job| job.id == id)
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

impl Assistant {
    /// How long the server stays busy after a reminder is dropped: its whole prompt. The
    /// GPU path's re-priming that follows is the pending [`Reprime`], counted on its own.
    fn reminder_busy_term(&self) -> Duration {
        match &*self.runtime() {
            Runtime::Ready {
                reminders: Reminders::Available(budgets),
                ..
            } => budgets.busy,
            _ => Duration::ZERO,
        }
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
        // A running reminder is cancelled by the admission, but the server may still be
        // evaluating its whole prompt (a cancel can come before the first token); on the GPU
        // path the re-priming follows. A re-priming still pending counts the same.
        let reminder_busy = if table
            .reminders
            .iter()
            .any(|job| matches!(job.state, State::Running { .. }))
        {
            self.reminder_busy_term()
        } else {
            Duration::ZERO
        };
        let pending_reprime = self
            .reprime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map_or(Duration::ZERO, |pending| pending.wait);
        let busy_left = busy_left.max(reminder_busy) + pending_reprime;
        // The first job queued behind a reminder's busy time waits for that time, not for a
        // running job, so it takes no place among the D that may wait: a scribble reading is
        // never refused because of a reminder, as it is not because of a running reading.
        let behind_reminder = waiting > 0
            && running_left.is_zero()
            && self
                .reminder_busy
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_some_and(|until| until > now);
        let counted = waiting - usize::from(behind_reminder);
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
        if counted >= budgets.depth || worst > budgets.queue_wait || live_records >= MAX_RECORDS {
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
        .chain(&table.reminders)
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

/// What a run sends.
enum Work {
    Interpretation(Draft),
    Reminder {
        request: ReminderRequest,
        reply_tokens: u32,
        /// The GPU path re-primes the interpretation prefix after the reminder.
        gpu: bool,
        reprime: Duration,
    },
}

impl Work {
    fn is_reminder(&self) -> bool {
        matches!(self, Self::Reminder { .. })
    }
}

struct Run {
    id: JobId,
    ticket: breaker::Ticket,
    origin: Origin,
    work: Work,
    /// The run deadline: R for an interpretation, the reminder's own for a reminder.
    deadline: Duration,
    /// How long the server is presumed busy after the request is dropped.
    busy: Duration,
    budgets: Budgets,
    generation: u64,
    interpreter: Arc<dyn Interpreter>,
    cancel: Arc<Notify>,
    /// Set once a reminder's request is about to reach the server (its cache is then touched).
    sent: Arc<std::sync::atomic::AtomicBool>,
    /// The prompt tokens the server read from its cache for this run's completion, once known.
    cached: Arc<Mutex<Option<u64>>>,
}

impl Run {
    /// The re-prime a started reminder leaves to do (GPU path), whatever way it ends.
    fn reprime(&self) -> Option<Reprime> {
        match self.work {
            Work::Reminder {
                gpu: true, reprime, ..
            } => Some(Reprime {
                generation: self.generation,
                interpreter: Arc::clone(&self.interpreter),
                wait: reprime,
            }),
            _ => None,
        }
    }
}

enum Attempt {
    /// A reminder's reply is not valid: shown as nothing, counted for nothing.
    Invalid,
    TooLong,
    /// A failure that counts at once (an HTTP status, a tokenizer failure).
    Failed,
    /// No well-formed exchange (the connection closed, the body or stream broke): possibly
    /// the child's death, so it waits for the generation's end before it counts.
    Transport,
}

/// Renders, calls the model once and reads the reply.
async fn attempt(run: &Run) -> Result<Answer, Attempt> {
    let draft = match &run.work {
        Work::Interpretation(draft) => draft,
        Work::Reminder {
            request,
            reply_tokens,
            ..
        } => return attempt_reminder(run, request, *reply_tokens).await,
    };
    let rendered = request::render(draft, run.budgets.bounds, run.interpreter.as_ref())
        .await
        .map_err(|error| match error {
            NotRendered::TooLong => Attempt::TooLong,
            NotRendered::Tokenizer(_) => Attempt::Failed,
        })?;
    run.sent.store(true, std::sync::atomic::Ordering::SeqCst);
    let completed = run
        .interpreter
        .complete(&Completion {
            dynamic: rendered.dynamic,
            ref_ids: rendered.ref_ids,
            max_tokens: run.budgets.max_tokens,
            kind: RequestKind::Interpretation,
        })
        .await
        .map_err(|failure| match failure {
            Failure::Transport => Attempt::Transport,
            Failure::Status(_) => Attempt::Failed,
        })?;
    *run.cached
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = completed.cached_tokens;
    Ok(match output::read(&completed.text, &rendered.checks) {
        Err(_) => Answer::Nothing,
        Ok(Reading::Ambiguous(readings)) => Answer::Readings(readings),
        Ok(Reading::Options { shown, .. }) => {
            // An option's index is its position among those kept, which the apply names.
            let mut options: Vec<Proposal> = Vec::new();
            for candidate in shown {
                if let Some(option) = proposal(run, draft, options.len(), candidate) {
                    options.push(option);
                }
            }
            if options.is_empty() {
                Answer::Nothing
            } else {
                Answer::Options(options)
            }
        }
    })
}

/// Binds an interpretation option: one integer change on the origin's target, its content
/// hash covering the job, the option's index, the origin, the write (target, value before,
/// value after) and every reference (address and value). `None` drops an option equal to the
/// current value.
fn proposal(run: &Run, draft: &Draft, n: usize, candidate: Candidate) -> Option<Proposal> {
    let before = draft.target.value;
    let labels: BTreeMap<&str, &str> = draft
        .items
        .iter()
        .map(|(item, _)| (item.id.as_str(), item.label.as_str()))
        .collect();
    let target_label = draft.target.label.clone().unwrap_or_default();
    let name = |alias: &str| {
        if alias == super::contract::TARGET_ALIAS {
            // The target as the other references: its label and Mimik's value.
            let value = before.map_or_else(|| "?".to_owned(), |value| value.to_string());
            if target_label.is_empty() {
                value
            } else {
                format!("{target_label} ({value})")
            }
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
    let explanation = candidate.explanation.clone();
    let change = Change {
        address: run.origin.target.clone(),
        before: before.map_or(NoteValue::Unknown, NoteValue::Int),
        edit: Edit::Expr { candidate, shown },
    };
    bind(run.id, n, &run.origin, vec![change], explanation)
}

/// Renders a reminder within B and N(B), calls the model once and reads the reply (plan 3c-1).
async fn attempt_reminder(
    run: &Run,
    request: &ReminderRequest,
    reply_tokens: u32,
) -> Result<Answer, Attempt> {
    let rendered = reminder::render(
        request,
        run.budgets.bounds,
        reply_tokens,
        run.interpreter.as_ref(),
    )
    .await
    .map_err(|error| match error {
        NotRendered::TooLong => Attempt::TooLong,
        NotRendered::Tokenizer(_) => Attempt::Failed,
    })?;
    run.sent.store(true, std::sync::atomic::Ordering::SeqCst);
    let completed = run
        .interpreter
        .complete(&rendered.completion)
        .await
        .map_err(|failure| match failure {
            Failure::Transport => Attempt::Transport,
            Failure::Status(_) => Attempt::Failed,
        })?;
    Ok(match reminder::read(&completed.text, &rendered.kept) {
        Err(_) => return Err(Attempt::Invalid),
        Ok(reminder::Reading::Nothing) => Answer::Nothing,
        Ok(reminder::Reading::Hint { hint, row, option }) => Answer::Reminder {
            hint,
            row,
            options: option
                .and_then(|(changes, explanation)| {
                    bind_with_reads(
                        run.id,
                        0,
                        &run.origin,
                        changes,
                        rendered.reads.clone(),
                        explanation,
                    )
                })
                .into_iter()
                .collect(),
        },
    })
}

#[cfg(test)]
#[path = "jobs_tests.rs"]
mod tests;
