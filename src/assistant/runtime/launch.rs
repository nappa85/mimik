//! The real launcher (assisted sheet plan §1.4a): verified files, the generation's
//! directory and token, the runner with the level's command, the endpoint check's connector,
//! the output drain and the child's lifetime. Level 1 runs llama-server under bubblewrap on
//! a Unix socket; level 3 executes it unconfined but for seccomp on `127.0.0.1`. Level 2
//! (Landlock) is step 2c-2b.
use super::bwrap::{self, Bwrap, Profile, Started};
use super::drain::{self, Output};
use super::endpoint::{self, Loopback, Sandbox, SandboxSocket, Watched};
use super::files::{self, HashCache, Owners, Verified};
use super::manager::{Child, Fail, Launcher};
use super::manifest::{Build, MANIFEST};
use super::rundir::{self, Entry, Generation, Pidfile};
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
    /// The probe's own executable at level 1.
    pub const PROBE: RawFd = 10;
    /// The first runtime file at level 1.
    pub const RUNTIME: RawFd = 11;
}

/// The probe's command: what runs in place of llama-server (Mimik itself, or the test
/// binary in tests).
#[derive(Clone, Debug)]
pub(crate) struct InnerCommand {
    pub executable: PathBuf,
    /// Arguments after argv[0], before the level's own.
    pub args: Vec<String>,
    /// Variables beside the minimal environment (none in production).
    pub env: Vec<(String, String)>,
}

impl InnerCommand {
    pub(crate) fn mimik() -> std::io::Result<Self> {
        Ok(Self {
            executable: std::env::current_exe()?,
            args: vec!["assistant-runner".into(), "--probe-inner".into()],
            env: Vec::new(),
        })
    }
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
    pidfile: Option<Pidfile>,
    bwrap: Option<Bwrap>,
}

/// The real launcher.
pub(crate) struct Real {
    settings: Settings,
    /// The pinned CPU build (the embedded manifest's; tests give another).
    build: Option<Build>,
    cache: HashCache,
    held: Mutex<Held>,
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
            held: Mutex::new(Held::default()),
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

    fn record(&self, entry: &Entry) {
        if let Some(pidfile) = self.held().pidfile.as_mut() {
            let _ = pidfile.record(entry);
        }
    }

    fn clear(&self) {
        if let Some(pidfile) = self.held().pidfile.as_mut() {
            let _ = pidfile.clear();
        }
    }

    /// The model and the build's files, verified.
    fn verify(&self, path: Path) -> Result<(Vec<Verified>, Verified), String> {
        let build = self
            .build(path)
            .ok_or_else(|| "this build has no model server for this path".to_owned())?;
        let runtime = files::open_dir(&self.settings.runtime_dir, Owners::MineOrRoot, false)
            .map_err(|refusal| format!("the model runtime: {refusal}"))?;
        let files = files::runtime(runtime.as_fd(), build, &self.cache)
            .map_err(|refusal| format!("the model runtime: {refusal}"))?;
        let models = files::open_dir(&self.settings.models_dir, Owners::Mine, true)
            .map_err(|refusal| format!("the models folder: {refusal}"))?;
        let model = files::model(
            models.as_fd(),
            &self.settings.model,
            &self.settings.model_sha256,
            &self.cache,
        )
        .map_err(|refusal| format!("the model: {refusal}"))?;
        Ok((files, model))
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
    child: std::process::Child,
    runner: Watched,
    /// Level 1: bwrap's report and the reaper.
    sandbox: Option<Arc<Sandbox>>,
    generation: Generation,
    connector: Arc<dyn Connect>,
    status_rest: Option<std::io::BufReader<std::fs::File>>,
}

/// The command run in the child: llama-server, or the probe.
enum Command {
    Server,
    Probe,
}

impl Real {
    /// The command inside the sandbox: llama-server from the bound runtime, or the probe (its
    /// own executable bound by descriptor as well).
    fn sandbox_command(
        &self,
        command: &Command,
        handover: &mut Vec<Handover>,
        extra: &mut Vec<(RawFd, String)>,
    ) -> Result<Vec<String>, Fail> {
        let socket = format!("{}/{}", bwrap::SOCKET_DIR, endpoint::SOCKET);
        Ok(match command {
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
                let exe = rustix::fs::open(
                    &self.settings.inner.executable,
                    rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
                    rustix::fs::Mode::empty(),
                )
                .map_err(|_| Fail::Other("Mimik's own binary cannot be opened".into()))?;
                handover.push(Handover {
                    fd: exe,
                    target: fds::PROBE,
                });
                extra.push((fds::PROBE, "/opt/mimik/mimik".to_owned()));
                let mut argv = vec!["/opt/mimik/mimik".to_owned()];
                argv.extend(self.settings.inner.args.iter().cloned());
                argv.push("1".to_owned());
                argv
            }
        })
    }

    /// Starts level 1: bwrap with the profile, then reads `child-pid` and `pid-namespace`.
    fn spawn_sandbox(
        &self,
        name: &str,
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
        let generation = Generation::create(&run, name)
            .map_err(|refusal| Fail::Other(format!("the socket directory: {refusal}")))?;
        let seccomp = bytes_pipe(&seccomp::bytes(&seccomp::program(
            seccomp::Profile::Sandbox,
        )))
        .map_err(Fail::Other)?;
        let (status_read, status_write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)
            .map_err(|_| Fail::Other("no pipe".into()))?;
        let token_pipe = token.pipe().map_err(|_| Fail::Other("no pipe".into()))?;
        let (mut handover, runtime) =
            sandbox_handover(status_write, seccomp, token_pipe, &generation, model, files)?;
        let mut extra = Vec::new();
        let program = self.sandbox_command(command, &mut handover, &mut extra)?;
        let env = match command {
            Command::Server => Vec::new(),
            Command::Probe => self.settings.inner.env.clone(),
        };
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
        let spec = Spec {
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
        };
        let child = runner::launch(&self.settings.runner, &spec, &handover)
            .map_err(|_| Fail::Sandbox("the runner could not be started".into()))?;
        drop(handover);
        let pid = i32::try_from(child.id()).unwrap_or(0);
        let runner = Watched::open(pid).map_err(|_| Fail::Other("no pidfd".into()))?;
        if let Some(entry) = Entry::of(
            0,
            pid,
            vec![
                bwrap.path.display().to_string(),
                self.settings.runner.program.display().to_string(),
            ],
        ) {
            self.record(&entry);
        }
        let mut child = child;
        let (
            Started {
                child_pid,
                pid_namespace,
            },
            rest,
        ) = match read_started(status_read) {
            Ok(started) => started,
            Err(fail) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(fail);
            }
        };
        let reaper = Watched::open(child_pid)
            .map_err(|_| Fail::Sandbox("the sandbox's reaper has gone".into()))?;
        let sandbox = Arc::new(Sandbox {
            reaper,
            pid_namespace,
        });
        let connector: Arc<dyn Connect> = Arc::new(SandboxSocket {
            directory: Arc::new(dup(&generation.fd).map_err(Fail::Other)?),
            sandbox: Arc::clone(&sandbox),
        });
        Ok(Spawned {
            child,
            runner,
            sandbox: Some(sandbox),
            generation,
            connector,
            status_rest: Some(rest),
        })
    }

    /// Starts level 3: the verified executable with seccomp, on `127.0.0.1:<port>`.
    fn spawn_unconfined(
        &self,
        name: &str,
        server: Option<&Verified>,
        model: &Verified,
        token: &Token,
        command: &Command,
        port: u16,
    ) -> Result<Spawned, Fail> {
        let run = self.run_dir().map_err(Fail::Other)?;
        let generation = Generation::create(&run, name)
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
        let (executable, argv) = match command {
            Command::Server => {
                let server = server.ok_or_else(|| Fail::Other("no llama-server".into()))?;
                let mut argv = vec!["llama-server".to_owned()];
                argv.extend(self.server_args(&model_path, "127.0.0.1", Some(port), &token_file));
                (dup(&server.fd).map_err(Fail::Other)?, argv)
            }
            Command::Probe => {
                let exe = rustix::fs::open(
                    &self.settings.inner.executable,
                    rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
                    rustix::fs::Mode::empty(),
                )
                .map_err(|_| Fail::Other("Mimik's own binary cannot be opened".into()))?;
                let mut argv = vec!["mimik".to_owned()];
                argv.extend(self.settings.inner.args.iter().cloned());
                argv.extend(["3".to_owned(), port.to_string(), token_file.clone()]);
                (exe, argv)
            }
        };
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
        let mut env = vec![
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
            (
                "HOME".to_owned(),
                directory.join("home").display().to_string(),
            ),
        ];
        if matches!(command, Command::Probe) {
            env.extend(self.settings.inner.env.iter().cloned());
        }
        let spec = Spec {
            parent: std::process::id(),
            keep: vec![fds::MODEL],
            env,
            exec: Exec::Server {
                fd: fds::SERVER,
                argv,
            },
        };
        let child = runner::launch(&self.settings.runner, &spec, &handover)
            .map_err(|_| Fail::Other("the runner could not be started".into()))?;
        drop(handover);
        let pid = i32::try_from(child.id()).unwrap_or(0);
        let runner = Watched::open(pid).map_err(|_| Fail::Other("no pidfd".into()))?;
        let mut executables = vec![self.settings.runner.program.display().to_string()];
        if let Some(server) = server
            && let Some(build) = &self.build
        {
            executables.push(
                self.settings
                    .runtime_dir
                    .join(&build.directory)
                    .join(&server.name)
                    .display()
                    .to_string(),
            );
        }
        if let Some(entry) = Entry::of(0, pid, executables) {
            self.record(&entry);
        }
        let connector: Arc<dyn Connect> = Arc::new(Loopback {
            port,
            child: Arc::new(Watched::open(pid).map_err(|_| Fail::Other("no pidfd".into()))?),
        });
        Ok(Spawned {
            child,
            runner,
            sandbox: None,
            generation,
            connector,
            status_rest: None,
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
fn read_started(status: OwnedFd) -> Result<(Started, std::io::BufReader<std::fs::File>), Fail> {
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
    let (started, rest) = report
        .recv_timeout(STATUS_WITHIN)
        .map_err(|_| Fail::Sandbox("bubblewrap did not report the sandbox".into()))?;
    let started = started.ok_or_else(|| {
        Fail::Sandbox("bubblewrap failed before starting the model server".into())
    })?;
    Ok((started, rest))
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

/// A running generation's child.
pub(crate) struct RealChild {
    server: Arc<dyn Server>,
    /// The checked connector (live tests reach the server through it with other tokens).
    #[cfg(test)]
    connector: Arc<dyn Connect>,
    exit: watch::Receiver<Option<Exit>>,
    pid: i32,
    /// Output events so far (sandbox and bind errors decide the failure kind).
    seen: Arc<Mutex<Vec<Output>>>,
    healthy: std::sync::atomic::AtomicBool,
    generation_dir: Mutex<Option<Generation>>,
    level: Level,
    notes: Vec<String>,
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
        let seen = self
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let healthy = self.healthy.load(std::sync::atomic::Ordering::SeqCst);
        if !healthy && seen.contains(&Output::SandboxError) {
            Fail::Sandbox(format!("bubblewrap could not set up the sandbox ({why})"))
        } else if seen.contains(&Output::ModelLoadError) {
            Fail::Other(format!("the model could not be loaded ({why})"))
        } else {
            Fail::Other(format!("the model server exited ({why})"))
        }
    }

    async fn stop(&self) {
        let mut exit = self.exit.clone();
        let running = exit.borrow().is_none();
        if running {
            if let Some(pid) = rustix::process::Pid::from_raw(self.pid) {
                let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
            }
            let gone = tokio::time::timeout(STOP_GRACE, exit.wait_for(Option::is_some))
                .await
                .is_ok_and(|result| result.is_ok());
            if !gone && let Some(pid) = rustix::process::Pid::from_raw(self.pid) {
                let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
                let _ = exit.wait_for(Option::is_some).await;
            }
        }
        self.generation_dir
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }

    fn answered_health(&self) {
        self.healthy
            .store(true, std::sync::atomic::Ordering::SeqCst);
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

/// Waits for the runner's exit (its pidfd), reaps it, and on level 1 waits for the reaper
/// too, so "gone" means the whole sandbox has gone.
async fn watch_exit(
    mut child: std::process::Child,
    runner: Watched,
    sandbox: Option<Arc<Sandbox>>,
    exit: watch::Sender<Option<Exit>>,
    on_gone: impl FnOnce() + Send + 'static,
) {
    if let Ok(fd) = tokio::io::unix::AsyncFd::new(runner.pidfd) {
        let _ = fd.readable().await;
    }
    let status = child.wait();
    if let Some(sandbox) = sandbox
        && !endpoint::exited(&sandbox.reaper.pidfd)
    {
        if let Some(pid) = rustix::process::Pid::from_raw(sandbox.reaper.pid) {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
        }
        if let Ok(fd) = sandbox.reaper.pidfd.try_clone()
            && let Ok(fd) = tokio::io::unix::AsyncFd::new(fd)
        {
            let _ = tokio::time::timeout(STOP_GRACE, fd.readable()).await;
        }
    }
    on_gone();
    let why = match status {
        Ok(status) => status.to_string(),
        Err(_) => "unknown status".into(),
    };
    let _ = exit.send(Some(Exit { why }));
}

impl Real {
    fn child(
        self: &Arc<Self>,
        spawned: Spawned,
        level: Level,
        token: &Token,
        bwrap: Option<&Bwrap>,
    ) -> Arc<RealChild> {
        let Spawned {
            mut child,
            runner,
            sandbox,
            generation,
            connector,
            status_rest,
        } = spawned;
        let pid = i32::try_from(child.id()).unwrap_or(0);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (events, mut incoming) = tokio::sync::mpsc::unbounded_channel();
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
            if let Some(stream) = stream {
                let _ = drain::spawn(name, stream, events.clone());
            }
        }
        if let Some(rest) = status_rest {
            // Later status lines (`exit-code`) are read and dropped: exit is seen by pidfd.
            let _ = std::thread::Builder::new()
                .name("assistant-status".into())
                .spawn(move || {
                    let _ = std::io::copy(&mut rest.take(64 * 1024), &mut std::io::sink());
                });
        }
        let collected = Arc::clone(&seen);
        tokio::spawn(async move {
            while let Some(event) = incoming.recv().await {
                collected
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(event);
            }
        });
        let (exit_sender, exit) = watch::channel(None);
        let launcher = Arc::clone(self);
        tokio::spawn(watch_exit(child, runner, sandbox, exit_sender, move || {
            launcher.clear();
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
        Arc::new(RealChild {
            server,
            #[cfg(test)]
            connector,
            exit,
            pid,
            seen,
            healthy: std::sync::atomic::AtomicBool::new(false),
            generation_dir: Mutex::new(Some(generation)),
            level,
            notes,
        })
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
        {
            let mut held = real.held();
            if held.pidfile.is_none() {
                match Pidfile::acquire(run.as_fd()) {
                    Ok(Ok(mut pidfile)) => {
                        pidfile.sweep();
                        rundir::sweep_generations(run.as_fd());
                        held.pidfile = Some(pidfile);
                    }
                    Ok(Err(_)) => {
                        return Err("another Mimik uses this data directory's assistant".into());
                    }
                    Err(refusal) => return Err(format!("the pidfile: {refusal}")),
                }
            }
            held.run = Some(Arc::new(run));
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

    async fn probe(&self, path: Path) -> Vec<(Level, Result<(), String>)> {
        let real = Arc::clone(&self.0);
        tokio::task::spawn_blocking(move || {
            vec![
                (Level::Bubblewrap, real.probe_level(path, Level::Bubblewrap)),
                (
                    Level::Landlock,
                    Err("Landlock confinement is not in this release".into()),
                ),
                (Level::Unconfined, real.probe_level(path, Level::Unconfined)),
            ]
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

impl Launch {
    /// [`Launcher::start`] with the concrete child.
    pub(crate) async fn start_real(
        &self,
        generation: u64,
        level: Level,
        path: Path,
    ) -> Result<Arc<RealChild>, Fail> {
        let real = Arc::clone(&self.0);
        let (files, model) = {
            let real = Arc::clone(&real);
            tokio::task::spawn_blocking(move || real.verify(path))
                .await
                .map_err(|_| Fail::Other("verification failed".into()))?
                .map_err(Fail::Other)?
        };
        let token = Token::new();
        let name = generation.to_string();
        let bwrap = real.held().bwrap.clone();
        let spawned = match level {
            Level::Bubblewrap => {
                real.spawn_sandbox(&name, &files, &model, &token, &Command::Server)?
            }
            Level::Unconfined => {
                let server_name = real.build.as_ref().map(|build| build.server.clone());
                let server = files
                    .iter()
                    .find(|file| Some(&file.name) == server_name.as_ref());
                // A child that cannot bind its port exits; a new port is tried, at most 3.
                let mut tries = 0;
                loop {
                    tries += 1;
                    let port = free_port()?;
                    let spawned = real.spawn_unconfined(
                        &name,
                        server,
                        &model,
                        &token,
                        &Command::Server,
                        port,
                    )?;
                    let child = real.child(spawned, level, &token, None);
                    let bound = tokio::time::timeout(Duration::from_secs(10), async {
                        loop {
                            if child
                                .seen
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .contains(&Output::BindError)
                            {
                                return false;
                            }
                            if child.exit.borrow().is_some() {
                                return true;
                            }
                            if endpoint::listeners(port, child.pid)
                                .is_ok_and(|result| result.is_ok())
                            {
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
            Level::Landlock => {
                return Err(Fail::Sandbox(
                    "Landlock confinement is not in this release".into(),
                ));
            }
        };
        Ok(real.child(spawned, level, &token, bwrap.as_ref()))
    }
}

impl Real {
    /// Runs the level's probe: the inner command under the full profile, then the endpoint
    /// check against it (on level 1 that asserts the process shape).
    pub(crate) fn probe_level(self: &Arc<Self>, path: Path, level: Level) -> Result<(), String> {
        let (report, verdict) = self.inner_report(path, level)?;
        if report["ok"] == serde_json::json!(true) {
            verdict
        } else {
            Err(format!("the probe failed inside the level: {report}"))
        }
    }

    /// Runs the inner command under the level's profile: its first JSON report, and the
    /// endpoint check's verdict on the connection it answers.
    pub(crate) fn inner_report(
        self: &Arc<Self>,
        path: Path,
        level: Level,
    ) -> Result<(serde_json::Value, Result<(), String>), String> {
        if level == Level::Bubblewrap {
            let bwrap = bwrap::preflight().map_err(|unusable| unusable.to_string())?;
            self.held().bwrap = Some(bwrap);
        } else {
            files::ancestors_safe(&self.settings.runtime_dir)
                .map_err(|refusal| format!("the model runtime: {refusal}"))?;
        }
        let (files, model) = self.verify(path)?;
        let token = Token::new();
        let name = format!("probe-{}", level.number());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "no probe runtime".to_owned())?;
        let why = |fail: Fail| match fail {
            Fail::Sandbox(why) | Fail::Other(why) => why,
        };
        let spawned = if level == Level::Bubblewrap {
            self.spawn_sandbox(&name, &files, &model, &token, &Command::Probe)
                .map_err(why)?
        } else {
            let port = free_port().map_err(why)?;
            self.spawn_unconfined(&name, None, &model, &token, &Command::Probe, port)
                .map_err(why)?
        };
        let Spawned {
            mut child,
            connector,
            generation,
            ..
        } = spawned;
        let mut stdout = std::io::BufReader::new(child.stdout.take().ok_or("no probe output")?);
        let mut stderr = child.stderr.take();
        // The first JSON line with "ok" (a test harness may print others first).
        let mut report: Option<serde_json::Value> = None;
        for _ in 0..64 {
            let mut line = String::new();
            if !stdout.read_line(&mut line).is_ok_and(|n| n > 0) {
                break;
            }
            // A test harness may print before the report on the same line.
            let start = line.find('{').unwrap_or(0);
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(line[start..].trim())
                && value.get("ok").is_some()
            {
                report = Some(value);
                break;
            }
        }
        let result = if let Some(report) = report {
            let verdict = if report["ok"] == serde_json::json!(true) {
                runtime.block_on(async {
                    match tokio::time::timeout(Duration::from_secs(10), connector.connect()).await {
                        Ok(Ok(_)) => Ok(()),
                        Ok(Err(error)) => {
                            Err(format!("the endpoint check refused the probe: {error}"))
                        }
                        Err(_) => Err("the probe's socket did not answer".to_owned()),
                    }
                })
            } else {
                Err("the probe failed inside the level".to_owned())
            };
            Ok((report, verdict))
        } else {
            let mut text = String::new();
            if let Some(stderr) = stderr.as_mut() {
                let _ = stderr.take(4096).read_to_string(&mut text);
            }
            let first = text
                .lines()
                .find(|line| line.starts_with("bwrap: "))
                .unwrap_or("");
            Err(format!("the probe did not start {}", first.trim())
                .trim()
                .to_owned())
        };
        let _ = child.kill();
        let _ = child.wait();
        self.clear();
        drop(generation);
        result
    }
}

#[cfg(test)]
impl RealChild {
    pub(crate) fn connector(&self) -> Arc<dyn Connect> {
        Arc::clone(&self.connector)
    }

    pub(crate) fn pid(&self) -> i32 {
        self.pid
    }
}

#[cfg(test)]
#[path = "launch_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "live_tests.rs"]
mod live_tests;
