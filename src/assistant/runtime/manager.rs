//! Generations and transitions (assisted sheet plan §1.4a, "Generations", "Transitions"): one
//! manager task serializes every start and stop, so two model servers never coexist; it
//! feeds the jobs' runtime events (generation started, ended with its reason, ready with the
//! budgets) and never reports a child exit to the breaker.
//!
//! An attempt is: start (spawn and the endpoint check), `/health` within 60 s, priming within
//! 300 s, calibration within twice the path's run-deadline cap. On the CPU path:
//!
//! | Event | Action | Failure count |
//! |---|---|---|
//! | Sandbox-attributable failure at a level | next allowed level that passed its probe, at once; with none left (level 1 is the only one Mimik uses, ASSIST-43) `unavailable` with the fix | not counted |
//! | Any other failed attempt | retry after 2 minutes, same level | +1 |
//! | Child exits while ready | restart now | +1 only within 10 minutes of `ready` |
//! | Two consecutive run-deadline expiries | stop as hung, restart, re-measure | as a child exit |
//! | Failure count reaches 4 | `unavailable` until Restart | — |
//! | Budgets do not derive ("too slow") | `unavailable` with the numbers, no retry | — |
//! | `ready` for 10 minutes | failure count back to 0 | reset |
//! | Restart (or Mimik's start) | stop, new attempt from level 1, breaker reset | reset |
//!
//! On the GPU path (ASSIST-27; `--assistant-device` not `cpu`) any failed attempt (the
//! start, `/health`, the offload check, priming, calibration, budgets that do not derive), a
//! child exit while ready or a stop as hung moves to the CPU path at once at the same level,
//! uncounted, and the CPU path stays until Mimik restarts or the owner presses "Try the GPU
//! again"; a Restart keeps it.
//!
//! Every timer is Tokio's, so tests drive the clock.
use super::budgets::NoBudgets;
use super::calibration::{self, Calibration};
use super::models::{
    DeviceDefault, Measurement, Model, ModelFile, ModelStatus, NineB, OlderPrompt, TrialKey,
    TrialReport, TrialState, default_model, gpu_default_name, phrase_bound, trial_bound,
};
use super::server::{Health, Server};
use super::{Level, Path, trial};
use crate::assistant::jobs::{Assistant, Budgets, EndReason, RuntimeEvent, Signal};
use crate::assistant::model::GPU_DEFAULT_MODEL_SHA256;
use crate::assistant::records::{Record, Records};
use async_trait::async_trait;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::time::Instant;

pub(crate) const HEALTH_WITHIN: Duration = Duration::from_secs(60);
pub(crate) const HEALTH_POLL: Duration = Duration::from_millis(250);
pub(crate) const PRIMING_WITHIN: Duration = Duration::from_secs(300);
pub(crate) const RETRY_AFTER: Duration = Duration::from_secs(120);
/// A child exit counts within this long of `ready`; `ready` this long resets the count.
pub(crate) const READY_WINDOW: Duration = Duration::from_secs(600);
pub(crate) const MAX_FAILURES: u32 = 4;

/// Why an attempt failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Fail {
    /// Attributable to the sandbox (a sandbox setup error, a status-descriptor failure before
    /// exec, the level's preflight): not counted; the next allowed level is tried, and with
    /// none left (only level 1 is used, ASSIST-43) the assistant is unavailable.
    Sandbox(String),
    /// Anything else (model-load errors, hash mismatches, exits after exec): counted.
    Other(String),
}

/// A started child of one generation.
#[async_trait]
pub(crate) trait Child: Send + Sync {
    fn server(&self) -> Arc<dyn Server>;
    /// Resolves once the child has exited, with why (callable repeatedly).
    async fn exited(&self) -> Fail;
    /// Stops it (SIGTERM, SIGKILL after 5 s) and resolves once it is gone; idempotent.
    async fn stop(&self);
    /// `/health` answered: the TCP levels delete the token file now.
    fn answered_health(&self) {}
    /// What the page should say about this child's confinement (for example "temporary
    /// space unbounded").
    fn notes(&self) -> Vec<String> {
        Vec::new()
    }
    /// What the Landlock layer inside the Hakoniwa sandbox did, as its closure reported it
    /// (`None`: not reported).
    fn landlock(&self) -> Option<super::hako::Landlock> {
        None
    }
    /// After `/health`: on the GPU path every layer must be on the GPU (the offload line),
    /// otherwise the attempt is a GPU failure.
    async fn loaded(&self) -> Result<(), String> {
        Ok(())
    }
}

/// The selected GPU as the assistant page shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeviceView {
    pub pci: String,
    pub node: String,
    /// The in-sandbox listing's name (`Vulkan0`) and description.
    pub name: String,
    pub description: String,
    pub total_mib: u64,
}

/// Starts children (the real one spawns the runner; tests use a fake).
#[async_trait]
pub(crate) trait Launcher: Send + Sync {
    /// Checks made once per start from level 1 (the pidfile lock, the model's size): an
    /// error keeps the assistant unavailable with that reason.
    async fn preflight(&self, path: Path) -> Result<(), String>;
    /// The start-time probe of each level, in order.
    async fn probe(&self, path: Path) -> Vec<(Level, Result<(), String>)>;
    /// Stops in-flight probes and starts at their next check (before they spawn or record
    /// anything).
    fn cancel(&self) {}
    /// Waits until no probe, start or child watcher of an earlier session runs.
    async fn settle(&self) {}
    async fn start(
        &self,
        generation: u64,
        level: Level,
        path: Path,
    ) -> Result<Arc<dyn Child>, Fail>;
    /// What an advisory check believes explains a failed start of `path` (a library the host
    /// seems to lack): added to the failure's reason, never a reason to refuse a start.
    fn failure_hint(&self, _path: Path) -> Option<String> {
        None
    }
    /// The GPU the last GPU probe selected.
    fn device(&self) -> Option<DeviceView> {
        None
    }
    /// Why render nodes were not used, from the last GPU probe.
    fn device_notes(&self) -> Vec<String> {
        Vec::new()
    }
    /// The models folder as the page's notes name it (an absolute path).
    fn models_folder(&self) -> String {
        "the models folder".to_owned()
    }
    /// The model the next start loads (its file, the SHA-256 it must have and the alias).
    fn select(&self, _model: &Model) {}
    /// Whether the model file `name` is in the models folder (a regular file the load would
    /// accept to open; not hashed). A launcher that starts nothing says yes.
    async fn model_present(&self, _name: &str) -> bool {
        true
    }
    /// Hashes the model file `name` in the models folder (the trial records the hash it
    /// then expects at every load). A launcher that starts nothing cannot.
    async fn hash_model(&self, _name: &str) -> Result<ModelFile, String> {
        Err("this assistant cannot examine model files".into())
    }
}

/// A load whose hash differs from the one recorded fails with this (the page says the file
/// changed): it is not retried, because the file will not change back by itself.
pub(crate) const MODEL_CHANGED: &str = "the model file changed";

/// The runtime's state as the page shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum State {
    Starting(&'static str),
    Ready,
    Unavailable(String),
}

/// Everything the assistant page shows of the runtime.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Status {
    pub state: State,
    pub path: Path,
    pub level: Option<Level>,
    /// Levels skipped, with why.
    pub skipped: Vec<(Level, String)>,
    pub generation: u64,
    pub failures: u32,
    pub last_failure: Option<String>,
    pub calibration: Option<Calibration>,
    pub budgets: Option<Budgets>,
    pub notes: Vec<String>,
    /// The Landlock layer of the Hakoniwa sandbox (DEP-25); `None` until it reports.
    pub landlock: Option<super::hako::Landlock>,
    /// A Restart is being carried out.
    pub restarting: bool,
    /// The GPU path is wanted (`--assistant-device` is not `cpu`).
    pub gpu_wanted: bool,
    /// Why the GPU is not in use although it is wanted (shown with "Try the GPU again").
    pub gpu_note: Option<String>,
    /// The GPU the last GPU probe selected.
    pub device: Option<DeviceView>,
    /// Why other render nodes were not used.
    pub device_notes: Vec<String>,
    /// The owner's model setting.
    pub model: ModelStatus,
    /// The latest model trial.
    pub trial: Option<TrialState>,
}

impl Status {
    fn new(path: Path, models: &ModelSetup) -> Self {
        Self {
            model: ModelStatus {
                active: models.active.clone(),
                previous: models.previous.clone(),
                older_prompt: Measurement::of(models.older_prompt.active),
                from_owner_setting: models.from_owner_setting,
                switchable: models.records.is_some(),
                next_trial: 1,
                pending: false,
                work: false,
                note: None,
                device_default: if models.device_default {
                    DeviceDefault::On { note: None }
                } else {
                    DeviceDefault::Off
                },
            },
            trial: None,
            gpu_wanted: path == Path::Gpu,
            gpu_note: None,
            device: None,
            device_notes: Vec::new(),
            state: State::Starting("checking"),
            path,
            level: None,
            skipped: Vec::new(),
            generation: 0,
            failures: 0,
            last_failure: None,
            calibration: None,
            budgets: None,
            notes: Vec::new(),
            landlock: None,
            restarting: false,
        }
    }
}

enum Command {
    /// The owner's Restart; with `gpu`, "Try the GPU again".
    Restart {
        gpu: bool,
    },
    Shutdown(oneshot::Sender<()>),
    /// The owner's model setting: it ends the running generation like a Restart does.
    Model(ModelCommand),
}

/// A model request: carried out after the running generation has ended.
enum ModelCommand {
    Trial(TrialRequest),
    Switch { key: TrialKey, by: i64 },
    SwitchBack { by: i64 },
}

/// The owner's "Try": the candidate file and who asked.
struct TrialRequest {
    number: u64,
    by: i64,
    spec: String,
    name: String,
}

/// The model the manager starts with, and where its changes are stored.
pub(crate) struct ModelSetup {
    pub active: Model,
    pub previous: Option<Model>,
    /// Where `Switch` and `Switch back` are stored; without it the owner cannot try models.
    pub records: Option<Arc<dyn Records>>,
    /// The active model comes from the owner's stored setting, not from the start flags.
    pub from_owner_setting: bool,
    /// Neither a start flag nor the stored setting names a model: `active` is a placeholder
    /// and each session picks the device default (ASSIST-55, `default_model`).
    pub device_default: bool,
    /// Whether the stored trial of the active and of the previous model measured it with an
    /// older prompt than the current one (ASSIST-56).
    pub older_prompt: OlderPrompt,
}

#[cfg(test)]
impl ModelSetup {
    /// A manager whose model is fixed: only `max_tokens` is known.
    pub(crate) fn fixed(max_tokens: u32) -> Self {
        Self {
            active: Model {
                spec: String::new(),
                name: String::new(),
                sha256: String::new(),
                alias: String::new(),
                max_tokens,
                size: None,
            },
            previous: None,
            records: None,
            from_owner_setting: false,
            device_default: false,
            older_prompt: OlderPrompt::default(),
        }
    }
}

/// What the rest of Mimik holds: the status and the owner's commands.
#[derive(Clone)]
pub(crate) struct Handle {
    status: Arc<Mutex<Status>>,
    commands: mpsc::UnboundedSender<Command>,
    /// The owner's Cancel of a running trial.
    cancel: Arc<Cancel>,
}

/// The owner's Cancel: which trial it is for, so a Cancel that arrives early, or late, never
/// cancels another trial.
#[derive(Default)]
struct Cancel {
    /// The number of the trial last asked to stop (0: none).
    trial: std::sync::atomic::AtomicU64,
    wake: Notify,
}

impl Cancel {
    /// Records a Cancel for trial `number` and wakes the manager. Trial numbers only grow, so
    /// an older Cancel never replaces a newer one.
    fn publish(&self, number: u64) {
        self.trial.fetch_max(number, Ordering::SeqCst);
        self.wake.notify_one();
    }
}

/// The model-work gate is held, or a trial or a model change is under way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WorkBusy;

/// Holds the model-work gate; dropping it releases the gate.
pub(crate) struct WorkGuard(Arc<Mutex<Status>>);

impl Drop for WorkGuard {
    fn drop(&mut self) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .model
            .work = false;
    }
}

/// Whether "Try the GPU again" was accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TryGpu {
    Accepted,
    /// A Restart (or another try) is already being carried out: no second transition.
    AlreadyRunning,
    /// The GPU is in use, or not wanted (`--assistant-device cpu`): nothing to try.
    NotApplicable,
}

/// Whether a Restart was accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Restart {
    Accepted,
    /// A Restart is already being carried out: no second transition.
    AlreadyRunning,
}

/// Whether a "Try" was accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TryModel {
    /// The trial's number.
    Accepted(u64),
    /// A trial is already running: no second one.
    Busy,
    /// Another model change (a Switch, a Switch back) or a restart is being carried out.
    Pending,
    /// This assistant keeps no model setting (the development endpoint).
    Unsupported,
}

/// What "Switch", "Switch back" and "Keep" answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Chosen {
    Accepted,
    /// A trial is running, or the model setting is not supported here.
    Busy,
    /// The trial is not the latest finished one (another hash, an older trial or an older
    /// generation), or nothing passed: nothing was changed.
    Stale,
    /// There is no previous model.
    NoPrevious,
    /// Another model change or a restart is being carried out: nothing else is started.
    Pending,
}

/// What the owner's Cancel did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CancelTrial {
    Done,
    NoTrial,
    /// The trial shown to the owner is not the one running.
    Stale,
}

impl Handle {
    pub(crate) fn status(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The owner's Restart.
    pub(crate) fn restart(&self) -> Restart {
        let mut status = self
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if status.restarting {
            return Restart::AlreadyRunning;
        }
        status.restarting = true;
        drop(status);
        let _ = self.commands.send(Command::Restart { gpu: false });
        Restart::Accepted
    }

    /// The owner's "Try the GPU again": a Restart that starts from the GPU path again. Only
    /// while the GPU is wanted and the CPU path is in use.
    pub(crate) fn try_gpu(&self) -> TryGpu {
        let mut status = self
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if status.restarting {
            return TryGpu::AlreadyRunning;
        }
        if !status.gpu_wanted || status.path == Path::Gpu {
            return TryGpu::NotApplicable;
        }
        status.restarting = true;
        drop(status);
        let _ = self.commands.send(Command::Restart { gpu: true });
        TryGpu::Accepted
    }

    /// The owner's "Try": starts the trial of the model file `name` (spec `spec`) after the
    /// running generation has stopped. One trial at a time.
    pub(crate) fn try_model(&self, by: i64, spec: String, name: String) -> TryModel {
        let mut status = self.lock();
        if !status.model.switchable {
            return TryModel::Unsupported;
        }
        if status.trial.as_ref().is_some_and(TrialState::running) {
            return TryModel::Busy;
        }
        if status.model.pending || status.restarting || status.model.work {
            return TryModel::Pending;
        }
        status.model.pending = true;
        let number = status.model.next_trial;
        status.model.next_trial += 1;
        status.model.note = None;
        status.trial = Some(TrialState::Running {
            number,
            spec: spec.clone(),
            phase: "waiting for the model server to stop",
        });
        drop(status);
        let _ = self
            .commands
            .send(Command::Model(ModelCommand::Trial(TrialRequest {
                number,
                by,
                spec,
                name,
            })));
        TryModel::Accepted(number)
    }

    /// The owner's Cancel of trial `shown`: stops it and brings the previous model back.
    pub(crate) fn cancel_trial(&self, shown: u64) -> CancelTrial {
        // Under the lock: the trial the page showed must be the one running now.
        // The guard is held through the publication, so a Cancel validated against one trial
        // cannot publish after a newer trial's.
        let status = self.lock();
        match status.trial {
            Some(TrialState::Running { number, .. }) if number != shown => CancelTrial::Stale,
            Some(TrialState::Running { number, .. }) => {
                self.cancel.publish(number);
                CancelTrial::Done
            }
            _ => CancelTrial::NoTrial,
        }
    }

    /// The owner's "Switch": the latest finished trial, if `key` names it, becomes the active
    /// model (stored first; the model server restarts with it).
    pub(crate) fn switch(&self, by: i64, key: &TrialKey) -> Chosen {
        let mut status = self.lock();
        if !status.model.switchable || status.trial.as_ref().is_some_and(TrialState::running) {
            return Chosen::Busy;
        }
        if status.model.pending || status.restarting || status.model.work {
            return Chosen::Pending;
        }
        match &status.trial {
            Some(TrialState::Passed(report)) if report.key == *key => {}
            _ => return Chosen::Stale,
        }
        status.model.note = None;
        status.model.pending = true;
        status.restarting = true;
        drop(status);
        let _ = self.commands.send(Command::Model(ModelCommand::Switch {
            key: key.clone(),
            by,
        }));
        Chosen::Accepted
    }

    /// The owner's "Switch back": the previous model becomes the active one again.
    pub(crate) fn switch_back(&self, by: i64) -> Chosen {
        let mut status = self.lock();
        if !status.model.switchable || status.trial.as_ref().is_some_and(TrialState::running) {
            return Chosen::Busy;
        }
        if status.model.pending || status.restarting || status.model.work {
            return Chosen::Pending;
        }
        if status.model.previous.is_none() {
            return Chosen::NoPrevious;
        }
        status.model.note = None;
        status.model.pending = true;
        status.restarting = true;
        drop(status);
        let _ = self
            .commands
            .send(Command::Model(ModelCommand::SwitchBack { by }));
        Chosen::Accepted
    }

    /// The owner's "Keep the current model": dismisses a finished trial. The previous model
    /// already runs again (every trial ends by restarting it), so nothing restarts.
    pub(crate) fn keep(&self) -> Chosen {
        let mut status = self.lock();
        if status.trial.as_ref().is_some_and(TrialState::running) {
            return Chosen::Busy;
        }
        if status.model.pending {
            return Chosen::Pending;
        }
        if status.trial.take().is_none() {
            return Chosen::Stale;
        }
        Chosen::Accepted
    }

    /// Takes the model-work gate for a download or a delete: one of these at a time, never
    /// beside a trial, a Switch or a Switch back. The gate is released when the guard drops.
    pub(crate) fn model_work(&self) -> Result<WorkGuard, WorkBusy> {
        let mut status = self.lock();
        if status.trial.as_ref().is_some_and(TrialState::running)
            || status.model.pending
            || status.model.work
        {
            return Err(WorkBusy);
        }
        status.model.work = true;
        Ok(WorkGuard(Arc::clone(&self.status)))
    }

    /// The file names of the active and previous model (a delete must keep them).
    pub(crate) fn model_files(&self) -> Vec<String> {
        let status = self.lock();
        std::iter::once(&status.model.active)
            .chain(status.model.previous.as_ref())
            .map(|model| model.name.clone())
            .collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Status> {
        self.status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Stops the child and the manager; resolves once the child is gone (or the manager
    /// has already ended).
    pub(crate) async fn shutdown(&self) {
        let (reply, done) = oneshot::channel();
        if self.commands.send(Command::Shutdown(reply)).is_ok() {
            let _ = done.await;
        }
    }
}

/// The manager's configuration.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Config {
    pub path: Path,
    /// The levels this manager may start, in order. Mimik's own runtime uses
    /// [`USED_LEVELS`] (level 1 only, ASSIST-43); the development endpoint, which reaches a
    /// hand-started external server and starts nothing, reports that server as level 3.
    pub levels: &'static [Level],
}

/// The levels Mimik starts llama-server at (ASSIST-43): level 1 or not at all. Level 3's code
/// stays built but no manager path reaches it; it returns later behind an owner opt-in.
pub(crate) const USED_LEVELS: &[Level] = &[Level::Sandbox];

/// Why the assistant is unavailable when level 1 cannot be used: how to fix it.
pub(crate) const LEVEL_1_NEEDED: &str = "the model server runs only inside Mimik's sandbox \
(level 1), which is not usable here. Allow unprivileged user namespaces: \
user.max_user_namespaces above 0, kernel.unprivileged_userns_clone = 1 where the kernel has \
it, and on Ubuntu 24.04 kernel.apparmor_restrict_unprivileged_userns = 0 or the AppArmor \
profile that `mimik assistant-apparmor-profile` prints. Then press Restart. Level 3 \
(unconfined) is not used in this release";

/// Answers a shutdown request once the child is gone.
fn shut(reply: oneshot::Sender<()>) {
    let _ = reply.send(());
}

/// One line on standard error per transition: states, levels, reasons and measurements,
/// never prompt text, URLs with tokens or the token.
fn log(line: &str) {
    eprintln!("assistant: {line}");
}

/// The measurement as one log line.
fn measured(calibration: &Calibration, budgets: Option<&Budgets>) -> String {
    let samples: Vec<String> = calibration
        .samples
        .iter()
        .map(|s| {
            format!(
                "W {:.0} ms, prompt {} tokens in {:.0} ms, cached {}, generated {} in {:.0} ms, filler {} tokens",
                s.wall_ms, s.prompt_n, s.prompt_ms, s.cached_n, s.predicted_n, s.predicted_ms, s.filler_tokens
            )
        })
        .collect();
    let budgets = budgets.map_or_else(
        || "no budgets".to_owned(),
        |b| {
            format!(
                "B {} bytes, N(B) {} tokens, R {} s, D {}, Q {} s, busy after expiry {} s",
                b.bounds.bytes,
                b.bounds.tokens,
                b.run.as_secs(),
                b.depth,
                b.queue_wait.as_secs(),
                b.busy_after_expiry.as_secs()
            )
        },
    );
    format!(
        "priming {} prefix tokens; samples [{}]; density {:.2} and {:.2} bytes per token; {budgets}",
        calibration.prefix_tokens,
        samples.join("; "),
        calibration.density[0],
        calibration.density[1]
    )
}

/// Starts the manager task for `assistant`, with a fixed model (tests).
#[cfg(test)]
pub(crate) fn spawn(
    assistant: Arc<Assistant>,
    launcher: Arc<dyn Launcher>,
    config: Config,
) -> Handle {
    spawn_with(assistant, launcher, config, ModelSetup::fixed(352))
}

/// Starts the manager task for `assistant` with `models` as the owner's model setting.
pub(crate) fn spawn_with(
    assistant: Arc<Assistant>,
    launcher: Arc<dyn Launcher>,
    config: Config,
    models: ModelSetup,
) -> Handle {
    let status = Arc::new(Mutex::new(Status::new(config.path, &models)));
    let (commands, incoming) = mpsc::unbounded_channel();
    let (signal_sender, signals) = mpsc::unbounded_channel();
    assistant.subscribe(signal_sender);
    let cancel = Arc::new(Cancel::default());
    let manager = Manager {
        assistant,
        launcher,
        config,
        status: Arc::clone(&status),
        commands: incoming,
        signals,
        generation: 0,
        failures: 0,
        gpu_failed: None,
        health_answered: Arc::new(AtomicBool::new(false)),
        active: models.active,
        device_default: models.device_default,
        nine_b_mismatch: false,
        previous: models.previous,
        older_prompt: models.older_prompt,
        records: models.records,
        cancel: Arc::clone(&cancel),
        trial: 0,
    };
    tokio::spawn(manager.run());
    Handle {
        status,
        commands,
        cancel,
    }
}

/// How an attempt ended.
enum Attempt {
    Ready(Arc<dyn Child>, Box<Calibration>, Budgets),
    Failed(Fail),
    TooSlow(String),
    Interrupted(Command),
}

/// How the ready phase ended.
enum Ended {
    /// The child exited or was stopped as hung: restart at the same level.
    Exited {
        counted: bool,
        why: String,
    },
    Restart {
        gpu: bool,
    },
    Model(ModelCommand),
    Shutdown(oneshot::Sender<()>),
}

/// How a session ended.
enum Next {
    /// Start again from level 1 (the owner's Restart); with `gpu`, from the GPU path again.
    Fresh {
        gpu: bool,
    },
    /// The owner's model request, carried out with no model server running.
    Model(ModelCommand),
    Stop(oneshot::Sender<()>),
}

/// How one path's attempts ended.
enum PathEnd {
    Next(Next),
    /// The GPU path failed: the CPU path next, at once, uncounted.
    GpuFailed(String),
}

/// What waiting for the owner gave.
enum Wake {
    Restart { gpu: bool },
    Model(ModelCommand),
    Shutdown(oneshot::Sender<()>),
    Timer,
}

impl From<Command> for Next {
    fn from(command: Command) -> Self {
        match command {
            Command::Restart { gpu } => Next::Fresh { gpu },
            Command::Shutdown(reply) => Next::Stop(reply),
            Command::Model(command) => Next::Model(command),
        }
    }
}

struct Manager {
    assistant: Arc<Assistant>,
    launcher: Arc<dyn Launcher>,
    config: Config,
    status: Arc<Mutex<Status>>,
    commands: mpsc::UnboundedReceiver<Command>,
    signals: mpsc::UnboundedReceiver<Signal>,
    generation: u64,
    failures: u32,
    /// Why the GPU path failed: the CPU path is used until Mimik restarts or the owner
    /// presses "Try the GPU again" (a Restart keeps it).
    gpu_failed: Option<String>,
    /// The model the sessions run, and the one before it (for "Switch back").
    active: Model,
    /// The active model is the device default, picked per session; a Switch or Switch back
    /// ends that.
    device_default: bool,
    /// The device default's 9B file failed its hash check at a load: Granite stands in until
    /// "Try the GPU again" or Mimik's restart.
    nine_b_mismatch: bool,
    previous: Option<Model>,
    /// Whether the active and the previous model were measured with an older prompt.
    older_prompt: OlderPrompt,
    /// Where model changes are stored (none: the owner cannot change the model).
    records: Option<Arc<dyn Records>>,
    /// The owner's Cancel of a running trial.
    cancel: Arc<Cancel>,
    /// The trial being carried out (0: none).
    trial: u64,
    /// Whether `/health` answered in the current attempt: from then on a failure (priming,
    /// calibration, the offload check) is not a library that failed to load.
    health_answered: Arc<AtomicBool>,
}

impl Manager {
    fn update(&self, change: impl FnOnce(&mut Status)) {
        change(
            &mut self
                .status
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
    }

    /// Waits for the owner, at most `timeout` when given.
    async fn wait(&mut self, timeout: Option<Duration>) -> Wake {
        let sleep = async {
            match timeout {
                Some(timeout) => tokio::time::sleep(timeout).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            command = self.commands.recv() => match command {
                Some(Command::Restart { gpu }) => Wake::Restart { gpu },
                Some(Command::Shutdown(reply)) => Wake::Shutdown(reply),
                Some(Command::Model(command)) => Wake::Model(command),
                // Every handle is gone: nothing can wake it again.
                None => std::future::pending().await,
            },
            () = sleep => Wake::Timer,
        }
    }

    fn unavailable(&self, why: String) {
        log(&format!("unavailable: {why}"));
        self.assistant.event(RuntimeEvent::GenerationEnded {
            generation: self.generation,
            reason: EndReason::Unavailable(why.clone()),
        });
        self.update(|status| {
            status.state = State::Unavailable(why);
            status.restarting = false;
            status.budgets = None;
        });
    }

    async fn run(mut self) {
        // Mimik's start counts as a Restart: from level 1, breaker and count reset. Only a
        // Restart does again; a model change starts its session with the breaker as it is.
        let mut reset_breaker = true;
        loop {
            let mut next = Some(self.session(reset_breaker).await);
            if let Some(Next::Model(command)) = next {
                // The model server is gone; whatever the request does, a session starts
                // next (with the previous model after a trial, the new one after a switch).
                // A trial leaves Restart available: it ends the trial.
                next = self.model_command(command).await;
                self.update(|status| status.model.pending = false);
            }
            reset_breaker = false;
            match next {
                Some(Next::Stop(reply)) => {
                    self.launcher.cancel();
                    self.launcher.settle().await;
                    return shut(reply);
                }
                Some(Next::Fresh { gpu }) => {
                    // Fresh always follows the owner's command: it stays "being carried out"
                    // until the session ends in `ready` or `unavailable`, even if one of them
                    // cleared the flag just before the command was taken.
                    self.update(|status| status.restarting = true);
                    reset_breaker = true;
                    if gpu {
                        self.gpu_failed = None;
                        self.nine_b_mismatch = false;
                    }
                }
                Some(Next::Model(_)) | None => {}
            }
        }
    }

    /// Unavailable with `why` until the owner's Restart or shutdown.
    async fn hold(&mut self, why: String) -> Next {
        self.unavailable(why);
        match self.wait(None).await {
            Wake::Restart { gpu } => Next::Fresh { gpu },
            Wake::Timer => Next::Fresh { gpu: false },
            Wake::Model(command) => Next::Model(command),
            Wake::Shutdown(reply) => Next::Stop(reply),
        }
    }

    /// Runs `work` unless the owner's Restart or shutdown comes first: then `work` is dropped,
    /// its blocking part is cancelled, and this waits until that has finished and cleaned up
    /// (so nothing of it overlaps the next session or outlives the shutdown).
    async fn interruptible<T>(&mut self, work: impl Future<Output = T>) -> Result<T, Next> {
        let next = tokio::select! {
            done = work => return Ok(done),
            command = self.commands.recv() => match command {
                Some(command) => Next::from(command),
                None => std::future::pending().await,
            },
        };
        self.launcher.cancel();
        self.launcher.settle().await;
        Err(next)
    }

    /// `why`, a failure of an attempt to start or to reach `/health`, with the launcher's hint
    /// for `path` when there is one and `why` lacks it. Only those failures: a probe's reasons,
    /// a policy skip and the exit of a server that was ready are not explained by a library.
    fn hinted(&self, path: Path, why: String) -> String {
        if self.health_answered.load(Ordering::SeqCst) {
            return why;
        }
        match self.launcher.failure_hint(path) {
            Some(hint) if !why.contains(&hint) => format!("{why} ({hint})"),
            _ => why,
        }
    }

    /// The levels that passed their probe and this manager may start, in order; the others
    /// are recorded as skipped.
    fn probed(&mut self, probed: Vec<(Level, Result<(), String>)>) -> Vec<Level> {
        let mut levels = Vec::new();
        for (level, result) in probed {
            let why = match result {
                Ok(()) if self.config.levels.contains(&level) => {
                    levels.push(level);
                    continue;
                }
                Ok(()) => "not used in this release (ASSIST-43)".to_owned(),
                Err(why) => why,
            };
            log(&format!("level {} not used: {why}", level.number()));
            self.update(|status| status.skipped.push((level, why)));
        }
        levels
    }

    /// Unavailable because no usable level is left: the fix, with level 1's own reason.
    fn no_level(&self) -> String {
        let level_1 = self
            .status()
            .skipped
            .iter()
            .find(|(level, _)| *level == Level::Sandbox)
            .map(|(_, why)| why.clone());
        match level_1 {
            Some(why) => format!("{LEVEL_1_NEEDED} (level 1: {why})"),
            None => LEVEL_1_NEEDED.to_owned(),
        }
    }

    /// A new generation at `level`.
    fn begin(&mut self, level: Level) -> u64 {
        self.generation += 1;
        let generation = self.generation;
        log(&format!(
            "starting: generation {generation}, level {}",
            level.number()
        ));
        self.assistant
            .event(RuntimeEvent::GenerationStarted { generation });
        self.update(|status| {
            status.state = State::Starting("starting the model server");
            status.level = Some(level);
            status.generation = generation;
            status.notes.clear();
            status.landlock = None;
        });
        generation
    }

    /// The preflight of the session (the CPU build's: the fallback always needs it).
    async fn preflight(&mut self) -> Result<(), Next> {
        let launcher = Arc::clone(&self.launcher);
        match self.interruptible(launcher.preflight(Path::Cpu)).await? {
            Ok(()) => Ok(()),
            Err(why) => Err(self.hold(why).await),
        }
    }

    /// `path`'s probes: the levels to try (its own, as its build passed them).
    async fn levels(&mut self, path: Path) -> Result<Vec<Level>, Next> {
        let launcher = Arc::clone(&self.launcher);
        let probed = self.interruptible(launcher.probe(path)).await?;
        self.update(|status| {
            status.path = path;
            status.skipped.clear();
        });
        if path == Path::Gpu {
            let (device, notes) = (launcher.device(), launcher.device_notes());
            self.update(|status| {
                status.device = device;
                status.device_notes = notes;
            });
        }
        Ok(self.probed(probed))
    }

    /// Hands the model the next start loads to the launcher. While the device default is in
    /// force it is picked first (ASSIST-55): the 9B on the GPU path when its file is there,
    /// Granite otherwise, with the note the page shows.
    async fn select_model(&mut self, gpu: bool) {
        if self.device_default {
            let file = if !gpu {
                NineB::Missing
            } else if self.nine_b_mismatch {
                NineB::Mismatch
            } else if self.launcher.model_present(&gpu_default_name()).await {
                NineB::Present
            } else {
                NineB::Missing
            };
            let (model, note) = default_model(
                gpu,
                self.config.path == Path::Gpu,
                file,
                &self.launcher.models_folder(),
            );
            if let Some(note) = &note {
                log(note);
            }
            self.active = model.clone();
            self.update(|status| {
                status.model.active = model;
                status.model.device_default = DeviceDefault::On { note: note.clone() };
            });
        }
        // A manager without a model setting (tests) leaves the launcher's own model alone.
        if !self.active.name.is_empty() {
            self.launcher.select(&self.active);
        }
    }

    /// The GPU path failed: the CPU path is used from now on, until Mimik restarts or the
    /// owner presses "Try the GPU again".
    fn gpu_fallback(&mut self, why: String) {
        log(&format!("GPU path not used, on the CPU path: {why}"));
        self.gpu_failed = Some(why.clone());
        self.update(|status| {
            status.gpu_note = Some(why);
            // The GPU child's state is over: the CPU server starts now.
            status.state = State::Starting("checking");
            status.path = Path::Cpu;
            status.device = None;
            status.device_notes.clear();
            status.budgets = None;
        });
    }

    /// One start from level 1 until the owner's Restart (or shutdown) ends it: the GPU path
    /// first when it is wanted and has not failed, the CPU path after a GPU failure.
    async fn session(&mut self, reset_breaker: bool) -> Next {
        // Nothing of an earlier session (a child's watcher, a cancelled probe) runs beside it.
        self.launcher.settle().await;
        let gpu = self.config.path == Path::Gpu && self.gpu_failed.is_none();
        self.select_model(gpu).await;
        self.failures = 0;
        if reset_breaker {
            self.assistant.reset_breaker();
        }
        let note = self.gpu_failed.clone();
        self.update(|status| {
            status.state = State::Starting("checking");
            status.skipped.clear();
            status.failures = 0;
            status.last_failure = None;
            status.level = None;
            status.gpu_note = note;
            status.landlock = None;
        });
        if let Err(next) = self.preflight().await {
            return next;
        }
        let mut path = if gpu { Path::Gpu } else { Path::Cpu };
        loop {
            let levels = match self.levels(path).await {
                Ok(levels) => levels,
                Err(next) => return next,
            };
            match self.on_path(path, &levels).await {
                PathEnd::Next(next) => return next,
                PathEnd::GpuFailed(why) => {
                    self.gpu_fallback(why);
                    // The device default is the CPU's now (Granite), not the 9B that failed.
                    self.select_model(false).await;
                    path = Path::Cpu;
                }
            }
        }
    }

    /// `path`'s attempts over its levels until the owner acts, the assistant is held
    /// unavailable, or (on the GPU path) an attempt fails.
    async fn on_path(&mut self, path: Path, levels: &[Level]) -> PathEnd {
        let gpu = path == Path::Gpu;
        let mut index = 0;
        loop {
            if self.failures >= MAX_FAILURES {
                let last = self.status().last_failure.unwrap_or_default();
                return PathEnd::Next(
                    self.hold(format!(
                        "the model server failed {MAX_FAILURES} times: {last}"
                    ))
                    .await,
                );
            }
            let Some(&level) = levels.get(index) else {
                if gpu {
                    let why = self
                        .status()
                        .skipped
                        .first()
                        .map_or_else(|| "no confinement level".to_owned(), |(_, why)| why.clone());
                    // The listing or the probe of the GPU failed: a library may explain it.
                    return PathEnd::GpuFailed(self.hinted(path, why));
                }
                let why = self.no_level();
                return PathEnd::Next(self.hold(why).await);
            };
            let generation = self.begin(level);
            match self.attempt(generation, level, path).await {
                Attempt::Ready(child, calibration, budgets) => {
                    let ended = self.ready(generation, &child, *calibration, budgets).await;
                    child.stop().await;
                    if let Some(end) = self.after_ready(generation, ended, gpu) {
                        return end;
                    }
                }
                // The model file no longer has the recorded hash: unavailable until the owner
                // tries a model again, on either path (the CPU would only fail the same way).
                Attempt::Failed(Fail::Other(why)) if why.starts_with(MODEL_CHANGED) => {
                    self.ended(generation, EndReason::Exited);
                    // The device default's 9B is not the pinned file: Granite runs instead,
                    // with the reason on the page. An explicit or stored model is refused.
                    if self.device_default && self.active.sha256 == GPU_DEFAULT_MODEL_SHA256 {
                        log(&why);
                        self.nine_b_mismatch = true;
                        return PathEnd::Next(Next::Fresh { gpu: false });
                    }
                    return PathEnd::Next(self.hold(why).await);
                }
                Attempt::Failed(Fail::Sandbox(why) | Fail::Other(why)) if gpu => {
                    self.ended(generation, EndReason::Exited);
                    return PathEnd::GpuFailed(self.hinted(path, why));
                }
                Attempt::TooSlow(why) if gpu => {
                    self.ended(generation, EndReason::Exited);
                    return PathEnd::GpuFailed(why);
                }
                Attempt::Failed(Fail::Sandbox(why)) => {
                    log(&format!("level {} skipped: {why}", level.number()));
                    self.ended(generation, EndReason::Exited);
                    self.update(|status| status.skipped.push((level, why)));
                    index += 1;
                }
                Attempt::Failed(Fail::Other(why)) => {
                    self.ended(generation, EndReason::Exited);
                    let why = self.hinted(path, why);
                    self.failed(why);
                    if self.failures < MAX_FAILURES {
                        self.update(|status| {
                            status.state = State::Starting("waiting to retry");
                        });
                        match self.wait(Some(RETRY_AFTER)).await {
                            Wake::Timer => {}
                            Wake::Restart { gpu } => return PathEnd::Next(Next::Fresh { gpu }),
                            Wake::Model(command) => return PathEnd::Next(Next::Model(command)),
                            Wake::Shutdown(reply) => return PathEnd::Next(Next::Stop(reply)),
                        }
                    }
                }
                Attempt::TooSlow(why) => return PathEnd::Next(self.hold(why).await),
                Attempt::Interrupted(command) => {
                    let restart = matches!(command, Command::Restart { .. });
                    self.ended(
                        generation,
                        if restart {
                            EndReason::Restart
                        } else {
                            EndReason::Exited
                        },
                    );
                    return PathEnd::Next(Next::from(command));
                }
            }
        }
    }

    /// What follows a ready phase: `None` starts the same level again (a counted exit).
    fn after_ready(&mut self, generation: u64, ended: Ended, gpu: bool) -> Option<PathEnd> {
        match ended {
            Ended::Exited { why, .. } if gpu => {
                self.ended(generation, EndReason::Exited);
                Some(PathEnd::GpuFailed(why))
            }
            Ended::Exited { counted, why } => {
                self.ended(generation, EndReason::Exited);
                if counted {
                    self.failed(why);
                }
                None
            }
            Ended::Restart { gpu } => {
                self.ended(generation, EndReason::Restart);
                Some(PathEnd::Next(Next::Fresh { gpu }))
            }
            Ended::Model(command) => {
                self.ended(generation, EndReason::Restart);
                Some(PathEnd::Next(Next::Model(command)))
            }
            Ended::Shutdown(reply) => {
                self.ended(generation, EndReason::Exited);
                Some(PathEnd::Next(Next::Stop(reply)))
            }
        }
    }

    fn status(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn ended(&self, generation: u64, reason: EndReason) {
        self.assistant
            .event(RuntimeEvent::GenerationEnded { generation, reason });
        self.update(|status| status.budgets = None);
    }

    fn failed(&mut self, why: String) {
        log(&format!("attempt failed (counted): {why}"));
        self.failures += 1;
        let failures = self.failures;
        self.update(|status| {
            status.failures = failures;
            status.last_failure = Some(why);
        });
    }

    /// Start, `/health`, priming and calibration, interruptible by the owner. A failed
    /// attempt's child is gone when this returns.
    async fn attempt(&mut self, generation: u64, level: Level, path: Path) -> Attempt {
        let launcher = Arc::clone(&self.launcher);
        self.health_answered.store(false, Ordering::SeqCst);
        let started = tokio::select! {
            started = launcher.start(generation, level, path) => started,
            command = self.commands.recv() => match command {
                Some(command) => {
                    self.launcher.cancel();
                    self.launcher.settle().await;
                    return Attempt::Interrupted(command);
                }
                None => std::future::pending().await,
            },
        };
        let child = match started {
            Ok(child) => child,
            Err(fail) => return Attempt::Failed(fail),
        };
        let measured = {
            let measure = measure(
                &child,
                path,
                self.active.max_tokens,
                &self.status,
                &self.health_answered,
            );
            tokio::select! {
                result = measure => Some(result),
                fail = child.exited() => Some(Err(Attempt::Failed(fail))),
                command = self.commands.recv() => match command {
                    Some(command) => {
                        child.stop().await;
                        return Attempt::Interrupted(command);
                    }
                    None => None,
                },
            }
        };
        let Some(measured) = measured else {
            child.stop().await;
            return Attempt::Failed(Fail::Other("the runtime manager lost its owner".into()));
        };
        match measured {
            Ok((calibration, budgets)) => Attempt::Ready(child, Box::new(calibration), budgets),
            Err(outcome) => {
                child.stop().await;
                outcome
            }
        }
    }

    /// The ready phase: until the child exits, is stopped as hung, or the owner acts.
    async fn ready(
        &mut self,
        generation: u64,
        child: &Arc<dyn Child>,
        calibration: Calibration,
        budgets: Budgets,
    ) -> Ended {
        let interpreter = child.server().interpreter();
        let notes = child.notes();
        let landlock = child.landlock();
        // Signals from an earlier generation are stale.
        while self.signals.try_recv().is_ok() {}
        self.assistant.event(RuntimeEvent::Ready {
            generation,
            budgets,
            interpreter,
        });
        let level = self.status().level.map_or(0, Level::number);
        log(&format!(
            "ready: generation {generation}, level {level}{}; {}",
            landlock.map_or_else(String::new, |landlock| format!(
                ", landlock {}",
                landlock.describe()
            )),
            measured(&calibration, Some(&budgets))
        ));
        self.update(|status| {
            status.state = State::Ready;
            // A Restart ends here or at `unavailable`: a press meanwhile starts nothing.
            status.restarting = false;
            status.calibration = Some(calibration);
            status.budgets = Some(budgets);
            status.notes = notes;
            status.landlock = landlock;
        });
        let ready_at = Instant::now();
        let mut reset_pending = true;
        let mut expiries = 0;
        let mut closed = false;
        loop {
            let reset = async {
                if reset_pending {
                    tokio::time::sleep_until(ready_at + READY_WINDOW).await;
                } else {
                    std::future::pending::<()>().await;
                }
            };
            tokio::select! {
                fail = child.exited() => {
                    let why = match fail {
                        Fail::Sandbox(why) | Fail::Other(why) => why,
                    };
                    return Ended::Exited { counted: ready_at.elapsed() <= READY_WINDOW, why };
                }
                signal = self.signals.recv() => match signal {
                    Some(Signal::Expired { generation: g }) if g == generation => {
                        expiries += 1;
                        if expiries >= 2 {
                            return Ended::Exited {
                                counted: ready_at.elapsed() <= READY_WINDOW,
                                why: "stopped as hung: two consecutive run deadlines expired".into(),
                            };
                        }
                    }
                    Some(Signal::Completed { generation: g }) if g == generation => expiries = 0,
                    _ => {}
                },
                () = reset => {
                    reset_pending = false;
                    self.failures = 0;
                    self.update(|status| status.failures = 0);
                }
                command = self.commands.recv(), if !closed => match command {
                    Some(Command::Restart { gpu }) => return Ended::Restart { gpu },
                    Some(Command::Model(command)) => return Ended::Model(command),
                    Some(Command::Shutdown(reply)) => return Ended::Shutdown(reply),
                    // Every handle is gone: no command can come again (polling the closed
                    // channel again would spin).
                    None => closed = true,
                },
            }
        }
    }
}

/// `/health` within [`HEALTH_WITHIN`]; a refused endpoint fails at once.
async fn healthy(child: &Arc<dyn Child>) -> Result<(), Attempt> {
    let server = child.server();
    let answered = tokio::time::timeout(HEALTH_WITHIN, async {
        loop {
            match server.health().await {
                Health::Ready => {
                    child.answered_health();
                    return Ok(());
                }
                Health::Refused(why) => return Err(why),
                Health::Loading | Health::NotListening | Health::Failed => {
                    tokio::time::sleep(HEALTH_POLL).await;
                }
            }
        }
    })
    .await;
    match answered {
        Ok(Ok(())) => Ok(()),
        Ok(Err(why)) => Err(Attempt::Failed(Fail::Other(why))),
        Err(_) => Err(Attempt::Failed(Fail::Other(
            "the model server did not answer /health within 60 s".into(),
        ))),
    }
}

/// `/health`, the offload check and priming: the readiness gate. Returns priming's prompt
/// tokens and how long priming took.
async fn warm(
    child: &Arc<dyn Child>,
    phase: &(dyn Fn(&'static str) + Sync),
    health_answered: &AtomicBool,
) -> Result<(f64, Duration), Attempt> {
    let server = child.server();
    let began = Instant::now();
    phase("loading the model");
    healthy(child).await?;
    health_answered.store(true, Ordering::SeqCst);
    log(&format!(
        "health after {:.1} s",
        began.elapsed().as_secs_f64()
    ));
    child
        .loaded()
        .await
        .map_err(|why| Attempt::Failed(Fail::Other(why)))?;
    phase("priming");
    let priming = Instant::now();
    let prefix =
        match tokio::time::timeout(PRIMING_WITHIN, calibration::prime(server.as_ref())).await {
            Ok(Ok(prefix)) => prefix,
            Ok(Err(interrupted)) => {
                return Err(Attempt::Failed(Fail::Other(format!(
                    "priming failed ({})",
                    interrupted.describe()
                ))));
            }
            Err(_) => {
                return Err(Attempt::Failed(Fail::Other(
                    "priming did not finish within 300 s".into(),
                )));
            }
        };
    log(&format!(
        "primed in {:.1} s ({prefix} prompt tokens)",
        priming.elapsed().as_secs_f64()
    ));
    Ok((prefix, priming.elapsed()))
}

/// Why the calibration did not give budgets.
enum NotCalibrated {
    /// Too slow for the path's caps; the measurement, when the samples completed.
    TooSlow(String, Option<Box<Calibration>>),
    Failed(String),
}

/// The two calibration samples at `max_tokens` and the budgets they derive (after [`warm`]).
async fn calibrate_budgets(
    server: &dyn Server,
    path: Path,
    max_tokens: u32,
    prefix: f64,
) -> Result<(Calibration, Budgets), NotCalibrated> {
    // Each sample is held to the run cap; this bounds the whole (two samples, two
    // `/tokenize` calls).
    let within = Duration::from_secs(2 * path.run_cap() + 60);
    let calibration = match tokio::time::timeout(
        within,
        calibration::calibrate(server, path, max_tokens, prefix),
    )
    .await
    {
        Ok(Ok(calibration)) => calibration,
        Ok(Err(calibration::Interrupted::SampleOverCap { cap_s })) => {
            let why = format!(
                "too slow on this machine: a calibration sample took more than {cap_s} s, the cap is {cap_s} s"
            );
            log(&format!("measured: {why}"));
            return Err(NotCalibrated::TooSlow(why, None));
        }
        Ok(Err(interrupted)) => {
            return Err(NotCalibrated::Failed(format!(
                "calibration failed ({})",
                interrupted.describe()
            )));
        }
        Err(_) => {
            return Err(NotCalibrated::Failed(format!(
                "calibration did not finish within {} s",
                within.as_secs()
            )));
        }
    };
    match &calibration.derived {
        Ok(derived) => {
            let budgets = derived.budgets;
            Ok((calibration, budgets))
        }
        Err(NoBudgets::TooSlow { needed_s, cap_s }) => {
            let why = format!(
                "too slow on this machine: about {needed_s} s needed, the cap is {cap_s} s"
            );
            log(&format!("measured: {}", measured(&calibration, None)));
            Err(NotCalibrated::TooSlow(why, Some(Box::new(calibration))))
        }
        Err(NoBudgets::Invalid(why)) => Err(NotCalibrated::Failed((*why).to_owned())),
    }
}

/// `/health`, priming and calibration within their limits.
async fn measure(
    child: &Arc<dyn Child>,
    path: Path,
    max_tokens: u32,
    status: &Arc<Mutex<Status>>,
    health_answered: &AtomicBool,
) -> Result<(Calibration, Budgets), Attempt> {
    let phase = |phase: &'static str| {
        status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .state = State::Starting(phase);
    };
    let (prefix, _) = warm(child, &phase, health_answered).await?;
    phase("calibrating");
    match calibrate_budgets(child.server().as_ref(), path, max_tokens, prefix).await {
        Ok(measured) => Ok(measured),
        Err(NotCalibrated::TooSlow(why, calibration)) => {
            if let Some(calibration) = calibration {
                status
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .calibration = Some(*calibration);
            }
            Err(Attempt::TooSlow(why))
        }
        Err(NotCalibrated::Failed(why)) => Err(Attempt::Failed(Fail::Other(why))),
    }
}

#[path = "manager_model.rs"]
mod model_commands;

#[cfg(test)]
#[path = "manager_tests.rs"]
pub(crate) mod tests;

#[cfg(test)]
#[path = "manager_default_tests.rs"]
mod default_tests;
