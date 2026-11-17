//! What the assistant page follows while the runtime works (`docs/model-runtime.md`, "Live
//! progress"): one [`Phase`] enum names every step of a start, a retry and a trial, so the
//! label, the step number and the page agree. The manager picks the step list ([`Kind`]) at
//! each entry point and moves through it with [`Reporter`]; the launcher reports the
//! model file's verification through [`StartEvent`].
//!
//! Times are text, never the bar's basis: a step shows at most the time its enforced limit
//! leaves (a deadline the code already holds), plus, when the last successful run of the
//! same file on the same path and entry kind measured the step, about what that left.
use super::Path;
use super::budgets::SAMPLE_TIMEOUT_S;
use super::manager::{MAX_FAILURES, PRIMING_WITHIN, RETRY_AFTER, State, Status};
use super::models::TrialState;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

/// One step of the runtime's work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Phase {
    /// The model server that ran is being stopped.
    Stopping,
    Checking,
    StartingServer,
    Verifying,
    Loading,
    Priming,
    /// Calibration sample 1 or 2.
    Sample(u8),
    WaitingRetry,
    /// A trial waits for the running model server to stop.
    WaitingStop,
    /// A trial hashes the candidate's file.
    Hashing,
    /// Trial phrase 1 to 5.
    Phrase(u8),
}

impl Phase {
    /// What the page says of the step.
    pub(crate) fn label(self) -> String {
        match self {
            Self::Stopping => "stopping the model server".to_owned(),
            Self::Checking => "checking the host".to_owned(),
            Self::StartingServer => "starting the model server".to_owned(),
            Self::Verifying => "verifying the model file".to_owned(),
            Self::Loading => "loading the model".to_owned(),
            Self::Priming => "priming".to_owned(),
            Self::Sample(n) => format!("calibration sample {n} of 2"),
            Self::WaitingRetry => "waiting to retry".to_owned(),
            Self::WaitingStop => "waiting for the model server to stop".to_owned(),
            Self::Hashing => "hashing the model file".to_owned(),
            Self::Phrase(n) => format!("trial phrase {n} of 5"),
        }
    }

    /// The limit the code enforces on the step, when it has one, and whether it is exact (a
    /// timer that fires at that moment) rather than a bound.
    fn limit(self) -> Option<(Duration, bool)> {
        match self {
            Self::Priming => Some((PRIMING_WITHIN, false)),
            // A sample is held to its timeout, and its `/tokenize` call sits outside it: the
            // calibration's bound is `2 * SAMPLE_TIMEOUT_S + 60`, so one sample can take
            // all of its share.
            Self::Sample(_) => Some((Duration::from_secs(SAMPLE_TIMEOUT_S + 60), false)),
            Self::WaitingRetry => Some((RETRY_AFTER, true)),
            _ => None,
        }
    }
}

use Phase::{
    Checking, Hashing, Loading, Phrase, Priming, Sample, StartingServer, Stopping, Verifying,
    WaitingRetry, WaitingStop,
};

const FRESH_STOPPING: &[Phase] = &[
    Stopping,
    Checking,
    StartingServer,
    Verifying,
    Loading,
    Priming,
    Sample(1),
    Sample(2),
];
const FRESH: &[Phase] = &[
    Checking,
    StartingServer,
    Verifying,
    Loading,
    Priming,
    Sample(1),
    Sample(2),
];
const RETRY: &[Phase] = &[
    StartingServer,
    Verifying,
    Loading,
    Priming,
    Sample(1),
    Sample(2),
];
const WAITING: &[Phase] = &[WaitingRetry];
const TRIAL: &[Phase] = &[
    WaitingStop,
    Hashing,
    Checking,
    StartingServer,
    Loading,
    Priming,
    Sample(1),
    Sample(2),
    Phrase(1),
    Phrase(2),
    Phrase(3),
    Phrase(4),
    Phrase(5),
];

/// Which step list a piece of work walks (chosen where the work begins).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Kind {
    /// A session from the check of the host: Mimik's start, a Restart, a switch, a length
    /// change, the previous model after a trial. `stopping`: a model server ran and is
    /// stopped first.
    Fresh {
        stopping: bool,
    },
    /// A start after a failure or a crash: no preflight, no probe.
    Retry,
    /// The wait before a retry.
    Waiting,
    Trial,
}

impl Kind {
    pub(crate) fn phases(self) -> &'static [Phase] {
        match self {
            Self::Fresh { stopping: true } => FRESH_STOPPING,
            Self::Fresh { stopping: false } => FRESH,
            Self::Retry => RETRY,
            Self::Waiting => WAITING,
            Self::Trial => TRIAL,
        }
    }
}

/// Why the list is walked, shown after the step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Qualifier {
    /// A counted attempt, numbered from 1.
    Attempt(u32),
    /// The CPU path after a GPU failure.
    GpuFallback,
    /// A reply-length change went back to the file's earlier length.
    EarlierLength,
}

impl Qualifier {
    fn text(self) -> String {
        match self {
            Self::Attempt(n) => format!("attempt {n} of up to {MAX_FAILURES}"),
            Self::GpuFallback => "on the CPU after a GPU failure".to_owned(),
            Self::EarlierLength => "with the earlier length".to_owned(),
        }
    }
}

/// How long each step of a finished run took.
pub(crate) type Steps = Vec<(Phase, Duration)>;

/// Where a piece of work is: its list, its current step and what the step's time shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Progress {
    pub kind: Kind,
    pub phase: Phase,
    pub qualifier: Option<Qualifier>,
    /// Bytes read of the model file and its size, while it is hashed.
    pub bytes: Option<(u64, u64)>,
    /// The first report of the step's bytes (when, and how many were read), the basis of
    /// the rate.
    first_read: Option<(Instant, u64)>,
    /// A note on the step ("verified (unchanged)").
    pub detail: Option<&'static str>,
    started: Instant,
    step_started: Instant,
    /// The enforced limit of the step and whether it is exact.
    limit: Option<(Duration, bool)>,
    /// The whole work's deadline (a trial's).
    overall: Option<Instant>,
    /// Steps already finished in this run.
    done: Steps,
    /// The last successful run's steps (same file, path and kind).
    reference: Steps,
    last_time: Option<Duration>,
}

impl Progress {
    pub(crate) fn new(
        kind: Kind,
        phase: Phase,
        qualifier: Option<Qualifier>,
        reference: Steps,
    ) -> Self {
        let now = Instant::now();
        let mut progress = Self {
            kind,
            phase,
            qualifier,
            bytes: None,
            first_read: None,
            detail: None,
            started: now,
            step_started: now,
            limit: None,
            overall: None,
            done: Vec::new(),
            reference,
            last_time: None,
        };
        progress.arrive();
        progress
    }

    /// Counts from the start of `earlier` (the same work, begun when it was admitted).
    pub(crate) fn keep_start_of(&mut self, earlier: &Self) {
        self.started = earlier.started;
    }

    /// This step, counted from 1, in its list.
    pub(crate) fn step(&self) -> usize {
        self.kind
            .phases()
            .iter()
            .position(|phase| *phase == self.phase)
            .map_or(1, |at| at + 1)
    }

    pub(crate) fn steps(&self) -> usize {
        self.kind.phases().len()
    }

    fn arrive(&mut self) {
        self.limit = self.phase.limit();
        self.last_time = self
            .reference
            .iter()
            .find(|(phase, _)| *phase == self.phase)
            .map(|(_, took)| *took);
        self.bytes = None;
        self.first_read = None;
        self.detail = None;
    }

    /// `done` of `total` bytes of the model file are read.
    pub(crate) fn read(&mut self, done: u64, total: u64) {
        if self.first_read.is_none() {
            self.first_read = Some((Instant::now(), done));
        }
        self.bytes = Some((done, total));
    }

    /// Moves to `phase` when the list has it (another list's step changes nothing).
    pub(crate) fn enter(&mut self, phase: Phase) -> bool {
        if phase == self.phase || !self.kind.phases().contains(&phase) {
            return false;
        }
        let now = Instant::now();
        self.done.push((self.phase, now - self.step_started));
        self.phase = phase;
        self.step_started = now;
        self.arrive();
        true
    }

    /// The current step is bounded by `bound` (a trial phrase's, known once calibrated).
    pub(crate) fn within(&mut self, bound: Duration) {
        self.limit = Some((bound, false));
    }

    /// The whole work ends by `deadline` at the latest.
    pub(crate) fn set_overall(&mut self, deadline: Instant) {
        self.overall = Some(deadline);
    }

    /// The last successful run's steps, once the file is known (a trial hashes it first).
    pub(crate) fn set_reference(&mut self, reference: Steps) {
        self.reference = reference;
        self.last_time = self
            .reference
            .iter()
            .find(|(phase, _)| *phase == self.phase)
            .map(|(_, took)| *took);
    }

    /// The steps of this run, the current one included (kept when the run succeeds).
    pub(crate) fn finished(&self) -> Steps {
        let mut steps = self.done.clone();
        steps.push((self.phase, self.step_elapsed()));
        steps
    }

    pub(crate) fn step_elapsed(&self) -> Duration {
        Instant::now().saturating_duration_since(self.step_started)
    }

    pub(crate) fn elapsed(&self) -> Duration {
        Instant::now().saturating_duration_since(self.started)
    }

    /// The step as one sentence: "step 8 of 13, trial phrase 1 of 5" with its qualifier.
    pub(crate) fn describe(&self) -> String {
        let steps = self.steps();
        let mut text = if steps > 1 {
            format!("step {} of {steps}, {}", self.step(), self.phase.label())
        } else {
            self.phase.label()
        };
        if let Some(qualifier) = self.qualifier {
            text = format!("{text} ({})", qualifier.text());
        }
        text
    }

    /// The elapsed time and what is left of it, as the page's non-live line.
    pub(crate) fn time_line(&self) -> String {
        let now = Instant::now();
        let step = self.step_elapsed();
        let mut parts = vec![format!("Elapsed {}", span(self.elapsed()))];
        if let Some((limit, exact)) = self.limit {
            let left = span(limit.saturating_sub(step));
            parts.push(if exact {
                format!("{left} left")
            } else {
                format!("at most {left} left in this step")
            });
        }
        if let Some((done, total)) = self.bytes {
            parts.push(format!(
                "read {} of {} MiB",
                done >> 20,
                total.div_ceil(1 << 20)
            ));
            if let Some((at, first)) = self.first_read
                && done > first
                && total > done
            {
                // Bytes read over the file size, at the rate since the first report.
                let since = now.saturating_duration_since(at);
                let nanos = since.as_nanos() * u128::from(total - done) / u128::from(done - first);
                let rest = Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX));
                parts.push(format!("about {} left at this rate", span(rest)));
            }
        }
        if let Some(note) = self.detail {
            parts.push(note.to_owned());
        }
        if let Some(last) = self.last_time {
            parts.push(if step <= last {
                format!("about {} left (last time)", span(last.saturating_sub(step)))
            } else {
                format!("taking longer than last time ({})", span(last))
            });
        }
        if let Some(overall) = self.overall {
            parts.push(format!(
                "the trial ends in at most {}",
                span(overall.saturating_duration_since(now))
            ));
        }
        parts.join(" · ")
    }
}

/// A duration as the page says it: "45 s", "3 min 5 s", "1 h 2 min".
pub(crate) fn span(duration: Duration) -> String {
    let seconds = duration.as_secs() + u64::from(duration.subsec_nanos() > 0);
    match seconds {
        0..=59 => format!("{seconds} s"),
        60..=3599 => match seconds % 60 {
            0 => format!("{} min", seconds / 60),
            rest => format!("{} min {rest} s", seconds / 60),
        },
        _ => match seconds % 3600 / 60 {
            0 => format!("{} h", seconds / 3600),
            rest => format!("{} h {rest} min", seconds / 3600),
        },
    }
}

/// Where the launcher reports to (the manager's reporter).
pub(crate) type Observer = Arc<dyn Fn(StartEvent) + Send + Sync>;

/// What the launcher reports of a start's verification of the model file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartEvent {
    /// The model file's verification begins.
    Verifying,
    /// Bytes of the model file hashed so far, and its size.
    Hashed { done: u64, total: u64 },
    /// The model file is verified; `unchanged`: its remembered hash held, nothing was read.
    Verified { unchanged: bool },
}

/// Writes the steps into the status the page reads.
#[derive(Clone)]
pub(crate) struct Reporter(pub(crate) Arc<Mutex<Status>>);

impl Reporter {
    fn with(&self, change: impl FnOnce(&mut Status)) {
        change(
            &mut self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
    }

    /// The work reaches `phase`: the progress and a running trial show it.
    pub(crate) fn phase(&self, phase: Phase) {
        self.with(|status| {
            if let State::Starting(progress) = &mut status.state {
                progress.enter(phase);
            }
            if let Some(TrialState::Running { phase: shown, .. }) = &mut status.trial {
                *shown = phase;
            }
        });
    }

    /// [`Reporter::phase`], the step bounded by `bound`.
    pub(crate) fn phase_within(&self, phase: Phase, bound: Duration) {
        self.phase(phase);
        self.with(|status| {
            if let State::Starting(progress) = &mut status.state {
                progress.within(bound);
            }
        });
    }

    /// The launcher's report on the model file. Verification starts only from the start of
    /// the model server; the bytes of a first hash count wherever it happens (the probes
    /// of the host hash it first).
    pub(crate) fn event(&self, event: StartEvent) {
        self.with(|status| {
            let State::Starting(progress) = &mut status.state else {
                return;
            };
            match event {
                StartEvent::Verifying => {
                    if progress.phase == StartingServer {
                        progress.enter(Verifying);
                    }
                }
                StartEvent::Hashed { done, total } => {
                    if matches!(
                        progress.phase,
                        Checking | StartingServer | Verifying | Hashing
                    ) {
                        progress.read(done, total);
                    }
                }
                StartEvent::Verified { unchanged } => {
                    if progress.phase == Verifying {
                        progress.bytes = None;
                        progress.detail = unchanged.then_some("verified (unchanged)");
                    }
                }
            }
        });
    }
}

/// The steps of the last successful run, by file, path and entry kind (memory only). Whether
/// a fresh session began with a stop does not make it another kind: the steps it shares
/// took as long.
#[derive(Default)]
pub(crate) struct Memory(HashMap<(String, Path, Kind), Steps>);

fn entry(kind: Kind) -> Kind {
    match kind {
        Kind::Fresh { .. } => Kind::Fresh { stopping: false },
        other => other,
    }
}

impl Memory {
    pub(crate) fn recall(&self, sha256: &str, path: Path, kind: Kind) -> Steps {
        self.0
            .get(&(sha256.to_owned(), path, entry(kind)))
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn keep(&mut self, sha256: &str, path: Path, kind: Kind, steps: Steps) {
        self.0.insert((sha256.to_owned(), path, entry(kind)), steps);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lists_have_their_steps_in_order() {
        assert_eq!(FRESH_STOPPING.len(), FRESH.len() + 1);
        assert_eq!(&FRESH_STOPPING[1..], FRESH);
        assert_eq!(RETRY.len(), 6);
        assert_eq!(TRIAL.len(), 13);
        assert_eq!(TRIAL.first(), Some(&WaitingStop));
        assert_eq!(TRIAL.last(), Some(&Phrase(5)));
    }

    #[test]
    fn durations_read_as_whole_seconds_rounded_up() {
        assert_eq!(span(Duration::from_millis(1)), "1 s");
        assert_eq!(span(Duration::from_secs(59)), "59 s");
        assert_eq!(span(Duration::from_secs(60)), "1 min");
        assert_eq!(span(Duration::from_secs(185)), "3 min 5 s");
        assert_eq!(span(Duration::from_secs(3600)), "1 h");
        assert_eq!(span(Duration::from_mins(62)), "1 h 2 min");
    }

    #[tokio::test(start_paused = true)]
    async fn a_step_outside_the_list_changes_nothing_and_time_never_goes_negative() {
        let mut progress = Progress::new(
            Kind::Retry,
            StartingServer,
            Some(Qualifier::Attempt(2)),
            vec![(Loading, Duration::from_secs(10))],
        );
        assert!(!progress.enter(Checking));
        assert_eq!(progress.phase, StartingServer);
        assert_eq!(
            progress.describe(),
            "step 1 of 6, starting the model server (attempt 2 of up to 4)"
        );
        assert!(progress.enter(Loading));
        progress.within(Duration::from_secs(65));
        assert_eq!(progress.step(), 3);
        assert!(progress.time_line().contains("about 10 s left (last time)"));
        tokio::time::advance(Duration::from_secs(70)).await;
        let line = progress.time_line();
        assert!(
            line.contains("taking longer than last time (10 s)"),
            "{line}"
        );
        assert!(line.contains("at most 0 s left in this step"), "{line}");
    }

    #[test]
    fn a_calibration_sample_has_its_timeout_and_its_tokenize_call() {
        assert_eq!(
            Sample(1).limit(),
            Some((Duration::from_secs(SAMPLE_TIMEOUT_S + 60), false))
        );
        assert_eq!(Sample(2).limit(), Sample(1).limit());
    }

    #[tokio::test(start_paused = true)]
    async fn the_rate_of_a_hash_counts_from_its_first_report_not_from_the_start_of_the_step() {
        let mut progress = Progress::new(Kind::Fresh { stopping: false }, Checking, None, vec![]);
        tokio::time::advance(Duration::from_secs(50)).await;
        progress.read(1 << 20, 12 << 20);
        tokio::time::advance(Duration::from_secs(10)).await;
        progress.read(2 << 20, 12 << 20);
        // 1 MiB in 10 s, 10 MiB to go.
        let line = progress.time_line();
        assert!(
            line.contains("about 1 min 40 s left at this rate"),
            "{line}"
        );
    }
}
