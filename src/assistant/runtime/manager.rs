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
use super::server::{Health, Server};
use super::{Level, Path};
use crate::assistant::jobs::{Assistant, Budgets, EndReason, RuntimeEvent, Signal};
use async_trait::async_trait;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
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
}

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
}

impl Status {
    fn new(path: Path) -> Self {
        Self {
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
}

/// What the rest of Mimik holds: the status and the owner's commands.
#[derive(Clone)]
pub(crate) struct Handle {
    status: Arc<Mutex<Status>>,
    commands: mpsc::UnboundedSender<Command>,
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
    pub max_tokens: u32,
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

/// Starts the manager task for `assistant`.
pub(crate) fn spawn(
    assistant: Arc<Assistant>,
    launcher: Arc<dyn Launcher>,
    config: Config,
) -> Handle {
    let status = Arc::new(Mutex::new(Status::new(config.path)));
    let (commands, incoming) = mpsc::unbounded_channel();
    let (signal_sender, signals) = mpsc::unbounded_channel();
    assistant.subscribe(signal_sender);
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
    };
    tokio::spawn(manager.run());
    Handle { status, commands }
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
    Shutdown(oneshot::Sender<()>),
}

/// How a session ended.
enum Next {
    /// Start again from level 1 (the owner's Restart); with `gpu`, from the GPU path again.
    Fresh {
        gpu: bool,
    },
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
    Shutdown(oneshot::Sender<()>),
    Timer,
}

impl From<Command> for Next {
    fn from(command: Command) -> Self {
        match command {
            Command::Restart { gpu } => Next::Fresh { gpu },
            Command::Shutdown(reply) => Next::Stop(reply),
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
        // Mimik's start counts as a Restart: from level 1, breaker and count reset.
        loop {
            match self.session().await {
                Next::Stop(reply) => {
                    self.launcher.cancel();
                    self.launcher.settle().await;
                    return shut(reply);
                }
                Next::Fresh { gpu } => {
                    // Fresh always follows the owner's command: it stays "being carried out"
                    // until the session ends in `ready` or `unavailable`, even if one of them
                    // cleared the flag just before the command was taken.
                    self.update(|status| status.restarting = true);
                    if gpu {
                        self.gpu_failed = None;
                    }
                }
            }
        }
    }

    /// Unavailable with `why` until the owner's Restart or shutdown.
    async fn hold(&mut self, why: String) -> Next {
        self.unavailable(why);
        match self.wait(None).await {
            Wake::Restart { gpu } => Next::Fresh { gpu },
            Wake::Timer => Next::Fresh { gpu: false },
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
    async fn session(&mut self) -> Next {
        // Nothing of an earlier session (a child's watcher, a cancelled probe) runs beside it.
        self.launcher.settle().await;
        self.failures = 0;
        self.assistant.reset_breaker();
        let gpu = self.config.path == Path::Gpu && self.gpu_failed.is_none();
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
                    match ended {
                        Ended::Exited { why, .. } if gpu => {
                            self.ended(generation, EndReason::Exited);
                            return PathEnd::GpuFailed(why);
                        }
                        Ended::Exited { counted, why } => {
                            self.ended(generation, EndReason::Exited);
                            if counted {
                                self.failed(why);
                            }
                        }
                        Ended::Restart { gpu } => {
                            self.ended(generation, EndReason::Restart);
                            return PathEnd::Next(Next::Fresh { gpu });
                        }
                        Ended::Shutdown(reply) => {
                            self.ended(generation, EndReason::Exited);
                            return PathEnd::Next(Next::Stop(reply));
                        }
                    }
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
                self.config.max_tokens,
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
    phase("calibrating");
    // Each sample is held to the run cap; this bounds the whole (two samples, two
    // `/tokenize` calls).
    let within = Duration::from_secs(2 * path.run_cap() + 60);
    let calibration = match tokio::time::timeout(
        within,
        calibration::calibrate(server.as_ref(), path, max_tokens, prefix),
    )
    .await
    {
        Ok(Ok(calibration)) => calibration,
        Ok(Err(calibration::Interrupted::SampleOverCap { cap_s })) => {
            let why = format!(
                "too slow on this machine: a calibration sample took more than {cap_s} s, the cap is {cap_s} s"
            );
            log(&format!("measured: {why}"));
            return Err(Attempt::TooSlow(why));
        }
        Ok(Err(interrupted)) => {
            return Err(Attempt::Failed(Fail::Other(format!(
                "calibration failed ({})",
                interrupted.describe()
            ))));
        }
        Err(_) => {
            return Err(Attempt::Failed(Fail::Other(format!(
                "calibration did not finish within {} s",
                within.as_secs()
            ))));
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
            let calibration = calibration.clone();
            status
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .calibration = Some(calibration);
            Err(Attempt::TooSlow(why))
        }
        Err(NoBudgets::Invalid(why)) => Err(Attempt::Failed(Fail::Other((*why).to_owned()))),
    }
}

#[cfg(test)]
#[path = "manager_tests.rs"]
mod tests;
