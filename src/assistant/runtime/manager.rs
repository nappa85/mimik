//! Generations and transitions (assisted sheet plan §1.4a, "Generations", "Transitions"): one
//! manager task serializes every start and stop, so two model servers never coexist; it
//! feeds the jobs' runtime events (generation started, ended with its reason, ready with the
//! budgets) and never reports a child exit to the breaker.
//!
//! An attempt is: start (spawn and the endpoint check), `/health` within 60 s, priming within
//! 300 s, calibration within twice the sample timeout plus 60 s. On the CPU path:
//!
//! | Event | Action | Failure count |
//! |---|---|---|
//! | Sandbox-attributable failure at a level | next allowed level that passed its probe, at once; with none left (level 1 is the only one Mimik uses, ASSIST-43) `unavailable` with the fix | not counted |
//! | Any other failed attempt | retry after 2 minutes, same level | +1 |
//! | Child exits while ready | restart now | +1 only within 10 minutes of `ready` |
//! | Two consecutive run-deadline expiries | stop as hung, restart, re-measure | as a child exit |
//! | Failure count reaches 4 | `unavailable` until Restart | — |
//! | Budgets do not derive ("too slow", or a reply too large for the context) | `unavailable` with the numbers, no retry; after a reply-length change, back to the previous setting | — |
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
use super::budgets::{self, NoBudgets};
use super::calibration::{self, Calibration};
use super::models::{
    DeviceDefault, Measurement, Model, ModelFile, ModelStatus, NineB, OlderPrompt, TrialKey,
    TrialReport, TrialState, default_model, gpu_default_name, trial_bound,
};
use super::progress::{Kind, Memory, Observer, Phase, Progress, Qualifier, Reporter};
use super::server::{Health, Server};
use super::{Level, Path, trial};
use crate::assistant::jobs::{Assistant, Budgets, EndReason, RuntimeEvent, Signal};
use crate::assistant::model::{GPU_DEFAULT_MODEL_SHA256, reply_length};
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
    /// Registers where the launcher reports a start's verification of the model file (and a
    /// trial's hashing of it): the manager's own reporter. A launcher that reports nothing
    /// ignores it.
    fn observe(&self, _observer: Observer) {}
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
    /// Working towards `ready`: the step and what its time shows.
    Starting(Progress),
    Ready,
    Unavailable(String),
}

impl State {
    /// The progress of work under way, when the runtime is starting.
    pub(crate) fn starting(&self) -> Option<&Progress> {
        match self {
            Self::Starting(progress) => Some(progress),
            _ => None,
        }
    }
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
    /// A model server that is ready is being stopped for the owner's request: the page shows
    /// the stop at once, as the first step of the list the session then walks.
    fn stopping(&mut self) {
        if self.state == State::Ready {
            self.state = State::Starting(Progress::new(
                Kind::Fresh { stopping: true },
                Phase::Stopping,
                None,
                Vec::new(),
            ));
        }
    }

    /// Why the owner can retry the GPU path or its default model file.
    pub(crate) fn gpu_retry_note(&self) -> Option<&str> {
        if !self.gpu_wanted {
            None
        } else if self.path == Path::Cpu {
            Some(
                self.gpu_note
                    .as_deref()
                    .unwrap_or("the GPU path has not been tried yet"),
            )
        } else {
            self.model.device_default.note()
        }
    }

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
            state: State::Starting(Progress::new(
                Kind::Fresh { stopping: false },
                Phase::Checking,
                None,
                Vec::new(),
            )),
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
    Switch {
        key: TrialKey,
        by: i64,
    },
    SwitchBack {
        by: i64,
    },
    /// The owner's reply length for the active model's file (`model::reply_length`), already
    /// stored for the file with `sha256` (the one the handle read as active); `from` is what
    /// was stored for it before (none: the default applied).
    SetLength {
        max_tokens: u32,
        by: i64,
        sha256: String,
        from: Option<u32>,
    },
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
    /// Where reply lengths are stored (the same records the manager holds).
    records: Option<Arc<dyn Records>>,
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
    /// Nothing to try: the GPU is not wanted (`--assistant-device cpu`), or it runs the device
    /// default model without a fallback note.
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

/// What the owner's reply-length setting answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SetLength {
    Accepted,
    /// A trial is running, or the model setting is not supported here.
    Busy,
    /// Another model change or a restart is being carried out: nothing else is started.
    Pending,
    /// Not a setting `model::reply_length` allows (out of bounds or off the step).
    Invalid,
    /// The active model already has this reply length.
    Unchanged,
    /// The setting could not be stored: nothing changed and no server restarts.
    NotStored,
    /// This assistant keeps no model setting (the development endpoint, fixed models).
    Unsupported,
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
        status.stopping();
        drop(status);
        let _ = self.commands.send(Command::Restart { gpu: false });
        Restart::Accepted
    }

    /// The owner's "Try the GPU again": a Restart that starts from the GPU path again. Only
    /// while the GPU is wanted and its path or default model file needs another try.
    pub(crate) fn try_gpu(&self) -> TryGpu {
        let mut status = self
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if status.restarting {
            return TryGpu::AlreadyRunning;
        }
        if status.gpu_retry_note().is_none() {
            return TryGpu::NotApplicable;
        }
        status.restarting = true;
        status.stopping();
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
            phase: Phase::WaitingStop,
        });
        status.state = State::Starting(Progress::new(
            Kind::Trial,
            Phase::WaitingStop,
            None,
            Vec::new(),
        ));
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
        status.stopping();
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
        status.stopping();
        drop(status);
        let _ = self
            .commands
            .send(Command::Model(ModelCommand::SwitchBack { by }));
        Chosen::Accepted
    }

    /// The owner's reply length for the active model's file: stored first (one row per file,
    /// independent of the model rows and of the model choice), then the model server restarts
    /// once and is measured again with it; a first start that fails because of the length goes
    /// back to the file's earlier setting. A store failure changes nothing and restarts
    /// nothing. Admitted like Switch back. The work runs in its own task, so a dropped request
    /// cannot leave the admission flags set.
    pub(crate) async fn set_length(&self, by: i64, max_tokens: u32) -> SetLength {
        let handle = self.clone();
        tokio::spawn(async move { handle.store_length(by, max_tokens).await })
            .await
            .unwrap_or(SetLength::NotStored)
    }

    async fn store_length(&self, by: i64, max_tokens: u32) -> SetLength {
        let Some(records) = self.records.clone() else {
            // Nothing to store into: the model setting is not supported here.
            return SetLength::Unsupported;
        };
        let (sha256, before) = {
            let mut status = self.lock();
            if !status.model.switchable || status.trial.as_ref().is_some_and(TrialState::running) {
                return SetLength::Busy;
            }
            if status.model.pending || status.restarting || status.model.work {
                return SetLength::Pending;
            }
            if !crate::assistant::model::reply_length::valid(max_tokens) {
                return SetLength::Invalid;
            }
            if status.model.active.max_tokens == max_tokens {
                return SetLength::Unchanged;
            }
            status.model.note = None;
            status.model.pending = true;
            status.restarting = true;
            let before = status.state.clone();
            status.stopping();
            (status.model.active.sha256.clone(), before)
        };
        let stored = match records.length(&sha256).await {
            Ok(from) => records
                .set_length(&sha256, Some(max_tokens), by)
                .await
                .map(|()| from),
            Err(why) => Err(why),
        };
        match stored {
            Ok(from) => {
                let _ = self.commands.send(Command::Model(ModelCommand::SetLength {
                    max_tokens,
                    by,
                    sha256,
                    from,
                }));
                SetLength::Accepted
            }
            Err(why) => {
                let mut status = self.lock();
                status.model.pending = false;
                status.restarting = false;
                // Nothing was stopped: the server runs as before.
                if matches!(&status.state, State::Starting(p) if p.phase == Phase::Stopping) {
                    status.state = before;
                }
                status.model.note = Some(format!("{why}; the current setting stays"));
                SetLength::NotStored
            }
        }
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
    spawn_with(
        assistant,
        launcher,
        config,
        ModelSetup::fixed(crate::assistant::model::reply_length::DEFAULT),
    )
}

/// Starts the manager task for `assistant` with `models` as the owner's model setting.
pub(crate) fn spawn_with(
    assistant: Arc<Assistant>,
    launcher: Arc<dyn Launcher>,
    config: Config,
    models: ModelSetup,
) -> Handle {
    let status = Arc::new(Mutex::new(Status::new(config.path, &models)));
    let reporter = Reporter(Arc::clone(&status));
    let observer = reporter.clone();
    launcher.observe(Arc::new(move |event| observer.event(event)));
    let records = models.records.clone();
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
        length_change: None,
        reporter,
        memory: Memory::default(),
        qualifier: None,
        child_ran: false,
    };
    tokio::spawn(manager.run());
    Handle {
        status,
        commands,
        cancel,
        records,
    }
}

/// How an attempt ended.
enum Attempt {
    Ready(Arc<dyn Child>, Box<Calibration>, Budgets),
    Failed(Fail),
    /// Too slow for the path's caps in a way the reply length does not change (a calibration
    /// sample timed out).
    TooSlow(String),
    /// No budgets for the reply length: the estimate is over the ceiling (`too_large` false)
    /// or the reply does not fit the context (`too_large`).
    LengthLimit {
        why: String,
        too_large: bool,
    },
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
    /// Start a session again with the breaker and the GPU choice as they are (a reply length
    /// went back to the file's earlier setting).
    Again,
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

/// What a reply-length change keeps to go back to when its first start fails because of the
/// length.
struct LengthChange {
    by: i64,
    /// The file the length is stored for.
    sha256: String,
    /// What was stored for the file before (none: nothing, the default applied).
    from: Option<u32>,
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
    /// A reply-length change whose first start has not ended yet: where to go back to.
    length_change: Option<LengthChange>,
    /// Whether `/health` answered in the current attempt: from then on a failure (priming,
    /// calibration, the offload check) is not a library that failed to load.
    health_answered: Arc<AtomicBool>,
    /// Writes the steps into the status.
    reporter: Reporter,
    /// How long each step took in the last successful run, by file, path and kind.
    memory: Memory,
    /// Why the next session's list is walked (a reply length that went back).
    qualifier: Option<Qualifier>,
    /// A model server was started by the work that is ending (and stopped, or being stopped,
    /// by it): the next session's list has the stop step only then.
    child_ran: bool,
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
                // (A trial is treated apart, below.)
                let trial = matches!(command, ModelCommand::Trial(_));
                next = self.model_command(command).await;
                self.update(|status| status.model.pending = false);
                if next.is_none() && !trial {
                    // The session that follows a Switch, Switch back or length change is part
                    // of the request: it stays "being carried out" until `ready` or
                    // `unavailable`, even if `ready` cleared the flag while the request was
                    // being stored. A trial's result is the owner's to act on while the
                    // previous model starts again, so it is left out.
                    self.update(|status| status.restarting = true);
                }
            }
            reset_breaker = false;
            match next {
                Some(Next::Stop(reply)) => {
                    self.launcher.cancel();
                    self.launcher.settle().await;
                    return shut(reply);
                }
                Some(Next::Fresh { gpu }) => {
                    self.length_change = None;
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
                Some(Next::Model(_) | Next::Again) | None => {}
            }
        }
    }

    /// Unavailable with `why` until the owner's Restart or shutdown.
    async fn hold(&mut self, why: String) -> Next {
        self.child_ran = false;
        self.length_change = None;
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

    /// A new generation at `level`. `first`: the session's first attempt, which goes on with
    /// the list it began (a fresh session's, a trial's); any later attempt walks the retry
    /// list, labelled with the counted attempt it is.
    fn begin(&mut self, level: Level, first: bool) -> u64 {
        // The earlier attempt's child, if any, is gone.
        self.child_ran = false;
        self.generation += 1;
        let generation = self.generation;
        log(&format!(
            "starting: generation {generation}, level {}",
            level.number()
        ));
        self.assistant
            .event(RuntimeEvent::GenerationStarted { generation });
        let path = self.status().path;
        // Only counted failures make an attempt a numbered one.
        let attempt = (self.failures > 0).then(|| Qualifier::Attempt(self.failures + 1));
        self.update(|status| {
            status.level = Some(level);
            status.generation = generation;
            status.notes.clear();
            status.landlock = None;
            match &mut status.state {
                // The trial's own steps are set as it goes.
                State::Starting(progress) if progress.kind == Kind::Trial => {}
                State::Starting(progress)
                    if first && matches!(progress.kind, Kind::Fresh { .. }) =>
                {
                    progress.enter(Phase::StartingServer);
                }
                state => {
                    let reference = self.memory.recall(&self.active.sha256, path, Kind::Retry);
                    *state = State::Starting(Progress::new(
                        Kind::Retry,
                        Phase::StartingServer,
                        attempt,
                        reference,
                    ));
                }
            }
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
            // The reply length is the file's own, whichever model the device picks.
            let model = self.with_stored_length(model).await;
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

    /// `model` with the reply length stored for its file (the default when there is none).
    /// A storage failure keeps the length the model has.
    pub(super) async fn with_stored_length(&self, mut model: Model) -> Model {
        let Some(records) = &self.records else {
            return model;
        };
        match records.length(&model.sha256).await {
            Ok(Some(tokens)) => model.max_tokens = tokens,
            Ok(None) => model.max_tokens = crate::assistant::model::reply_length::DEFAULT,
            Err(why) => log(&format!("{why}; using {}", model.max_tokens)),
        }
        model
    }

    /// The GPU path failed: the CPU path is used from now on, until Mimik restarts or the
    /// owner presses "Try the GPU again".
    fn gpu_fallback(&mut self, why: String) {
        log(&format!("GPU path not used, on the CPU path: {why}"));
        self.gpu_failed = Some(why.clone());
        self.update(|status| {
            status.gpu_note = Some(why);
            // The GPU child's state is over: the CPU server starts now (the session sets
            // the steps once the model is chosen).
            status.path = Path::Cpu;
            status.device = None;
            status.device_notes.clear();
            status.budgets = None;
        });
    }

    /// The steps of the session about to start, set before anything of it runs (the wait for
    /// the earlier work to settle included). The list has the stop step when a model server
    /// may have run (the owner stopped a ready or starting one, or a trial just ended) and
    /// not otherwise; a session with a reason of its own (the CPU after a GPU failure, a
    /// reverted length) follows a stop that already ended and has none.
    fn session_steps(&mut self, path: Path) {
        let qualifier = self.qualifier.take();
        let ran = std::mem::take(&mut self.child_ran);
        let stopping = qualifier.is_none()
            && {
                let status = self
                    .status
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // A child ran (a start that failed before it spawned stopped nothing), or the
                // stop of a ready one was already announced.
                ran || matches!(
                    &status.state,
                    State::Starting(p) if p.kind == (Kind::Fresh { stopping: true }) && p.phase == Phase::Stopping
                )
            };
        let kind = Kind::Fresh { stopping };
        let reference = self.memory.recall(&self.active.sha256, path, kind);
        self.update(|status| match &mut status.state {
            // The stop `stopping()` showed goes on: same start, same list.
            State::Starting(progress)
                if stopping && progress.kind == kind && progress.phase == Phase::Stopping =>
            {
                progress.qualifier = None;
                progress.set_reference(reference);
            }
            state => {
                let phase = if stopping {
                    Phase::Stopping
                } else {
                    Phase::Checking
                };
                *state = State::Starting(Progress::new(kind, phase, qualifier, reference));
            }
        });
    }

    /// The model the session runs is chosen: the last time's steps are those of its file.
    fn refresh_reference(&self, path: Path) {
        self.update(|status| {
            if let State::Starting(progress) = &mut status.state {
                let reference = self.memory.recall(&self.active.sha256, path, progress.kind);
                progress.set_reference(reference);
            }
        });
    }

    /// A start or a model server is being stopped for the owner's command: the page says so
    /// at once.
    fn mark_stopping(&self) {
        self.update(|status| {
            if let State::Starting(progress) = &status.state
                && matches!(progress.kind, Kind::Fresh { .. } | Kind::Retry)
                && !matches!(progress.phase, Phase::Checking | Phase::Stopping)
            {
                status.state = State::Starting(Progress::new(
                    Kind::Fresh { stopping: true },
                    Phase::Stopping,
                    None,
                    Vec::new(),
                ));
            }
        });
    }

    /// One start from level 1 until the owner's Restart (or shutdown) ends it: the GPU path
    /// first when it is wanted and has not failed, the CPU path after a GPU failure.
    async fn session(&mut self, reset_breaker: bool) -> Next {
        let gpu = self.config.path == Path::Gpu && self.gpu_failed.is_none();
        let first_path = if gpu { Path::Gpu } else { Path::Cpu };
        self.session_steps(first_path);
        // Nothing of an earlier session (a child's watcher, a cancelled probe) runs beside it.
        self.launcher.settle().await;
        self.reporter.phase(Phase::Checking);
        self.select_model(gpu).await;
        self.refresh_reference(first_path);
        self.failures = 0;
        if reset_breaker {
            self.assistant.reset_breaker();
        }
        let note = self.gpu_failed.clone();
        self.update(|status| {
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
        let mut path = first_path;
        loop {
            let levels = match self.levels(path).await {
                Ok(levels) => levels,
                Err(next) => return next,
            };
            match self.on_path(path, &levels).await {
                PathEnd::Next(next) => return next,
                PathEnd::GpuFailed(why) => {
                    self.gpu_fallback(why);
                    // The steps first: choosing the model may wait on the records.
                    self.qualifier = Some(Qualifier::GpuFallback);
                    self.session_steps(Path::Cpu);
                    // The device default is the CPU's now (Granite), not the 9B that failed.
                    self.select_model(false).await;
                    self.refresh_reference(Path::Cpu);
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
        let mut first = true;
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
            let generation = self.begin(level, first);
            first = false;
            let attempt = self.attempt(generation, level, path).await;
            let attempt = match self.revert_length(generation, attempt).await {
                Ok(attempt) => attempt,
                Err(next) => return PathEnd::Next(next),
            };
            match attempt {
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
                Attempt::LengthLimit { why, too_large } => {
                    return self.length_limit(generation, gpu, why, too_large).await;
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
                    if self.failures < MAX_FAILURES
                        && let Some(next) = self.wait_to_retry().await
                    {
                        return PathEnd::Next(next);
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

    /// Waits [`RETRY_AFTER`] for the next counted attempt, shown as its own step with the
    /// exact time left; `Some` when the owner's command (or shutdown) ends the wait instead.
    async fn wait_to_retry(&mut self) -> Option<Next> {
        // The failed attempt's child is long gone when the owner acts.
        self.child_ran = false;
        let next = Qualifier::Attempt(self.failures + 1);
        self.update(|status| {
            status.state = State::Starting(Progress::new(
                Kind::Waiting,
                Phase::WaitingRetry,
                Some(next),
                Vec::new(),
            ));
        });
        match self.wait(Some(RETRY_AFTER)).await {
            Wake::Timer => None,
            Wake::Restart { gpu } => Some(Next::Fresh { gpu }),
            Wake::Model(command) => Some(Next::Model(command)),
            Wake::Shutdown(reply) => Some(Next::Stop(reply)),
        }
    }

    /// A start whose budgets fail for the reply length (and no length change reverted it). A
    /// reply too large for the context is not the GPU's failure: no other path changes what
    /// the reply needs, so it is held as unavailable with its message. One over the ceiling
    /// is the GPU path's failure (the CPU path next) and is held on the CPU path.
    async fn length_limit(
        &mut self,
        generation: u64,
        gpu: bool,
        why: String,
        too_large: bool,
    ) -> PathEnd {
        if gpu && !too_large {
            self.ended(generation, EndReason::Exited);
            return PathEnd::GpuFailed(why);
        }
        PathEnd::Next(self.hold(why).await)
    }

    /// After the first start of a reply-length change: a start whose budgets fail because of
    /// the length (the estimate over the ceiling, or the reply too large for the context) goes
    /// back to the file's earlier setting (stored first) and the session starts again, without
    /// resetting the breaker. Every other outcome (ready, interrupted, a sandbox or other
    /// failure, a model-file change, a calibration sample timeout) follows its normal path and
    /// keeps the new setting.
    async fn revert_length(&mut self, generation: u64, attempt: Attempt) -> Result<Attempt, Next> {
        let Some(change) = self.length_change.take() else {
            return Ok(attempt);
        };
        let Attempt::LengthLimit { why, .. } = &attempt else {
            return Ok(attempt);
        };
        if self.active.sha256 != change.sha256 {
            return Ok(attempt);
        }
        let why = why.clone();
        self.ended(generation, EndReason::Exited);
        self.qualifier = Some(Qualifier::EarlierLength);
        let Some(records) = self.records.clone() else {
            return Err(Next::Again);
        };
        let tried = self.active.max_tokens;
        let back = change.from.unwrap_or(reply_length::DEFAULT);
        match records.set_length(&change.sha256, change.from, change.by).await {
            Ok(()) => {
                self.set_active_length(back);
                self.note(format!(
                    "a reply length of {tried} tokens was not used: {why}; the length is back to {back} tokens"
                ));
            }
            Err(stored) => self.note(format!(
                "a reply length of {tried} tokens was not used: {why}; the earlier setting could not be restored ({stored})"
            )),
        }
        Err(Next::Again)
    }

    /// Gives the file of the active model (and the previous model, when it is the same file)
    /// the reply length `tokens`, here and on the page. Nothing else about the choice changes.
    pub(super) fn set_active_length(&mut self, tokens: u32) {
        let sha256 = self.active.sha256.clone();
        self.active.max_tokens = tokens;
        if let Some(previous) = self.previous.as_mut().filter(|p| p.sha256 == sha256) {
            previous.max_tokens = tokens;
        }
        self.update(|status| {
            status.model.active.max_tokens = tokens;
            if let Some(previous) = status
                .model
                .previous
                .as_mut()
                .filter(|p| p.sha256 == sha256)
            {
                previous.max_tokens = tokens;
            }
        });
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
                    // Nothing was spawned yet: only the start is being stopped.
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
        self.child_ran = true;
        let measured = {
            let measure = measure(
                &child,
                path,
                self.active.max_tokens,
                &self.reporter,
                &self.health_answered,
            );
            tokio::select! {
                result = measure => Some(result),
                fail = child.exited() => Some(Err(Attempt::Failed(fail))),
                command = self.commands.recv() => match command {
                    Some(command) => {
                        self.mark_stopping();
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
        // How long each step took is kept for the same file, path and kind next time (a
        // failed attempt's partial times never reach here).
        let (path, finished) = {
            let status = self.status();
            let finished = match &status.state {
                State::Starting(progress)
                    if matches!(progress.kind, Kind::Fresh { .. } | Kind::Retry) =>
                {
                    Some((progress.kind, progress.finished()))
                }
                _ => None,
            };
            (status.path, finished)
        };
        if let Some((kind, steps)) = finished {
            let sha256 = self.active.sha256.clone();
            self.memory.keep(&sha256, path, kind, steps);
        }
        // An owner command already waiting ends this generation at once: the state stays
        // as it is (the stop or the trial it shows) rather than flip to ready and back.
        let command_waiting = !self.commands.is_empty();
        self.update(|status| {
            if !command_waiting {
                status.state = State::Ready;
                // A Restart ends here or at `unavailable`: a press meanwhile starts nothing.
                status.restarting = false;
            }
            status.calibration = Some(calibration);
            status.budgets = Some(budgets);
            status.notes = notes;
            status.landlock = landlock;
        });
        if command_waiting {
            self.mark_stopping();
        }
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
    reporter: &Reporter,
    health_answered: &AtomicBool,
    path: Path,
) -> Result<(f64, Duration), Attempt> {
    let server = child.server();
    let began = Instant::now();
    // `/health`, then what `loaded()` allows the child: its readers to settle and, on the GPU
    // path, the offload line.
    let loading = HEALTH_WITHIN
        + super::launch::READERS_WITHIN
        + if path == Path::Gpu {
            super::launch::OFFLOAD_WITHIN
        } else {
            Duration::ZERO
        };
    reporter.phase_within(Phase::Loading, loading);
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
    reporter.phase(Phase::Priming);
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
    /// No budgets for the reply length: over the ceiling, or (`too_large`) too large for the
    /// context. The measurement is kept.
    Length {
        why: String,
        too_large: bool,
        calibration: Box<Calibration>,
    },
    Failed(String),
}

/// The two calibration samples and the budgets they derive for replies of up to `max_tokens`
/// (after [`warm`]).
async fn calibrate_budgets(
    server: &dyn Server,
    path: Path,
    max_tokens: u32,
    prefix: f64,
    progress: &(dyn Fn(u8) + Sync),
) -> Result<(Calibration, Budgets), NotCalibrated> {
    // Each sample is held to its timeout; this bounds the whole (two samples, two
    // `/tokenize` calls).
    let within = Duration::from_secs(2 * budgets::SAMPLE_TIMEOUT_S + 60);
    let calibration = match tokio::time::timeout(
        within,
        calibration::calibrate(server, path, max_tokens, prefix, progress),
    )
    .await
    {
        Ok(Ok(calibration)) => calibration,
        Ok(Err(calibration::Interrupted::SampleTimedOut { limit_s })) => {
            let why = format!(
                "too slow on this machine: a calibration sample took more than {limit_s} s, so a reply could take longer than the {} s ceiling",
                budgets::RUN_CEILING_S
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
                "too slow on this machine: about {needed_s} s needed, the ceiling is {cap_s} s"
            );
            log(&format!("measured: {}", measured(&calibration, None)));
            Err(NotCalibrated::Length {
                why,
                too_large: false,
                calibration: Box::new(calibration),
            })
        }
        Err(NoBudgets::ReplyTooLarge) => {
            let why = format!(
                "reply length too large for the context: no request bound leaves room in {} tokens for the fixed prefix, a reply of {max_tokens} tokens and the template margin; choose a shorter reply length",
                super::launch::CTX_SIZE
            );
            log(&format!("measured: {why}"));
            Err(NotCalibrated::Length {
                why,
                too_large: true,
                calibration: Box::new(calibration),
            })
        }
        Err(NoBudgets::Invalid(why)) => Err(NotCalibrated::Failed((*why).to_owned())),
    }
}

/// `/health`, priming and calibration within their limits.
async fn measure(
    child: &Arc<dyn Child>,
    path: Path,
    max_tokens: u32,
    reporter: &Reporter,
    health_answered: &AtomicBool,
) -> Result<(Calibration, Budgets), Attempt> {
    let status = &reporter.0;
    let (prefix, _) = warm(child, reporter, health_answered, path).await?;
    let sample = |n: u8| reporter.phase(Phase::Sample(n));
    match calibrate_budgets(child.server().as_ref(), path, max_tokens, prefix, &sample).await {
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
        Err(NotCalibrated::Length {
            why,
            too_large,
            calibration,
        }) => {
            status
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .calibration = Some(*calibration);
            Err(Attempt::LengthLimit { why, too_large })
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

#[cfg(test)]
#[path = "manager_progress_tests.rs"]
mod progress_tests;
