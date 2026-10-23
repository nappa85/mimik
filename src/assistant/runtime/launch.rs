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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
}

/// Kills and reaps a spawned runner on every failure path until it is handed to its watcher,
/// so no child outlives a failed start.
struct Owned(Option<std::process::Child>);

impl Owned {
    fn pid(&self) -> i32 {
        self.0
            .as_ref()
            .map_or(0, |child| i32::try_from(child.id()).unwrap_or(0))
    }

    fn hand_over(mut self) -> std::process::Child {
        self.0.take().expect("handed over once")
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
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
        $real.fault_at($step, $child.pid())?;
    };
}

#[cfg(test)]
impl Real {
    fn fault_at(&self, step: &'static str, pid: i32) -> Result<(), Fail> {
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
                return Err(Fail::Other(format!("injected at {step}")));
            }
        }
        Ok(())
    }
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
        let (marker_read, marker_write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)
            .map_err(|_| Fail::Other("no pipe".into()))?;
        let (mut handover, runtime) =
            sandbox_handover(status_write, seccomp, token_pipe, &generation, model, files)?;
        handover.push(Handover {
            fd: marker_write,
            target: fds::MARKER,
        });
        let mut extra = Vec::new();
        let (program, env) = self.sandbox_command(command, &mut handover, &mut extra)?;
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
        let child = Owned(Some(
            runner::launch(&self.settings.runner, &spec, &handover)
                .map_err(|_| Fail::Other("the runner could not be started".into()))?,
        ));
        drop(handover);
        fault_at!(self, "spawned", child);
        let pid = child.pid();
        let runner = Watched::open(pid).map_err(|_| Fail::Other("no pidfd".into()))?;
        if let Some(entry) = Entry::of(
            name.parse().unwrap_or(0),
            pid,
            vec![
                bwrap.path.display().to_string(),
                self.settings.runner.program.display().to_string(),
            ],
        ) {
            self.record(&entry);
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
        ) = read_started(status_read)?;
        fault_at!(self, "started", child);
        let reaper = Watched::open(child_pid)
            .map_err(|_| Fail::Other("the sandbox's reaper has gone".into()))?;
        let sandbox = Arc::new(Sandbox {
            reaper,
            pid_namespace,
        });
        let connector: Arc<dyn Connect> = Arc::new(SandboxSocket {
            directory: Arc::new(dup(&generation.fd).map_err(Fail::Other)?),
            sandbox: Arc::clone(&sandbox),
        });
        fault_at!(self, "connector", child);
        Ok(Spawned {
            child,
            runner,
            sandbox: Some(sandbox),
            generation,
            connector,
            status_rest: Some(rest),
            marker: Some(marker_read),
        })
    }

    /// Level 3's executable (llama-server or Mimik's own binary) and its arguments.
    fn unconfined_command(
        &self,
        command: &Command,
        server: Option<&Verified>,
        model_path: &str,
        port: u16,
        token_file: &str,
    ) -> Result<(OwnedFd, Vec<String>), Fail> {
        match command {
            Command::Server => {
                let server = server.ok_or_else(|| Fail::Other("no llama-server".into()))?;
                let mut argv = vec!["llama-server".to_owned()];
                argv.extend(self.server_args(model_path, "127.0.0.1", Some(port), token_file));
                Ok((dup(&server.fd).map_err(Fail::Other)?, argv))
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
        let spec = Spec {
            parent: std::process::id(),
            keep: vec![fds::MODEL],
            env,
            exec: Exec::Server {
                fd: fds::SERVER,
                argv,
            },
        };
        let child = Owned(Some(
            runner::launch(&self.settings.runner, &spec, &handover)
                .map_err(|_| Fail::Other("the runner could not be started".into()))?,
        ));
        drop(handover);
        fault_at!(self, "spawned", child);
        let pid = child.pid();
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
        if let Some(entry) = Entry::of(name.parse().unwrap_or(0), pid, executables) {
            self.record(&entry);
        }
        fault_at!(self, "recorded", child);
        let connector: Arc<dyn Connect> = Arc::new(Loopback {
            port,
            child: Arc::new(Watched::open(pid).map_err(|_| Fail::Other("no pidfd".into()))?),
        });
        fault_at!(self, "connector", child);
        Ok(Spawned {
            child,
            runner,
            sandbox: None,
            generation,
            connector,
            status_rest: None,
            marker: None,
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
    // A timeout proves nothing about the sandbox (a slow host): counted, the level kept.
    let (started, rest) = report.recv_timeout(STATUS_WITHIN).map_err(|_| {
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

    async fn all_ended(&self, within: Duration) {
        let _ = tokio::time::timeout(within, async {
            loop {
                let ended = self.ended.notified();
                if self.left.load(Ordering::SeqCst) == 0 {
                    return;
                }
                ended.await;
            }
        })
        .await;
    }
}

/// How long `exited` waits for the readers after the exit.
const READERS_WITHIN: Duration = Duration::from_secs(5);

/// Why a child ended: at level 1, a sandbox failure only when the shim never marked its
/// exec (nothing the executed command prints can make it one); otherwise counted.
fn classify(level: Level, executed: bool, summary: Summary, why: &str) -> Fail {
    if level == Level::Bubblewrap && !executed {
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
    signal: Option<OwnedFd>,
    summary: Arc<Mutex<Summary>>,
    /// Level 1: the shim marked its exec.
    executed: Arc<AtomicBool>,
    readers: Arc<Readers>,
    healthy: AtomicBool,
    generation_dir: Mutex<Option<Generation>>,
    level: Level,
    notes: Vec<String>,
}

impl RealChild {
    fn kill(&self, signal: rustix::process::Signal) {
        if let Some(pidfd) = &self.signal {
            let _ = rustix::process::pidfd_send_signal(pidfd, signal);
        }
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
        self.readers.all_ended(READERS_WITHIN).await;
        classify(
            self.level,
            self.executed.load(Ordering::SeqCst),
            self.summary(),
            &why,
        )
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

/// Reads the shim's marker pipe to its end: `executed` once the marker arrived. Without a
/// reader thread the exec cannot be shown either way, so it counts as having happened (a
/// failure then keeps the level).
fn read_marker(marker: OwnedFd, executed: &Arc<AtomicBool>, readers: &Arc<Readers>) {
    let (seen, ended) = (Arc::clone(executed), Arc::clone(readers));
    let spawned = std::thread::Builder::new()
        .name("assistant-marker".into())
        .spawn(move || {
            let mut text = Vec::new();
            let _ = std::fs::File::from(marker).take(64).read_to_end(&mut text);
            if text.starts_with(super::probe::MARKER) {
                seen.store(true, Ordering::SeqCst);
            }
            ended.end();
        });
    if spawned.is_err() {
        executed.store(true, Ordering::SeqCst);
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
    ) -> Arc<RealChild> {
        let Spawned {
            child,
            runner,
            sandbox,
            generation,
            connector,
            status_rest,
            marker,
        } = spawned;
        let mut child = child.hand_over();
        let pid = i32::try_from(child.id()).unwrap_or(0);
        let summary = Arc::new(Mutex::new(Summary::default()));
        let executed = Arc::new(AtomicBool::new(marker.is_none()));
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
            read_marker(marker, &executed, &readers);
        }
        if let Some(rest) = status_rest {
            // Later status lines (`exit-code`) are read and dropped: exit is seen by pidfd.
            let _ = std::thread::Builder::new()
                .name("assistant-status".into())
                .spawn(move || {
                    let _ = std::io::copy(&mut rest.take(64 * 1024), &mut std::io::sink());
                });
        }
        let signal = runner.pidfd.try_clone().ok();
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
        #[cfg(not(test))]
        drop(connector);
        Arc::new(RealChild {
            server,
            #[cfg(test)]
            connector,
            #[cfg(test)]
            token: token.expose().to_owned(),
            exit,
            pid,
            signal,
            summary,
            executed,
            readers,
            healthy: AtomicBool::new(false),
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
                (
                    Level::Bubblewrap,
                    real.probe_with_retries(path, Level::Bubblewrap),
                ),
                (
                    Level::Landlock,
                    Err("Landlock confinement is not in this release".into()),
                ),
                (
                    Level::Unconfined,
                    real.probe_with_retries(path, Level::Unconfined),
                ),
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
        let token = Arc::new(Token::new());
        let name = generation.to_string();
        let bwrap = real.held().bwrap.clone();
        let files = Arc::new(files);
        let model = Arc::new(model);
        if level == Level::Unconfined {
            // Level 3 loads its libraries by path: the runtime's ancestors are checked at every
            // start, not only by the probe.
            let checking = Arc::clone(&real);
            tokio::task::spawn_blocking(move || {
                files::ancestors_safe(&checking.settings.runtime_dir)
                    .map_err(|refusal| Fail::Other(format!("the model runtime: {refusal}")))
            })
            .await
            .map_err(|_| Fail::Other("the runtime check did not finish".into()))??;
        }
        // The spawn blocks (bwrap's status line, up to 30 s): off the async runtime. A start
        // dropped meanwhile drops its result, whose guard kills the child.
        let spawn = |port: u16| {
            let (real, files, model, token, name) = (
                Arc::clone(&real),
                Arc::clone(&files),
                Arc::clone(&model),
                Arc::clone(&token),
                name.clone(),
            );
            tokio::task::spawn_blocking(move || match level {
                Level::Bubblewrap => {
                    real.spawn_sandbox(&name, &files, &model, &token, &Command::Server)
                }
                Level::Unconfined => {
                    let server_name = real.build.as_ref().map(|build| build.server.clone());
                    let server = files
                        .iter()
                        .find(|file| Some(&file.name) == server_name.as_ref());
                    real.spawn_unconfined(&name, server, &model, &token, &Command::Server, port)
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
            return Ok(real.child(spawned, level, &token, bwrap.as_ref()));
        }
        // A child that cannot bind its port exits; a new port is tried, at most 3.
        let mut tries = 0;
        loop {
            tries += 1;
            let port = free_port()?;
            let spawned = joined(spawn(port).await)?;
            let child = real.child(spawned, level, &token, None);
            let bound = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if child.summary().failed.bind {
                        return false;
                    }
                    if child.exit.borrow().is_some() {
                        return true;
                    }
                    let pid = child.pid;
                    let listening = tokio::task::spawn_blocking(move || {
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

/// Runs `probe` up to [`PROBE_TRIES`] times while it is inconclusive: a definite failure
/// skips the level, and a level that stays inconclusive is kept (the real start decides).
pub(crate) fn retry_probe(
    level: Level,
    pause: Duration,
    mut probe: impl FnMut() -> Probe,
) -> Result<(), String> {
    for attempt in 1..=PROBE_TRIES {
        match probe() {
            Probe::Passed => return Ok(()),
            Probe::Failed(why) => return Err(why),
            Probe::Inconclusive(why) => {
                eprintln!(
                    "assistant: level {} probe inconclusive (attempt {attempt}): {why}",
                    level.number()
                );
                if attempt < PROBE_TRIES {
                    std::thread::sleep(pause);
                }
            }
        }
    }
    eprintln!(
        "assistant: level {} kept: its probe stayed inconclusive, the start decides",
        level.number()
    );
    Ok(())
}

/// How long the probe may take to report from inside its level.
const PROBE_REPORT_WITHIN: Duration = Duration::from_secs(30);
/// Probe attempts while inconclusive.
const PROBE_TRIES: usize = 3;

impl Real {
    /// Runs the level's probe: the inner command under the full profile, then the endpoint
    /// check against it (on level 1 that asserts the process shape).
    pub(crate) fn probe_level(self: &Arc<Self>, path: Path, level: Level) -> Probe {
        match self.inner_report(path, level) {
            Ok((report, verdict)) if report["ok"] == serde_json::json!(true) => verdict,
            Ok((report, _)) => {
                Probe::Failed(format!("the probe failed inside the level: {report}"))
            }
            Err(outcome) => outcome,
        }
    }

    /// The level's probe, tried again while inconclusive; a level that stays inconclusive is
    /// kept (the real start decides), never skipped on a timeout alone.
    fn probe_with_retries(self: &Arc<Self>, path: Path, level: Level) -> Result<(), String> {
        retry_probe(level, Duration::from_secs(2), || {
            self.probe_level(path, level)
        })
        .map_err(|why| {
            match (level, bwrap::namespace_hint(std::path::Path::new("/proc"))) {
                (Level::Bubblewrap, Some(hint)) => format!("{why} ({hint})"),
                _ => why,
            }
        })
    }

    /// Runs the inner command under the level's profile: its first JSON report, and the
    /// endpoint check's verdict on the connection it answers.
    pub(crate) fn inner_report(
        self: &Arc<Self>,
        path: Path,
        level: Level,
    ) -> Result<(serde_json::Value, Probe), Probe> {
        if level == Level::Bubblewrap {
            let bwrap =
                bwrap::preflight().map_err(|unusable| Probe::Failed(unusable.to_string()))?;
            self.held().bwrap = Some(bwrap);
        } else {
            files::ancestors_safe(&self.settings.runtime_dir)
                .map_err(|refusal| Probe::Failed(format!("the model runtime: {refusal}")))?;
        }
        let (files, model) = self.verify(path).map_err(Probe::Failed)?;
        let token = Token::new();
        let name = format!("probe-{}", level.number());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| Probe::Inconclusive("no probe runtime".into()))?;
        let failed = |fail: Fail| match fail {
            Fail::Sandbox(why) => Probe::Failed(why),
            Fail::Other(why) => Probe::Inconclusive(why),
        };
        let spawned = if level == Level::Bubblewrap {
            self.spawn_sandbox(&name, &files, &model, &token, &Command::Probe)
                .map_err(failed)?
        } else {
            let port = free_port().map_err(failed)?;
            self.spawn_unconfined(&name, None, &model, &token, &Command::Probe, port)
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
        let (out, stderr) = owned.0.as_mut().map_or((None, None), |child| {
            (child.stdout.take(), child.stderr.take())
        });
        let out = out.ok_or_else(|| Probe::Inconclusive("no probe output".into()))?;
        let (sender, reported) = std::sync::mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("assistant-probe".into())
            .spawn(move || {
                let _ = sender.send(first_report(std::io::BufReader::new(out)));
            });
        let result = match reported.recv_timeout(PROBE_REPORT_WITHIN) {
            Ok(Some(report)) => {
                let verdict = if report["ok"] == serde_json::json!(true) {
                    runtime.block_on(async {
                        match tokio::time::timeout(Duration::from_secs(10), connector.connect())
                            .await
                        {
                            Ok(Ok(_)) => Probe::Passed,
                            Ok(Err(error)) => Probe::Failed(format!(
                                "the endpoint check refused the probe: {error}"
                            )),
                            Err(_) => {
                                Probe::Inconclusive("the probe's socket did not answer".into())
                            }
                        }
                    })
                } else {
                    Probe::Failed("the probe failed inside the level".into())
                };
                Ok((report, verdict))
            }
            Ok(None) => {
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
                Err(Probe::Failed(
                    format!("the probe did not start {}", first.trim())
                        .trim()
                        .to_owned(),
                ))
            }
            Err(_) => Err(Probe::Inconclusive(format!(
                "the probe did not report within {} s",
                PROBE_REPORT_WITHIN.as_secs()
            ))),
        };
        drop(owned);
        self.clear();
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
