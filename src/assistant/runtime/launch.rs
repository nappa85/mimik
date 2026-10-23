//! The real launcher (assisted sheet plan §1.4a): verified files, the generation's
//! directory and token, the runner with the level's command, the endpoint check's connector,
//! the output drain and the child's lifetime. Level 1 runs llama-server under bubblewrap on
//! a Unix socket; level 3 executes it unconfined but for seccomp on `127.0.0.1`. Level 2
//! (Landlock) is step 2c-2b.
use super::bwrap::{self, Bwrap, Profile, Started};
use super::drain::{self, Summary};
use super::endpoint::{self, Loopback, Sandbox, SandboxSocket, Watched};
use super::files::{self, HashCache, Owners, Verified};
use super::manager::{Child, Fail, Launcher};
use super::manifest::{Build, MANIFEST};
use super::rundir::{self, Entry, Generation, Pidfile, Swept};
use super::runner::{self, Exec, Handover, RunnerCommand, Spec};
use super::seccomp;
use super::server::{Llama, Server};
use super::token::{self, Token};
use super::{Level, Path};
use crate::assistant::transport::{Bounded, Connect};
use async_trait::async_trait;
use std::io::{BufRead, Read};
use std::os::fd::{AsFd, OwnedFd, RawFd};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

/// `--ctx-size` (plan §1.4a, "Flag vector"; measurement-relevant).
pub(crate) const CTX_SIZE: u32 = 8192;
/// How long bwrap may take to report the sandbox's PID.
const STATUS_WITHIN: Duration = Duration::from_secs(30);
/// SIGTERM, then SIGKILL after this long.
const STOP_GRACE: Duration = Duration::from_secs(5);
/// Ports tried per attempt on the TCP levels.
const PORT_TRIES: usize = 3;
/// Free memory a model must leave (`MemAvailable` minus this, ASSIST-L23).
const MEMORY_MARGIN: u64 = 1 << 30;

/// The descriptor numbers the child sees.
mod fds {
    use std::os::fd::RawFd;
    pub const STATUS: RawFd = 4;
    pub const SECCOMP: RawFd = 5;
    pub const TOKEN: RawFd = 6;
    pub const SOCKET_DIR: RawFd = 7;
    pub const MODEL: RawFd = 8;
    /// The executable on the TCP levels (moved to `runner::EXEC_FD` by the runner).
    pub const SERVER: RawFd = 9;
    /// Mimik's own executable at level 1: the shim (and the probe).
    pub const SHIM: RawFd = 10;
    /// The shim's marker pipe at level 1.
    pub const MARKER: RawFd = 12;
    /// The first runtime file at level 1.
    pub const RUNTIME: RawFd = 16;
    /// Level 1's descriptors besides the runtime files: status, seccomp, token, socket
    /// directory, model, shim and marker.
    #[cfg(test)]
    pub const FIXED: usize = 7;
}

/// Mimik's own binary inside the levels: the probe (in place of llama-server) and, at level
/// 1, the shim that marks the exec (the test binary in tests).
#[derive(Clone, Debug)]
pub(crate) struct InnerCommand {
    pub executable: PathBuf,
    /// The probe's arguments after argv[0], before the level's own.
    pub probe_args: Vec<String>,
    /// Variables the probe gets beside the minimal environment (none in production).
    pub probe_env: Vec<(String, String)>,
    /// The shim's arguments after argv[0].
    pub shim_args: Vec<String>,
    /// Variables the shim gets beside its spec (none in production).
    pub shim_env: Vec<(String, String)>,
}

impl InnerCommand {
    pub(crate) fn mimik() -> std::io::Result<Self> {
        Ok(Self {
            executable: std::env::current_exe()?,
            probe_args: vec!["assistant-runner".into(), "--probe-inner".into()],
            probe_env: Vec::new(),
            shim_args: vec!["assistant-runner".into(), "--shim".into()],
            shim_env: Vec::new(),
        })
    }
}

/// The level-1 sandbox's own executable path for Mimik's binary.
const SHIM_PATH: &str = "/opt/mimik/mimik";

/// The minimal environment inside the sandbox.
fn sandbox_env() -> Vec<(String, String)> {
    vec![
        ("PATH".into(), "/usr/bin:/bin".into()),
        ("LANG".into(), "C.UTF-8".into()),
        ("HOME".into(), "/tmp".into()),
    ]
}

/// What the launcher needs.
#[derive(Clone, Debug)]
pub(crate) struct Settings {
    pub data_dir: PathBuf,
    pub models_dir: PathBuf,
    pub runtime_dir: PathBuf,
    /// The model's file name in the models folder.
    pub model: String,
    pub model_sha256: String,
    /// `--alias` and the requests' model name.
    pub alias: String,
    pub threads: u32,
    pub ctx_size: u32,
    pub runner: RunnerCommand,
    pub inner: InnerCommand,
}

#[derive(Default)]
struct Held {
    run: Option<Arc<OwnedFd>>,
    /// The leftover sweep of this Mimik's start is done.
    swept: bool,
    /// The sweep runs (it holds the pidfile meanwhile).
    sweeping: bool,
    bwrap: Option<Bwrap>,
}

/// What one attempt verified: the pinned files and the model, opened and hashed beneath the
/// runtime folder's descriptor, and the build folder's real path as checked (level 3's
/// executable and libraries are found through it, never through the configured path).
pub(crate) struct Checked {
    pub files: Vec<Verified>,
    pub model: Verified,
    pub build_dir: PathBuf,
}

/// Level 3's server: its verified file and the checked folder its libraries come from.
#[derive(Clone, Copy)]
pub(crate) struct ServerFile<'a> {
    pub file: &'a Verified,
    pub build_dir: &'a std::path::Path,
}

/// The real launcher.
pub(crate) struct Real {
    settings: Settings,
    /// The pinned CPU build (the embedded manifest's; tests give another).
    build: Option<Build>,
    cache: HashCache,
    held: Arc<Mutex<Held>>,
    /// The locked pidfile, under its own lock: its writes never hold `held`.
    pidfile: PidSlot,
    /// Probes and starts in flight (their blocking work and watchers).
    work: Arc<Work>,
    /// Probe sequence numbers: unique directory names and pidfile keys.
    probes: AtomicU64,
    /// Failed attempts' cleanups still running: no new process is spawned until they end.
    cleanups: Arc<watch::Sender<usize>>,
}

/// Pidfile keys of probes: above any generation number.
const PROBE_KEYS: u64 = 1 << 40;

/// The launcher's in-flight work: every probe or start's blocking part and every child's
/// watcher holds a [`Ticket`]. A cancel invalidates the tickets taken before it (their work
/// stops at its next check, before spawning or recording anything); `settle` waits until no
/// ticket is held, so nothing of an earlier session runs beside the next.
pub(crate) struct Work {
    epoch: AtomicU64,
    running: watch::Sender<usize>,
}

impl Work {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            epoch: AtomicU64::new(0),
            running: watch::channel(0).0,
        })
    }

    pub(crate) fn enter(self: &Arc<Self>) -> Ticket {
        self.running.send_modify(|running| *running += 1);
        Ticket {
            epoch: self.epoch.load(Ordering::SeqCst),
            work: Arc::clone(self),
        }
    }

    pub(crate) fn cancel(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) async fn settle(&self) {
        let mut running = self.running.subscribe();
        let _ = running.wait_for(|running| *running == 0).await;
    }
}

/// One holder of in-flight work (see [`Work`]).
pub(crate) struct Ticket {
    epoch: u64,
    work: Arc<Work>,
}

impl Ticket {
    pub(crate) fn cancelled(&self) -> bool {
        self.work.epoch.load(Ordering::SeqCst) != self.epoch
    }
}

impl Clone for Ticket {
    fn clone(&self) -> Self {
        self.work.running.send_modify(|running| *running += 1);
        Self {
            epoch: self.epoch,
            work: Arc::clone(&self.work),
        }
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.work
            .running
            .send_modify(|running| *running = running.saturating_sub(1));
    }
}

/// A probe or start: its generation directory's name, its pidfile key and its ticket.
pub(crate) struct Job {
    name: String,
    key: u64,
    ticket: Ticket,
}

impl Job {
    /// Fails once the work was cancelled.
    fn check(&self) -> Result<(), Fail> {
        if self.ticket.cancelled() {
            Err(Fail::Other("cancelled".into()))
        } else {
            Ok(())
        }
    }
}

impl Real {
    /// The launcher of the embedded manifest's build.
    pub(crate) fn new(settings: Settings) -> Self {
        Self::with_build(settings, MANIFEST.build(Path::Cpu).cloned())
    }

    pub(crate) fn with_build(settings: Settings, build: Option<Build>) -> Self {
        Self {
            settings,
            build,
            cache: HashCache::default(),
            held: Arc::new(Mutex::new(Held::default())),
            pidfile: Arc::new(Mutex::new(None)),
            work: Work::new(),
            probes: AtomicU64::new(1),
            cleanups: Arc::new(watch::channel(0).0),
        }
    }

    fn build(&self, path: Path) -> Option<&Build> {
        // The CPU path is the only one built in 2c-2a.
        (path == Path::Cpu).then_some(self.build.as_ref()).flatten()
    }

    fn held(&self) -> std::sync::MutexGuard<'_, Held> {
        self.held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn run_dir(&self) -> Result<Arc<OwnedFd>, String> {
        self.held()
            .run
            .clone()
            .ok_or_else(|| "the run directory is not open".to_owned())
    }

    /// Records the child; a failure fails the start (an unrecorded child could outlive a
    /// crash of Mimik unseen).
    fn record(&self, entry: &Entry) -> Result<(), Fail> {
        #[cfg(test)]
        if RECORD_FAILS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_deref()
            == Some(self.settings.runtime_dir.as_path())
        {
            return Err(Fail::Other("the pidfile cannot be written".into()));
        }
        self.pidfile
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
            .ok_or_else(|| Fail::Other("the pidfile is not held".into()))?
            .record(entry)
            .map_err(|_| Fail::Other("the pidfile cannot be written".into()))
    }

    /// Waits until no earlier attempt's cleanup runs (its process may still be ending, its
    /// pidfile entry still standing): each spawn follows the previous one's end. A cancel
    /// ends the wait.
    fn after_cleanups(&self, job: &Job) -> Result<(), Fail> {
        let mut warned = false;
        let started = std::time::Instant::now();
        while *self.cleanups.borrow() > 0 {
            job.check()?;
            if !warned && started.elapsed() >= REAPER_WITHIN {
                eprintln!("assistant: waiting for an earlier start's processes to end");
                warned = true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    /// Clears the pidfile's entry when it is still `key`'s (never a newer one's).
    pub(crate) fn clear(&self, key: u64) {
        clear_key(&self.pidfile, key);
    }

    /// The launcher's in-flight work.
    pub(crate) fn work(&self) -> &Arc<Work> {
        &self.work
    }

    /// The model and the build's files, verified.
    fn verify(&self, path: Path, level: Level, ticket: &Ticket) -> Result<Checked, String> {
        let stop = || ticket.cancelled();
        let build = self
            .build(path)
            .ok_or_else(|| "this build has no model server for this path".to_owned())?;
        // Resolved once for this attempt: what is checked is what runs.
        let (runtime, real) =
            files::runtime_folder(&self.settings.runtime_dir, level.tcp(), || {
                #[cfg(test)]
                self.paused("runtime-opened");
            })
            .map_err(|refusal| format!("the model runtime: {refusal}"))?;
        let files = files::runtime_until(runtime.as_fd(), build, &self.cache, &stop)
            .map_err(|refusal| format!("the model runtime: {refusal}"))?;
        let models = files::open_dir(&self.settings.models_dir, Owners::Mine, true)
            .map_err(|refusal| format!("the models folder: {refusal}"))?;
        let model = files::model_until(
            models.as_fd(),
            &self.settings.model,
            &self.settings.model_sha256,
            &self.cache,
            &stop,
        )
        .map_err(|refusal| format!("the model: {refusal}"))?;
        Ok(Checked {
            files,
            model,
            build_dir: real.join(&build.directory),
        })
    }

    /// The llama-server flag vector (plan §1.4a, "Flag vector"), CPU path, for `host`.
    fn server_args(&self, model: &str, host: &str, port: Option<u16>, token: &str) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "--model".into(),
            model.into(),
            "--alias".into(),
            self.settings.alias.clone(),
            "--host".into(),
            host.into(),
        ];
        if let Some(port) = port {
            args.extend(["--port".into(), port.to_string()]);
        }
        args.extend(
            [
                "--api-key-file",
                token,
                "--offline",
                "--no-webui",
                "--no-slots",
                "--no-mmproj",
                "--jinja",
                "--n-gpu-layers",
                "0",
                "--fit",
                "off",
            ]
            .map(String::from),
        );
        let threads = self.settings.threads.to_string();
        args.extend([
            "--threads".into(),
            threads.clone(),
            "--threads-batch".into(),
            threads,
        ]);
        args.extend(
            [
                "--ctx-size",
                &self.settings.ctx_size.to_string(),
                "--parallel",
                "1",
                "--no-warmup",
                "--cache-ram",
                "0",
                "--reasoning",
                "off",
            ]
            .map(String::from),
        );
        args
    }
}

fn dup(fd: &impl AsFd) -> Result<OwnedFd, String> {
    fd.as_fd()
        .try_clone_to_owned()
        .map_err(|_| "a descriptor cannot be duplicated".to_owned())
}

fn bytes_pipe(bytes: &[u8]) -> Result<OwnedFd, String> {
    let (read, write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)
        .map_err(|_| "no pipe".to_owned())?;
    let mut writer = std::fs::File::from(write);
    std::io::Write::write_all(&mut writer, bytes).map_err(|_| "the pipe is full".to_owned())?;
    Ok(read)
}

fn mem_available() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = text
        .lines()
        .find(|line| line.starts_with("MemAvailable:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    kib.checked_mul(1024)
}

/// One spawned runner and what came with it.
struct Spawned {
    /// Killed and reaped if this is dropped before its watcher takes it.
    child: Owned,
    runner: Watched,
    /// Level 1: bwrap's report and the reaper.
    sandbox: Option<Arc<Sandbox>>,
    generation: Generation,
    connector: Arc<dyn Connect>,
    status_rest: Option<std::io::BufReader<std::fs::File>>,
    /// Level 1: the read end of the shim's marker pipe.
    marker: Option<OwnedFd>,
    /// The start's ticket, released last (after every field above is cleaned up), so an
    /// abandoned result still counts until then.
    ticket: Ticket,
}

/// The pidfile's slot: `None` until the first preflight takes the lock.
type PidSlot = Arc<Mutex<Option<Pidfile>>>;

/// Test-forced record failures, keyed by runtime directory (`launch_tests.rs`).
#[cfg(test)]
pub(crate) static RECORD_FAILS: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Clears the pidfile's entry when it is `key`'s.
fn clear_key(slot: &Mutex<Option<Pidfile>>, key: u64) {
    let mut slot = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(pidfile) = slot.as_mut()
        && pidfile
            .leftover()
            .is_some_and(|entry| entry.generation == key)
    {
        let _ = pidfile.clear();
    }
}

/// Kills and reaps a spawned runner on every failure path until it is handed to its watcher,
/// so no child outlives a failed start: then, on level 1, the sandbox's reaper is killed and
/// waited for too, and the pidfile entry it recorded is cleared. The waiting runs on a
/// cleanup thread holding the start's ticket, without a deadline: until the exit is
/// confirmed the entry stays and a settle (so the next session) waits.
struct Owned {
    child: Option<std::process::Child>,
    reaper: Option<Arc<Sandbox>>,
    recorded: Option<(PidSlot, u64)>,
    ticket: Option<Ticket>,
    /// The launcher's count of running cleanups (the next spawn waits for them).
    cleanups: Option<Arc<watch::Sender<usize>>>,
    /// The launcher's runtime directory (a test's pause key).
    #[cfg(test)]
    dir: PathBuf,
    /// A test's reaper that the kill does not reach.
    #[cfg(test)]
    unkillable: bool,
}

/// After this long without the reaper's exit, the cleanup says so in the log (and waits on).
const REAPER_WITHIN: Duration = Duration::from_secs(5);

impl Owned {
    fn new(child: std::process::Child, ticket: Ticket) -> Self {
        Self {
            child: Some(child),
            reaper: None,
            recorded: None,
            ticket: Some(ticket),
            cleanups: None,
            #[cfg(test)]
            dir: PathBuf::new(),
            #[cfg(test)]
            unkillable: false,
        }
    }

    fn pid(&self) -> i32 {
        self.child
            .as_ref()
            .map_or(0, |child| i32::try_from(child.id()).unwrap_or(0))
    }

    /// The runner and the start's ticket, for its watcher (which then owns the reaper's end
    /// and the entry).
    fn hand_over(mut self) -> (std::process::Child, Ticket) {
        self.reaper = None;
        self.recorded = None;
        let child = self.child.take().expect("handed over once");
        let ticket = self.ticket.take().expect("handed over once");
        (child, ticket)
    }
}

/// What a dropped [`Owned`] still has to do.
struct Cleanup {
    child: Option<std::process::Child>,
    reaper: Option<Arc<Sandbox>>,
    recorded: Option<(PidSlot, u64)>,
    ticket: Option<Ticket>,
    cleanups: Option<Arc<watch::Sender<usize>>>,
    #[cfg(test)]
    dir: PathBuf,
    #[cfg(test)]
    unkillable: bool,
}

impl Cleanup {
    fn run(mut self) {
        #[cfg(test)]
        pause_point(&self.dir, "cleanup");
        #[cfg(test)]
        let kill_reaper = !self.unkillable;
        #[cfg(not(test))]
        let kill_reaper = true;
        if let Some(mut child) = self.child.take() {
            // Killed already; its exit is waited for through a pidfd, so a slow one is said.
            if let Ok(runner) = Watched::open(i32::try_from(child.id()).unwrap_or(0)) {
                wait_exited(&|| runner.alive(), &|| {}, REAPER_WITHIN, &mut || {
                    eprintln!("assistant: a failed start's runner has not exited yet; waiting");
                });
            }
            let _ = child.wait();
        }
        if let Some(sandbox) = self.reaper.take() {
            let reaper = &sandbox.reaper;
            wait_exited(
                &|| reaper.alive(),
                &|| {
                    if kill_reaper {
                        let _ = rustix::process::pidfd_send_signal(
                            &reaper.pidfd,
                            rustix::process::Signal::KILL,
                        );
                    }
                },
                REAPER_WITHIN,
                &mut || {
                    eprintln!("assistant: a failed start's sandbox has not exited yet; waiting");
                },
            );
        }
        if let Some((held, key)) = self.recorded.take() {
            clear_key(&held, key);
        }
        if let Some(cleanups) = self.cleanups.take() {
            cleanups.send_modify(|running| *running = running.saturating_sub(1));
        }
        drop(self.ticket.take());
    }
}

/// Waits, without a deadline, until `alive` says no, calling `kill` each round; `warn` once
/// after `warn_after`.
pub(crate) fn wait_exited(
    alive: &dyn Fn() -> bool,
    kill: &dyn Fn(),
    warn_after: Duration,
    warn: &mut dyn FnMut(),
) {
    let started = std::time::Instant::now();
    let mut warned = false;
    while alive() {
        kill();
        if !warned && started.elapsed() >= warn_after {
            warn();
            warned = true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
        }
        let cleanup = Cleanup {
            child: self.child.take(),
            reaper: self.reaper.take(),
            recorded: self.recorded.take(),
            ticket: self.ticket.take(),
            cleanups: self.cleanups.take(),
            #[cfg(test)]
            dir: std::mem::take(&mut self.dir),
            #[cfg(test)]
            unkillable: self.unkillable,
        };
        if cleanup.child.is_none() && cleanup.reaper.is_none() && cleanup.recorded.is_none() {
            return;
        }
        if let Some(cleanups) = &cleanup.cleanups {
            cleanups.send_modify(|running| *running += 1);
        }
        // Off the caller's thread (often a tokio worker); inline only if no thread starts.
        let (sender, receiver) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("assistant-cleanup".into())
            .spawn(move || {
                if let Ok(cleanup) = receiver.recv() {
                    Cleanup::run(cleanup);
                }
            });
        match spawned {
            Ok(_) => {
                if let Err(unsent) = sender.send(cleanup) {
                    unsent.0.run();
                }
            }
            Err(_) => cleanup.run(),
        }
    }
}

/// Holds a test's pause point (see [`PAUSE`]).
#[cfg(test)]
pub(crate) struct Pause {
    pub dir: PathBuf,
    pub step: &'static str,
    pub arrived: std::sync::mpsc::Sender<()>,
    pub release: std::sync::mpsc::Receiver<()>,
}

/// A test's pause inside the blocking start or probe, keyed by runtime directory.
#[cfg(test)]
pub(crate) static PAUSE: Mutex<Vec<Pause>> = Mutex::new(Vec::new());

/// Blocks at `step` while a test holds a pause there for `dir`.
#[cfg(test)]
fn pause_point(dir: &std::path::Path, step: &'static str) {
    let pause = {
        let mut pauses = PAUSE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pauses
            .iter()
            .position(|pause| pause.dir == dir && pause.step == step)
            .map(|at| pauses.remove(at))
    };
    if let Some(pause) = pause {
        let _ = pause.arrived.send(());
        let _ = pause.release.recv();
    }
}

/// Blocks at `step` while a test holds it there; nothing outside tests.
macro_rules! pause_at {
    ($real:expr, $step:literal) => {
        #[cfg(test)]
        $real.paused($step);
    };
}

/// Test-injected failures after the spawn, keyed by the runtime directory of the launcher
/// they apply to (`launch_tests.rs`); none outside tests.
#[cfg(test)]
pub(crate) static FAULT: Mutex<Option<(PathBuf, &'static str)>> = Mutex::new(None);
/// The runner's PID when the injected failure fired.
#[cfg(test)]
pub(crate) static FAULTED: Mutex<Option<i32>> = Mutex::new(None);

/// Fails the start at `step` when a test injected a failure there; nothing outside tests.
macro_rules! fault_at {
    ($real:expr, $step:literal, $child:expr) => {
        #[cfg(test)]
        if $real.faulted($step, $child.pid()) {
            return Err(Fail::Other(format!("injected at {}", $step)));
        }
    };
}

#[cfg(test)]
impl Real {
    fn paused(&self, step: &'static str) {
        pause_point(&self.settings.runtime_dir, step);
    }

    fn faulted(&self, step: &'static str, pid: i32) -> bool {
        {
            let mut fault = FAULT
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if fault
                .as_ref()
                .is_some_and(|(dir, at)| *dir == self.settings.runtime_dir && *at == step)
            {
                *fault = None;
                *FAULTED
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pid);
                return true;
            }
        }
        false
    }
}

/// Returns the pidfile and the sweep's outcome to `held` when the sweep ends, however it ends.
struct SweepGuard {
    held: Arc<Mutex<Held>>,
    slot: PidSlot,
    pidfile: Option<Pidfile>,
    swept: bool,
    _ticket: Ticket,
}

impl Drop for SweepGuard {
    fn drop(&mut self) {
        *self
            .slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = self.pidfile.take();
        let mut held = self
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        held.sweeping = false;
        held.swept = self.swept;
    }
}

/// Test-forced panics in the sweep, keyed by data directory (`launch_tests.rs`).
#[cfg(test)]
pub(crate) static SWEEP_PANIC: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Test-forced pending sweeps, keyed by data directory (`launch_tests.rs`).
#[cfg(test)]
pub(crate) static SWEEP_PENDING: Mutex<Option<PathBuf>> = Mutex::new(None);

/// The start's leftover sweep (a test may force the killed leftover to look still running).
fn sweep_leftover(pidfile: &mut Pidfile, data_dir: &std::path::Path) -> Swept {
    #[cfg(test)]
    assert!(
        SWEEP_PANIC
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_deref()
            != Some(data_dir),
        "a test's panic in the sweep"
    );
    #[cfg(test)]
    if SWEEP_PENDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_deref()
        == Some(data_dir)
    {
        return pidfile.sweep_with(|| {}, |_| false);
    }
    let _ = data_dir;
    pidfile.sweep()
}

/// The shim's arguments and environment.
type ShimCommand = (Vec<String>, Vec<(String, String)>);

/// The command run in the child: llama-server, or the probe.
enum Command {
    Server,
    Probe,
}

impl Real {
    /// The sandbox's command: the shim (Mimik's own binary, bound by descriptor), which marks
    /// the exec and executes llama-server from the bound runtime or the probe.
    fn sandbox_command(
        &self,
        command: &Command,
        handover: &mut Vec<Handover>,
        extra: &mut Vec<(RawFd, String)>,
    ) -> Result<ShimCommand, Fail> {
        let socket = format!("{}/{}", bwrap::SOCKET_DIR, endpoint::SOCKET);
        let exe = rustix::fs::open(
            &self.settings.inner.executable,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(|_| Fail::Other("Mimik's own binary cannot be opened".into()))?;
        handover.push(Handover {
            fd: exe,
            target: fds::SHIM,
        });
        extra.push((fds::SHIM, SHIM_PATH.to_owned()));
        let mut env = sandbox_env();
        let argv = match command {
            Command::Server => {
                let server = self
                    .build
                    .as_ref()
                    .map_or("llama-server", |build| build.server.as_str());
                let mut argv = vec![format!("{}/{server}", bwrap::RUNTIME)];
                argv.extend(self.server_args(bwrap::MODEL, &socket, None, bwrap::TOKEN));
                argv
            }
            Command::Probe => {
                env.extend(self.settings.inner.probe_env.iter().cloned());
                let mut argv = vec![SHIM_PATH.to_owned()];
                argv.extend(self.settings.inner.probe_args.iter().cloned());
                argv.push("1".to_owned());
                argv
            }
        };
        let spec = super::probe::ShimSpec {
            marker: fds::MARKER,
            argv,
            env,
        };
        let mut shim_env = self.settings.inner.shim_env.clone();
        shim_env.push((
            super::probe::SHIM_VARIABLE.to_owned(),
            serde_json::to_string(&spec).map_err(|_| Fail::Other("no shim spec".into()))?,
        ));
        let mut program = vec![SHIM_PATH.to_owned()];
        program.extend(self.settings.inner.shim_args.iter().cloned());
        Ok((program, shim_env))
    }

    /// The runner's spec for bwrap with the full profile around the shim.
    fn sandbox_spec(
        &self,
        bwrap: &Bwrap,
        command: &Command,
        handover: &mut Vec<Handover>,
        runtime: Binds,
    ) -> Result<Spec, Fail> {
        let mut extra = Vec::new();
        let (program, env) = self.sandbox_command(command, handover, &mut extra)?;
        let profile = Profile {
            size: bwrap.size,
            status_fd: fds::STATUS,
            seccomp_fd: fds::SECCOMP,
            token_fd: fds::TOKEN,
            socket_dir_fd: fds::SOCKET_DIR,
            model_fd: fds::MODEL,
            runtime,
            extra,
            env,
            command: program,
        }
        .arguments();
        Ok(Spec {
            parent: std::process::id(),
            keep: handover.iter().map(|h| h.target).collect(),
            env: vec![
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("LANG".into(), "C.UTF-8".into()),
            ],
            exec: Exec::Bwrap {
                path: bwrap.path.display().to_string(),
                args: profile
                    .iter()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect(),
            },
        })
    }

    /// Starts level 1: bwrap with the profile, then reads `child-pid` and `pid-namespace`.
    fn spawn_sandbox(
        &self,
        job: &Job,
        files: &[Verified],
        model: &Verified,
        token: &Token,
        command: &Command,
    ) -> Result<Spawned, Fail> {
        let bwrap = self
            .held()
            .bwrap
            .clone()
            .ok_or_else(|| Fail::Sandbox("bubblewrap did not pass the preflight".into()))?;
        let run = self.run_dir().map_err(Fail::Other)?;
        let generation = Generation::create(&run, &job.name)
            .map_err(|refusal| Fail::Other(format!("the socket directory: {refusal}")))?;
        let seccomp = bytes_pipe(&seccomp::bytes(&seccomp::program(
            seccomp::Profile::Sandbox,
        )))
        .map_err(Fail::Other)?;
        let (status_read, status_write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)
            .map_err(|_| Fail::Other("no pipe".into()))?;
        let token_pipe = token.pipe().map_err(|_| Fail::Other("no pipe".into()))?;
        let (marker_read, marker_write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)
            .map_err(|_| Fail::Other("no pipe".into()))?;
        let (mut handover, runtime) =
            sandbox_handover(status_write, seccomp, token_pipe, &generation, model, files)?;
        handover.push(Handover {
            fd: marker_write,
            target: fds::MARKER,
        });
        let spec = self.sandbox_spec(&bwrap, command, &mut handover, runtime)?;
        self.after_cleanups(job)?;
        pause_at!(self, "before-spawn");
        job.check()?;
        let mut child = Owned::new(
            runner::launch(&self.settings.runner, &spec, &handover)
                .map_err(|_| Fail::Other("the runner could not be started".into()))?,
            job.ticket.clone(),
        );
        child.cleanups = Some(Arc::clone(&self.cleanups));
        #[cfg(test)]
        {
            child.dir.clone_from(&self.settings.runtime_dir);
        }
        drop(handover);
        fault_at!(self, "spawned", child);
        let pid = child.pid();
        let runner = Watched::open(pid).map_err(|_| Fail::Other("no pidfd".into()))?;
        pause_at!(self, "before-record");
        job.check()?;
        if let Some(entry) = Entry::of(
            job.key,
            pid,
            vec![
                bwrap.path.display().to_string(),
                self.settings.runner.program.display().to_string(),
            ],
        ) {
            self.record(&entry)?;
            child.recorded = Some((Arc::clone(&self.pidfile), job.key));
        }
        fault_at!(self, "recorded", child);
        // No status line: bwrap failed before it created the sandbox (nothing it started could
        // have written anything yet).
        let (
            Started {
                child_pid,
                pid_namespace,
            },
            rest,
        ) = read_started(status_read, job)?;
        fault_at!(self, "started", child);
        let reaper = endpoint::open_reaper(child_pid, pid, pid_namespace)
            .map_err(|error| Fail::Other(format!("the sandbox's reaper: {error}")))?;
        let sandbox = Arc::new(Sandbox {
            reaper,
            pid_namespace,
        });
        child.reaper = Some(Arc::clone(&sandbox));
        let connector: Arc<dyn Connect> = Arc::new(SandboxSocket {
            directory: Arc::new(dup(&generation.fd).map_err(Fail::Other)?),
            sandbox: Arc::clone(&sandbox),
        });
        fault_at!(self, "connector", child);
        pause_at!(self, "spawned");
        Ok(Spawned {
            child,
            runner,
            sandbox: Some(sandbox),
            generation,
            connector,
            status_rest: Some(rest),
            marker: Some(marker_read),
            ticket: job.ticket.clone(),
        })
    }

    /// Level 3's exact environment.
    fn unconfined_env(
        &self,
        command: &Command,
        server: Option<ServerFile<'_>>,
        directory: &std::path::Path,
    ) -> Vec<(String, String)> {
        let mut env = vec![
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
            (
                "HOME".to_owned(),
                directory.join("home").display().to_string(),
            ),
        ];
        if matches!(command, Command::Probe) {
            env.extend(self.settings.inner.probe_env.iter().cloned());
        }
        if let Some(server) = server {
            // The checked real path: the configured one is never resolved again.
            env.push((
                "LD_LIBRARY_PATH".to_owned(),
                server.build_dir.display().to_string(),
            ));
        }
        env
    }

    /// Level 3's executable (llama-server or Mimik's own binary) and its arguments.
    fn unconfined_command(
        &self,
        command: &Command,
        server: Option<ServerFile<'_>>,
        model_path: &str,
        port: u16,
        token_file: &str,
    ) -> Result<(OwnedFd, Vec<String>), Fail> {
        match command {
            Command::Server => {
                let server = server.ok_or_else(|| Fail::Other("no llama-server".into()))?;
                let mut argv = vec!["llama-server".to_owned()];
                argv.extend(self.server_args(model_path, "127.0.0.1", Some(port), token_file));
                Ok((dup(&server.file.fd).map_err(Fail::Other)?, argv))
            }
            Command::Probe => {
                let exe = rustix::fs::open(
                    &self.settings.inner.executable,
                    rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
                    rustix::fs::Mode::empty(),
                )
                .map_err(|_| Fail::Other("Mimik's own binary cannot be opened".into()))?;
                let mut argv = vec!["mimik".to_owned()];
                argv.extend(self.settings.inner.probe_args.iter().cloned());
                argv.extend(["3".to_owned(), port.to_string(), token_file.to_owned()]);
                Ok((exe, argv))
            }
        }
    }

    /// Starts level 3: the verified executable with seccomp, on `127.0.0.1:<port>`.
    fn spawn_unconfined(
        &self,
        job: &Job,
        server: Option<ServerFile<'_>>,
        model: &Verified,
        token: &Token,
        command: &Command,
        port: u16,
    ) -> Result<Spawned, Fail> {
        let run = self.run_dir().map_err(Fail::Other)?;
        let generation = Generation::create(&run, &job.name)
            .map_err(|refusal| Fail::Other(format!("the generation directory: {refusal}")))?;
        token
            .write_file(generation.fd.as_fd())
            .map_err(|_| Fail::Other("the token file cannot be written".into()))?;
        rustix::fs::mkdirat(
            &generation.fd,
            "home",
            rustix::fs::Mode::from_raw_mode(0o700),
        )
        .map_err(|_| Fail::Other("no private home".into()))?;
        let directory = self.settings.data_dir.join("run").join(generation.name());
        let token_file = directory.join(token::FILE).display().to_string();
        let model_path = format!("/proc/self/fd/{}", fds::MODEL);
        let (executable, argv) =
            self.unconfined_command(command, server, &model_path, port, &token_file)?;
        let handover = vec![
            Handover {
                fd: dup(&model.fd).map_err(Fail::Other)?,
                target: fds::MODEL,
            },
            Handover {
                fd: executable,
                target: fds::SERVER,
            },
        ];
        let env = self.unconfined_env(command, server, &directory);
        let spec = Spec {
            parent: std::process::id(),
            keep: vec![fds::MODEL],
            env,
            exec: Exec::Server {
                fd: fds::SERVER,
                argv,
            },
        };
        self.after_cleanups(job)?;
        pause_at!(self, "before-spawn");
        job.check()?;
        let mut child = Owned::new(
            runner::launch(&self.settings.runner, &spec, &handover)
                .map_err(|_| Fail::Other("the runner could not be started".into()))?,
            job.ticket.clone(),
        );
        child.cleanups = Some(Arc::clone(&self.cleanups));
        #[cfg(test)]
        {
            child.dir.clone_from(&self.settings.runtime_dir);
        }
        drop(handover);
        fault_at!(self, "spawned", child);
        let pid = child.pid();
        let runner = Watched::open(pid).map_err(|_| Fail::Other("no pidfd".into()))?;
        let mut executables = vec![self.settings.runner.program.display().to_string()];
        if let Some(server) = server {
            executables.push(
                server
                    .build_dir
                    .join(&server.file.name)
                    .display()
                    .to_string(),
            );
        }
        pause_at!(self, "before-record");
        job.check()?;
        if let Some(entry) = Entry::of(job.key, pid, executables) {
            self.record(&entry)?;
            child.recorded = Some((Arc::clone(&self.pidfile), job.key));
        }
        fault_at!(self, "recorded", child);
        let connector: Arc<dyn Connect> = Arc::new(Loopback {
            port,
            child: Arc::new(Watched::open(pid).map_err(|_| Fail::Other("no pidfd".into()))?),
        });
        fault_at!(self, "connector", child);
        pause_at!(self, "spawned");
        Ok(Spawned {
            child,
            runner,
            sandbox: None,
            generation,
            connector,
            status_rest: None,
            marker: None,
            ticket: job.ticket.clone(),
        })
    }
}

/// Runtime files bound into the sandbox: descriptor number and name.
type Binds = Vec<(RawFd, String)>;

/// Level 1's descriptors at their numbers, and the runtime files' binds.
fn sandbox_handover(
    status: OwnedFd,
    seccomp: OwnedFd,
    token: OwnedFd,
    generation: &Generation,
    model: &Verified,
    files: &[Verified],
) -> Result<(Vec<Handover>, Binds), Fail> {
    let mut handover = vec![
        Handover {
            fd: status,
            target: fds::STATUS,
        },
        Handover {
            fd: seccomp,
            target: fds::SECCOMP,
        },
        Handover {
            fd: token,
            target: fds::TOKEN,
        },
        Handover {
            fd: dup(&generation.fd).map_err(Fail::Other)?,
            target: fds::SOCKET_DIR,
        },
        Handover {
            fd: dup(&model.fd).map_err(Fail::Other)?,
            target: fds::MODEL,
        },
    ];
    let mut runtime = Vec::new();
    for (n, file) in files.iter().enumerate() {
        let target = fds::RUNTIME + RawFd::try_from(n).unwrap_or(0);
        handover.push(Handover {
            fd: dup(&file.fd).map_err(Fail::Other)?,
            target,
        });
        runtime.push((target, file.name.clone()));
    }
    Ok((handover, runtime))
}

/// bwrap's first status line (the reaper's PID and the PID namespace), within
/// [`STATUS_WITHIN`], and the rest of the descriptor.
fn read_started(
    status: OwnedFd,
    job: &Job,
) -> Result<(Started, std::io::BufReader<std::fs::File>), Fail> {
    let file = std::fs::File::from(status);
    let (lines, report) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("assistant-status".into())
        .spawn(move || {
            let mut reader = std::io::BufReader::new(file.take(64 * 1024));
            let mut line = String::new();
            let started = match reader.read_line(&mut line) {
                Ok(n) if n > 0 => bwrap::started(&line),
                _ => None,
            };
            let rest = std::io::BufReader::new(reader.into_inner().into_inner());
            let _ = lines.send((started, rest));
        })
        .map_err(|_| Fail::Other("no status thread".into()))?;
    // A timeout proves nothing about the sandbox (a slow host): counted, the level kept.
    let (started, rest) =
        recv_unless_cancelled(&report, STATUS_WITHIN, &job.ticket)?.ok_or_else(|| {
            Fail::Other(format!(
                "bubblewrap did not report the sandbox within {} s",
                STATUS_WITHIN.as_secs()
            ))
        })?;
    let started = started.ok_or_else(|| {
        Fail::Sandbox("bubblewrap failed before starting the model server".into())
    })?;
    Ok((started, rest))
}

/// Waits up to `within` for `receiver`, in short slices so a cancel ends the wait: `None` on
/// a timeout (or a sender gone), a failure once cancelled.
fn recv_unless_cancelled<T>(
    receiver: &std::sync::mpsc::Receiver<T>,
    within: Duration,
    ticket: &Ticket,
) -> Result<Option<T>, Fail> {
    let deadline = std::time::Instant::now() + within;
    loop {
        if ticket.cancelled() {
            return Err(Fail::Other("cancelled".into()));
        }
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Ok(None);
        }
        match receiver.recv_timeout(left.min(Duration::from_millis(100))) {
            Ok(value) => return Ok(Some(value)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(None),
        }
    }
}

/// Ports a test hands out first, keyed by runtime directory (`launch_tests.rs`).
#[cfg(test)]
pub(crate) static PORTS: Mutex<Vec<(PathBuf, u16)>> = Mutex::new(Vec::new());

/// A port for a level-3 child of the launcher at `runtime_dir`: a fresh free one each time
/// (a test may hand one out).
fn pick_port(runtime_dir: &std::path::Path) -> Result<u16, Fail> {
    #[cfg(test)]
    {
        let mut ports = PORTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(at) = ports.iter().position(|(dir, _)| dir == runtime_dir) {
            return Ok(ports.remove(at).1);
        }
    }
    let _ = runtime_dir;
    free_port()
}

/// A free loopback port: bound, read, released.
fn free_port() -> Result<u16, Fail> {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .map_err(|_| Fail::Other("no free loopback port".into()))?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|_| Fail::Other("no free loopback port".into()))
}

/// How a child ended, as its watcher saw it.
#[derive(Clone, Debug)]
struct Exit {
    why: String,
}

/// The reader threads of a child (stdout, stderr, the marker): `exited` classifies only
/// once they have all ended (or after a bounded wait).
struct Readers {
    left: AtomicUsize,
    ended: tokio::sync::Notify,
}

impl Readers {
    fn new(count: usize) -> Arc<Self> {
        Arc::new(Self {
            left: AtomicUsize::new(count),
            ended: tokio::sync::Notify::new(),
        })
    }

    fn end(&self) {
        if self.left.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.ended.notify_waiters();
        }
    }

    /// Whether they all ended within `within`.
    async fn all_ended(&self, within: Duration) -> bool {
        tokio::time::timeout(within, async {
            loop {
                let ended = self.ended.notified();
                if self.left.load(Ordering::SeqCst) == 0 {
                    return;
                }
                ended.await;
            }
        })
        .await
        .is_ok()
    }
}

/// What the shim's marker pipe showed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Marker {
    /// The marker arrived: the command was executed.
    Executed,
    /// A clean end of the pipe without the marker: the shim never ran (bwrap failed first).
    Absent,
    /// Not known (still being read, a read error, anything else): the level is kept.
    Unknown,
}

/// The marker pipe's outcome, set once by its reader.
struct MarkerState(std::sync::atomic::AtomicU8);

impl MarkerState {
    fn new() -> Arc<Self> {
        Arc::new(Self(std::sync::atomic::AtomicU8::new(0)))
    }

    fn set(&self, marker: Marker) {
        let value = match marker {
            Marker::Executed => 1,
            Marker::Absent => 2,
            Marker::Unknown => 3,
        };
        self.0.store(value, Ordering::SeqCst);
    }

    /// The outcome so far; a reader still running is `Unknown`.
    fn get(&self) -> Marker {
        match self.0.load(Ordering::SeqCst) {
            1 => Marker::Executed,
            2 => Marker::Absent,
            _ => Marker::Unknown,
        }
    }
}

/// Reads the marker pipe to its end (64 bytes at most).
fn marker_outcome(mut pipe: impl Read) -> Marker {
    let mut text = Vec::new();
    match pipe.by_ref().take(64).read_to_end(&mut text) {
        Ok(_) if text.starts_with(super::probe::MARKER) => Marker::Executed,
        Ok(0) => Marker::Absent,
        _ => Marker::Unknown,
    }
}

/// The marker once the readers ended, or `Unknown` when they did not within `within`.
async fn settled_marker(readers: &Readers, marker: &MarkerState, within: Duration) -> Marker {
    if readers.all_ended(within).await {
        marker.get()
    } else {
        Marker::Unknown
    }
}

/// How long `exited` waits for the readers after the exit.
const READERS_WITHIN: Duration = Duration::from_secs(5);

/// Why a child ended: at level 1, a sandbox failure only when the marker pipe ended cleanly
/// without the marker (nothing the executed command prints can make it one); an unknown
/// outcome, like any other exit, is counted at the same level.
fn classify(level: Level, marker: Marker, summary: Summary, why: &str) -> Fail {
    if level == Level::Bubblewrap && marker == Marker::Absent {
        Fail::Sandbox(format!("bubblewrap could not set up the sandbox ({why})"))
    } else if summary.failed.model_load {
        Fail::Other(format!("the model could not be loaded ({why})"))
    } else {
        Fail::Other(format!("the model server exited ({why})"))
    }
}

/// A running generation's child.
pub(crate) struct RealChild {
    server: Arc<dyn Server>,
    /// The checked connector (live tests reach the server through it with other tokens).
    #[cfg(test)]
    connector: Arc<dyn Connect>,
    /// The token (live tests send short requests without the fixed prefix).
    #[cfg(test)]
    token: String,
    exit: watch::Receiver<Option<Exit>>,
    pid: i32,
    /// The runner's pidfd: signals go to this process and no other.
    signal: OwnedFd,
    summary: Arc<Mutex<Summary>>,
    /// Level 1: what the shim's marker pipe showed.
    marker: Arc<MarkerState>,
    readers: Arc<Readers>,
    healthy: AtomicBool,
    generation_dir: Mutex<Option<Generation>>,
    level: Level,
    notes: Vec<String>,
}

impl RealChild {
    fn kill(&self, signal: rustix::process::Signal) {
        let _ = rustix::process::pidfd_send_signal(&self.signal, signal);
    }

    fn summary(&self) -> Summary {
        *self
            .summary
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Drop for RealChild {
    fn drop(&mut self) {
        // A child nobody holds any more (an interrupted start) does not outlive it; the watcher
        // still reaps it.
        if self.exit.borrow().is_none() {
            self.kill(rustix::process::Signal::KILL);
        }
    }
}

#[async_trait]
impl Child for RealChild {
    fn server(&self) -> Arc<dyn Server> {
        Arc::clone(&self.server)
    }

    async fn exited(&self) -> Fail {
        let mut exit = self.exit.clone();
        let why = loop {
            if let Some(exit) = exit.borrow_and_update().clone() {
                break exit.why;
            }
            if exit.changed().await.is_err() {
                return std::future::pending().await;
            }
        };
        let marker = settled_marker(&self.readers, &self.marker, READERS_WITHIN).await;
        classify(self.level, marker, self.summary(), &why)
    }

    async fn stop(&self) {
        let mut exit = self.exit.clone();
        let running = exit.borrow().is_none();
        if running {
            self.kill(rustix::process::Signal::TERM);
            let gone = tokio::time::timeout(STOP_GRACE, exit.wait_for(Option::is_some))
                .await
                .is_ok_and(|result| result.is_ok());
            if !gone {
                self.kill(rustix::process::Signal::KILL);
                // Gone only when the watcher confirmed it: until then this blocks, so no new
                // generation starts beside a sandbox that has not exited.
                let _ = exit.wait_for(Option::is_some).await;
            }
        }
        self.generation_dir
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }

    fn answered_health(&self) {
        self.healthy.store(true, Ordering::SeqCst);
        if self.level.tcp()
            && let Some(dir) = &*self
                .generation_dir
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            token::remove_file(&dir.fd);
        }
    }

    fn notes(&self) -> Vec<String> {
        self.notes.clone()
    }
}

/// Waits until `pidfd` reports its process gone: every `grace` without it, `kill` is called
/// again and the wait goes on (there is no giving up: "gone" is never assumed).
/// Waits, without any deadline or signal, until the pidfd's process has exited.
async fn exited(pidfd: &OwnedFd) {
    let async_fd = pidfd
        .try_clone()
        .ok()
        .and_then(|fd| tokio::io::unix::AsyncFd::new(fd).ok());
    while !endpoint::exited(pidfd) {
        match &async_fd {
            Some(fd) => match fd.readable().await {
                Ok(mut guard) => guard.clear_ready(),
                Err(_) => tokio::time::sleep(Duration::from_millis(200)).await,
            },
            None => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
}

async fn until_gone(pidfd: &OwnedFd, grace: Duration, kill: impl Fn()) {
    let async_fd = pidfd
        .try_clone()
        .ok()
        .and_then(|fd| tokio::io::unix::AsyncFd::new(fd).ok());
    let mut warned = false;
    loop {
        if endpoint::exited(pidfd) {
            return;
        }
        match &async_fd {
            Some(fd) => {
                let _ = tokio::time::timeout(grace, fd.readable()).await;
            }
            None => tokio::time::sleep(grace.min(Duration::from_millis(200))).await,
        }
        if endpoint::exited(pidfd) {
            return;
        }
        if !warned {
            eprintln!("assistant: a model server process has not exited yet; waiting");
            warned = true;
        }
        kill();
    }
}

/// Waits for the runner's exit (its pidfd), reaps it, and on level 1 for the sandbox's
/// reaper too (killed through its pidfd until gone): exit is published only once the whole
/// sandbox is confirmed gone.
async fn watch_exit(
    mut child: std::process::Child,
    runner: Watched,
    sandbox: Option<Arc<Sandbox>>,
    exit: watch::Sender<Option<Exit>>,
    on_gone: impl FnOnce() + Send + 'static,
) {
    // However long it runs: stopping it is `stop`'s job, not the watcher's.
    exited(&runner.pidfd).await;
    // The pidfd shows it exited, so this returns at once.
    let status = child.wait();
    if let Some(sandbox) = sandbox {
        let reaper = &sandbox.reaper.pidfd;
        if !endpoint::exited(reaper) {
            let _ = rustix::process::pidfd_send_signal(reaper, rustix::process::Signal::KILL);
        }
        until_gone(reaper, STOP_GRACE, || {
            let _ = rustix::process::pidfd_send_signal(reaper, rustix::process::Signal::KILL);
        })
        .await;
    }
    on_gone();
    let why = match status {
        Ok(status) => status.to_string(),
        Err(_) => "unknown status".into(),
    };
    let _ = exit.send(Some(Exit { why }));
}

/// Reads the shim's marker pipe to its end on its own thread. Without a reader thread the
/// outcome stays unknown (a failure then keeps the level).
fn read_marker(pipe: OwnedFd, marker: &Arc<MarkerState>, readers: &Arc<Readers>) {
    let (seen, ended) = (Arc::clone(marker), Arc::clone(readers));
    let spawned = std::thread::Builder::new()
        .name("assistant-marker".into())
        .spawn(move || {
            seen.set(marker_outcome(std::fs::File::from(pipe)));
            ended.end();
        });
    if spawned.is_err() {
        marker.set(Marker::Unknown);
        readers.end();
    }
}

impl Real {
    fn child(
        self: &Arc<Self>,
        spawned: Spawned,
        level: Level,
        token: &Token,
        bwrap: Option<&Bwrap>,
        key: u64,
    ) -> Result<Arc<RealChild>, Fail> {
        let Spawned {
            child,
            runner,
            sandbox,
            generation,
            connector,
            status_rest,
            marker,
            ticket: _spawned,
        } = spawned;
        // Signals need their own handle on the runner: without one the child could not be
        // stopped, so the start fails (and the guard kills it).
        let signal = runner.pidfd.try_clone();
        #[cfg(test)]
        let signal = if self.faulted("signal", child.pid()) {
            Err(std::io::Error::other("injected"))
        } else {
            signal
        };
        let signal =
            signal.map_err(|_| Fail::Other("no pidfd to signal the model server".into()))?;
        let (mut child, ticket) = child.hand_over();
        let pid = i32::try_from(child.id()).unwrap_or(0);
        let summary = Arc::new(Mutex::new(Summary::default()));
        let marker_state = MarkerState::new();
        let readers = Readers::new(2 + usize::from(marker.is_some()));
        for (name, stream) in [
            (
                "stdout",
                child
                    .stdout
                    .take()
                    .map(|s| Box::new(s) as Box<dyn Read + Send>),
            ),
            (
                "stderr",
                child
                    .stderr
                    .take()
                    .map(|s| Box::new(s) as Box<dyn Read + Send>),
            ),
        ] {
            let ended = Arc::clone(&readers);
            let spawned =
                stream.map(|stream| drain::spawn(name, stream, &summary, move || ended.end()));
            if !matches!(spawned, Some(Ok(_))) {
                readers.end();
            }
        }
        if let Some(marker) = marker {
            read_marker(marker, &marker_state, &readers);
        }
        if let Some(rest) = status_rest {
            // Later status lines (`exit-code`) are read and dropped: exit is seen by pidfd.
            let _ = std::thread::Builder::new()
                .name("assistant-status".into())
                .spawn(move || {
                    let _ = std::io::copy(&mut rest.take(64 * 1024), &mut std::io::sink());
                });
        }
        let (exit_sender, exit) = watch::channel(None);
        let launcher = Arc::clone(self);
        // The watcher is in-flight work until the child is gone (a settle waits for it): it
        // keeps the start's ticket.
        tokio::spawn(watch_exit(child, runner, sandbox, exit_sender, move || {
            launcher.clear(key);
            drop(ticket);
        }));
        let server: Arc<dyn Server> = Arc::new(Llama::new(
            Bounded::new(Arc::clone(&connector)),
            token.expose(),
            &self.settings.alias,
        ));
        let mut notes = Vec::new();
        if level == Level::Bubblewrap && bwrap.is_some_and(|bwrap| !bwrap.size) {
            notes.push(
                "temporary space unbounded: this bubblewrap cannot size /tmp and /dev/shm, so only the 64 MiB per-file limit holds"
                    .into(),
            );
        }
        #[cfg(not(test))]
        drop(connector);
        Ok(Arc::new(RealChild {
            server,
            #[cfg(test)]
            connector,
            #[cfg(test)]
            token: token.expose().to_owned(),
            exit,
            pid,
            signal,
            summary,
            marker: marker_state,
            readers,
            healthy: AtomicBool::new(false),
            generation_dir: Mutex::new(Some(generation)),
            level,
            notes,
        }))
    }
}

/// The real launcher behind an `Arc`, so its children can clear the pidfile.
pub(crate) struct Launch(pub Arc<Real>);

#[async_trait]
impl Launcher for Launch {
    async fn preflight(&self, path: Path) -> Result<(), String> {
        let real = &self.0;
        std::fs::create_dir_all(&real.settings.data_dir)
            .map_err(|_| "the data directory cannot be created".to_owned())?;
        let run = rundir::open(&real.settings.data_dir)
            .map_err(|refusal| format!("the run directory: {refusal}"))?;
        let run = Arc::new(run);
        let unswept = {
            let mut held = real.held();
            // First: during a sweep the pidfile is out of `held` (and its lock still taken).
            if held.sweeping {
                return Err("the leftover sweep of an earlier start is still running".into());
            }
            let mut slot = real
                .pidfile
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if slot.is_none() {
                match Pidfile::acquire(run.as_fd()) {
                    Ok(Ok(pidfile)) => *slot = Some(pidfile),
                    Ok(Err(_)) => {
                        return Err("another Mimik uses this data directory's assistant".into());
                    }
                    Err(refusal) => return Err(format!("the pidfile: {refusal}")),
                }
            }
            held.run = Some(Arc::clone(&run));
            if held.swept {
                None
            } else {
                held.sweeping = true;
                slot.take()
            }
        };
        if let Some(pidfile) = unswept {
            // A leftover's kill waits for its end: off the async runtime, as in-flight work
            // (a settle waits for it), and the sweep itself puts the pidfile back, so a
            // preflight dropped meanwhile loses nothing. Until the leftover has ended, its
            // entry and every generation directory stay, and nothing starts.
            let (data_dir, swept_run) = (real.settings.data_dir.clone(), Arc::clone(&run));
            let (held, slot, ticket) = (
                Arc::clone(&real.held),
                Arc::clone(&real.pidfile),
                real.work.enter(),
            );
            #[cfg(test)]
            let pausing = Arc::clone(real);
            let swept = tokio::task::spawn_blocking(move || {
                // Puts the pidfile back and ends the sweep however this closure ends, a panic
                // included.
                let mut sweep = SweepGuard {
                    held,
                    slot,
                    pidfile: Some(pidfile),
                    swept: false,
                    _ticket: ticket,
                };
                #[cfg(test)]
                pausing.paused("sweep");
                let pidfile = sweep.pidfile.as_mut().expect("held until dropped");
                let swept = sweep_leftover(pidfile, &data_dir);
                let ended = matches!(swept, Swept::Nothing | Swept::Killed);
                if ended {
                    rundir::sweep_generations(swept_run.as_fd());
                }
                sweep.swept = ended;
                swept
            })
            .await
            .map_err(|_| "the leftover sweep did not finish".to_owned())?;
            match swept {
                Swept::StillRunning => {
                    return Err(
                        "a model server from an earlier run has not exited yet; Restart tries again"
                            .into(),
                    );
                }
                Swept::Unreadable => {
                    return Err("the pidfile run/assistant.pid cannot be read: check that no model server of an earlier run is left, then remove the file".into());
                }
                Swept::Nothing | Swept::Killed => {}
            }
        }
        if real.build(path).is_none() {
            return Err("this build has no model server for this path".into());
        }
        let models = files::open_dir(&real.settings.models_dir, Owners::Mine, true)
            .map_err(|refusal| format!("the models folder: {refusal}"))?;
        let (_, stat) = files::open_file(models.as_fd(), &real.settings.model, Owners::Mine)
            .map_err(|refusal| format!("the model: {refusal}"))?;
        let size = u64::try_from(stat.st_size).unwrap_or(u64::MAX);
        if let Some(available) = mem_available()
            && size > available.saturating_sub(MEMORY_MARGIN)
        {
            return Err(format!(
                "not enough memory for this model ({} MiB, {} MiB available)",
                size >> 20,
                available >> 20
            ));
        }
        Ok(())
    }

    fn cancel(&self) {
        self.0.work.cancel();
    }

    async fn settle(&self) {
        self.0.work.settle().await;
    }

    async fn probe(&self, path: Path) -> Vec<(Level, Result<(), String>)> {
        let real = Arc::clone(&self.0);
        let ticket = real.work.enter();
        tokio::task::spawn_blocking(move || {
            real.probe_levels(path, &ticket)
                .into_iter()
                .map(|(level, probed)| (level, probed.eligible()))
                .collect()
        })
        .await
        .unwrap_or_default()
    }

    async fn start(
        &self,
        generation: u64,
        level: Level,
        path: Path,
    ) -> Result<Arc<dyn Child>, Fail> {
        self.start_real(generation, level, path)
            .await
            .map(|child| child as Arc<dyn Child>)
    }
}

/// The start's verification, off the async runtime; a cancelled one is "cancelled".
async fn checked(
    real: &Arc<Real>,
    path: Path,
    level: Level,
    ticket: &Ticket,
) -> Result<Checked, Fail> {
    let (real, ticket) = (Arc::clone(real), ticket.clone());
    tokio::task::spawn_blocking(move || {
        real.verify(path, level, &ticket).map_err(|why| {
            if ticket.cancelled() {
                Fail::Other("cancelled".into())
            } else {
                Fail::Other(why)
            }
        })
    })
    .await
    .map_err(|_| Fail::Other("verification failed".into()))?
}

impl Launch {
    /// [`Launcher::start`] with the concrete child.
    pub(crate) async fn start_real(
        &self,
        generation: u64,
        level: Level,
        path: Path,
    ) -> Result<Arc<RealChild>, Fail> {
        let real = Arc::clone(&self.0);
        // Every blocking part holds a ticket: a dropped start's work still counts until it
        // has finished (and cleaned up), and a cancel stops it before it spawns or records.
        let ticket = real.work.enter();
        let Checked {
            files,
            model,
            build_dir,
        } = checked(&real, path, level, &ticket).await?;
        let build_dir = Arc::new(build_dir);
        let token = Arc::new(Token::new());
        let bwrap = real.held().bwrap.clone();
        let files = Arc::new(files);
        let model = Arc::new(model);
        // The spawn blocks (bwrap's status line, up to 30 s): off the async runtime. A start
        // dropped meanwhile drops its result, whose guard kills the child.
        let spawn = |port: u16| {
            let (real, files, model, token, build_dir) = (
                Arc::clone(&real),
                Arc::clone(&files),
                Arc::clone(&model),
                Arc::clone(&token),
                Arc::clone(&build_dir),
            );
            let job = Job {
                name: generation.to_string(),
                key: generation,
                ticket: ticket.clone(),
            };
            tokio::task::spawn_blocking(move || match level {
                Level::Bubblewrap => {
                    real.spawn_sandbox(&job, &files, &model, &token, &Command::Server)
                }
                Level::Unconfined => {
                    let server_name = real.build.as_ref().map(|build| build.server.clone());
                    let server = files
                        .iter()
                        .find(|file| Some(&file.name) == server_name.as_ref())
                        .map(|file| ServerFile {
                            file,
                            build_dir: &build_dir,
                        });
                    real.spawn_unconfined(&job, server, &model, &token, &Command::Server, port)
                }
                Level::Landlock => Err(Fail::Sandbox(
                    "Landlock confinement is not in this release".into(),
                )),
            })
        };
        let joined = |result: Result<Result<Spawned, Fail>, tokio::task::JoinError>| {
            result.map_err(|_| Fail::Other("the start did not finish".into()))?
        };
        if level != Level::Unconfined {
            let spawned = joined(spawn(0).await)?;
            return real.child(spawned, level, &token, bwrap.as_ref(), generation);
        }
        // A child that cannot bind its port exits; a new port is tried, at most 3.
        let mut tries = 0;
        loop {
            tries += 1;
            let port = pick_port(&real.settings.runtime_dir)?;
            let spawned = joined(spawn(port).await)?;
            let child = real.child(spawned, level, &token, None, generation)?;
            let bound = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if child.summary().failed.bind {
                        return false;
                    }
                    if child.exit.borrow().is_some() {
                        return true;
                    }
                    let (pid, held) = (child.pid, ticket.clone());
                    let listening = tokio::task::spawn_blocking(move || {
                        let _ticket = held;
                        endpoint::listeners(port, pid).is_ok_and(|result| result.is_ok())
                    })
                    .await
                    .unwrap_or(false);
                    if listening {
                        return true;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await
            .unwrap_or(true);
            if bound || tries >= PORT_TRIES {
                return Ok(child);
            }
            child.stop().await;
        }
    }
}

/// A probe's outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Probe {
    Passed,
    /// The level cannot work here (a definite cause): it is skipped.
    Failed(String),
    /// Nothing definite (a timeout, a missing pidfd): tried again, and never a reason alone to
    /// skip the level.
    Inconclusive(String),
}

/// A level's probe after its retries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Probed {
    Passed,
    /// Inconclusive every time: eligible for a real start (which decides), but never reported
    /// as passed.
    Kept(String),
    /// Skipped.
    Failed(String),
}

impl Probed {
    /// The manager's view: a kept level is eligible.
    pub(crate) fn eligible(self) -> Result<(), String> {
        match self {
            Probed::Passed | Probed::Kept(_) => Ok(()),
            Probed::Failed(why) => Err(why),
        }
    }
}

/// Runs `probe` up to [`PROBE_TRIES`] times while it is inconclusive: a definite failure
/// skips the level, and a level that stays inconclusive is kept (the real start decides).
pub(crate) fn retry_probe(
    level: Level,
    pause: Duration,
    cancelled: &dyn Fn() -> bool,
    mut probe: impl FnMut() -> Probe,
) -> Probed {
    let mut last = String::new();
    for attempt in 1..=PROBE_TRIES {
        match probe() {
            Probe::Passed => return Probed::Passed,
            Probe::Failed(why) => return Probed::Failed(why),
            Probe::Inconclusive(why) => {
                eprintln!(
                    "assistant: level {} probe inconclusive (attempt {attempt}): {why}",
                    level.number()
                );
                last = why;
                if attempt < PROBE_TRIES && sleep_unless_cancelled(pause, cancelled) {
                    return Probed::Failed("cancelled".into());
                }
            }
        }
    }
    eprintln!(
        "assistant: level {} kept: its probe stayed inconclusive, the start decides",
        level.number()
    );
    Probed::Kept(last)
}

/// Sleeps `pause` in 100 ms slices; true (at once) when `cancelled` says so.
fn sleep_unless_cancelled(pause: Duration, cancelled: &dyn Fn() -> bool) -> bool {
    let deadline = std::time::Instant::now() + pause;
    loop {
        if cancelled() {
            return true;
        }
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return false;
        }
        std::thread::sleep(left.min(Duration::from_millis(100)));
    }
}

/// The endpoint check of the probe's connection, ended by a cancel.
async fn checked_endpoint(connector: &Arc<dyn Connect>, ticket: &Ticket) -> Probe {
    let connecting = tokio::time::timeout(Duration::from_secs(10), connector.connect());
    let connected = tokio::select! {
        connected = connecting => connected,
        () = until_cancelled(ticket) => return Probe::Failed("cancelled".into()),
    };
    match connected {
        Ok(Ok(_)) => Probe::Passed,
        Ok(Err(error)) => endpoint_verdict(&error),
        Err(_) => Probe::Inconclusive("the probe's socket did not answer".into()),
    }
}

/// The verdict on a report that is not ok: a port another process took between the
/// launcher's pick and the probe's bind is tried again (on a fresh port); anything else
/// failed inside the level.
pub(crate) fn report_verdict(report: &serde_json::Value) -> Probe {
    if report["retry"] == serde_json::json!(true) {
        Probe::Inconclusive("the probe's port was taken meanwhile".into())
    } else {
        Probe::Failed(format!("the probe failed inside the level: {report}"))
    }
}

/// Resolves once `ticket` is cancelled (checked every 100 ms).
async fn until_cancelled(ticket: &Ticket) {
    while !ticket.cancelled() {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The probe's verdict on an endpoint error: a refusal is definite; running out of
/// descriptors, memory or another transient resource proves nothing about the level.
pub(crate) fn endpoint_verdict(error: &std::io::Error) -> Probe {
    let transient = [
        libc::EMFILE,
        libc::ENFILE,
        libc::ENOMEM,
        libc::EAGAIN,
        libc::EINTR,
        libc::ENOBUFS,
    ];
    if error
        .raw_os_error()
        .is_some_and(|code| transient.contains(&code))
    {
        Probe::Inconclusive(format!("the endpoint check could not run: {error}"))
    } else {
        Probe::Failed(format!("the endpoint check refused the probe: {error}"))
    }
}

/// How long the probe may take to report from inside its level.
const PROBE_REPORT_WITHIN: Duration = Duration::from_secs(30);
/// Probe attempts while inconclusive.
const PROBE_TRIES: usize = 3;

impl Real {
    /// Runs the level's probe: the inner command under the full profile, then the endpoint
    /// check against it (on level 1 that asserts the process shape).
    pub(crate) fn probe_level(
        self: &Arc<Self>,
        path: Path,
        level: Level,
        ticket: &Ticket,
    ) -> Probe {
        match self.inner_report(path, level, ticket) {
            Ok((report, verdict)) if report["ok"] == serde_json::json!(true) => verdict,
            Ok((_, verdict)) => verdict,
            Err(outcome) => outcome,
        }
    }

    /// The level's probe, tried again while inconclusive; a level that stays inconclusive is
    /// kept (the real start decides), never skipped on a timeout alone.
    fn probe_with_retries(self: &Arc<Self>, path: Path, level: Level, ticket: &Ticket) -> Probed {
        let hinted =
            |why: String| match (level, bwrap::namespace_hint(std::path::Path::new("/proc"))) {
                (Level::Bubblewrap, Some(hint)) => format!("{why} ({hint})"),
                _ => why,
            };
        match retry_probe(
            level,
            Duration::from_secs(2),
            &|| ticket.cancelled(),
            || {
                if ticket.cancelled() {
                    // Ends the retries at once; the session that asked is gone.
                    return Probe::Failed("cancelled".into());
                }
                self.probe_level(path, level, ticket)
            },
        ) {
            Probed::Passed => Probed::Passed,
            Probed::Kept(why) => Probed::Kept(hinted(why)),
            Probed::Failed(why) => Probed::Failed(hinted(why)),
        }
    }

    /// Every level's probe (blocking).
    pub(crate) fn probe_levels(
        self: &Arc<Self>,
        path: Path,
        ticket: &Ticket,
    ) -> Vec<(Level, Probed)> {
        vec![
            (
                Level::Bubblewrap,
                self.probe_with_retries(path, Level::Bubblewrap, ticket),
            ),
            (
                Level::Landlock,
                Probed::Failed("Landlock confinement is not in this release".into()),
            ),
            (
                Level::Unconfined,
                self.probe_with_retries(path, Level::Unconfined, ticket),
            ),
        ]
    }

    /// Runs the inner command under the level's profile: its first JSON report, and the
    /// endpoint check's verdict on the connection it answers.
    pub(crate) fn inner_report(
        self: &Arc<Self>,
        path: Path,
        level: Level,
        ticket: &Ticket,
    ) -> Result<(serde_json::Value, Probe), Probe> {
        if level == Level::Bubblewrap {
            let bwrap =
                bwrap::preflight().map_err(|unusable| Probe::Failed(unusable.to_string()))?;
            self.held().bwrap = Some(bwrap);
        }
        let Checked { files, model, .. } = self.verify(path, level, ticket).map_err(|why| {
            if ticket.cancelled() {
                Probe::Failed("cancelled".into())
            } else {
                Probe::Failed(why)
            }
        })?;
        let token = Token::new();
        let sequence = self.probes.fetch_add(1, Ordering::SeqCst);
        let job = Job {
            name: format!("probe-{sequence}-{}", level.number()),
            key: PROBE_KEYS + sequence,
            ticket: ticket.clone(),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| Probe::Inconclusive("no probe runtime".into()))?;
        // A spawn that fails (bwrap with no status line included) proves nothing alone: it may
        // be transient. Three of them keep the level for the real start to decide.
        let failed = |fail: Fail| match fail {
            Fail::Sandbox(why) | Fail::Other(why) => Probe::Inconclusive(why),
        };
        let spawned = if level == Level::Bubblewrap {
            self.spawn_sandbox(&job, &files, &model, &token, &Command::Probe)
                .map_err(failed)?
        } else {
            let port = pick_port(&self.settings.runtime_dir).map_err(failed)?;
            self.spawn_unconfined(&job, None, &model, &token, &Command::Probe, port)
                .map_err(failed)?
        };
        let Spawned {
            child: mut owned,
            connector,
            generation,
            marker: _marker,
            ..
        } = spawned;
        // `owned` kills and reaps the probe whichever way this ends.
        let (out, stderr) = owned.child.as_mut().map_or((None, None), |child| {
            (child.stdout.take(), child.stderr.take())
        });
        let out = out.ok_or_else(|| Probe::Inconclusive("no probe output".into()))?;
        let (sender, reported) = std::sync::mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("assistant-probe".into())
            .spawn(move || {
                let _ = sender.send(first_report(std::io::BufReader::new(out)));
            });
        let reported = recv_unless_cancelled(&reported, PROBE_REPORT_WITHIN, ticket)
            .map_err(|_| Probe::Failed("cancelled".into()))?;
        let result = match reported {
            Some(Some(report)) => {
                let verdict = if report["ok"] == serde_json::json!(true) {
                    runtime.block_on(checked_endpoint(&connector, ticket))
                } else {
                    report_verdict(&report)
                };
                Ok((report, verdict))
            }
            Some(None) => {
                // The probe runs Mimik itself, never llama-server: its stderr (bwrap's and the
                // shim's) is trusted here.
                let mut text = String::new();
                if let Some(mut stderr) = stderr {
                    let _ = (&mut stderr).take(4096).read_to_string(&mut text);
                }
                let first = text
                    .lines()
                    .find(|line| line.starts_with("bwrap: "))
                    .unwrap_or("");
                Err(Probe::Inconclusive(
                    format!("the probe did not start {}", first.trim())
                        .trim()
                        .to_owned(),
                ))
            }
            None => Err(Probe::Inconclusive(format!(
                "the probe did not report within {} s",
                PROBE_REPORT_WITHIN.as_secs()
            ))),
        };
        // Kills the probe (and its sandbox), then clears its own entry only.
        drop(owned);
        drop(generation);
        result
    }
}

/// The first JSON line with "ok" (a test harness may print others first, even on the line).
fn first_report(mut stdout: impl BufRead) -> Option<serde_json::Value> {
    for _ in 0..64 {
        let mut line = String::new();
        if !stdout.read_line(&mut line).is_ok_and(|n| n > 0) {
            return None;
        }
        let start = line.find('{').unwrap_or(0);
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line[start..].trim())
            && value.get("ok").is_some()
        {
            return Some(value);
        }
    }
    None
}

#[cfg(test)]
impl RealChild {
    pub(crate) fn connector(&self) -> Arc<dyn Connect> {
        Arc::clone(&self.connector)
    }

    pub(crate) fn pid(&self) -> i32 {
        self.pid
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }
}

#[cfg(test)]
#[path = "launch_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "live_tests.rs"]
mod live_tests;
