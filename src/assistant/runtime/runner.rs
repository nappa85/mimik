//! The runner (assisted sheet plan §1.4a, "Runner"): `mimik assistant-runner`, a hidden
//! subcommand Mimik spawns from one dedicated, long-lived thread (the parent-death signal
//! follows the spawning thread). It sets `PR_SET_PDEATHSIG` (SIGKILL), then checks that its
//! parent is still the Mimik PID it was given (the race before the signal is set); sets
//! `RLIMIT_CORE` to 0 and `RLIMIT_FSIZE` to 64 MiB; keeps only the descriptors it was handed;
//! and executes the level's command in place with exactly the environment it was given:
//! `bwrap` on level 1, or on the TCP levels the verified llama-server descriptor with
//! `execveat` after installing the seccomp program (which keeps the parent-death signal and
//! `no_new_privs` from being changed again).
use super::seccomp;
use serde::{Deserialize, Serialize};
use std::ffi::{CString, OsString};
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{LazyLock, Mutex, mpsc};

/// Where the runner reads its spec.
pub(crate) const SPEC_FD: RawFd = 3;
/// The number the TCP levels execute llama-server from; also their `RLIMIT_NOFILE`, so no
/// descriptor can take the number after the exec (`seccomp.rs`, `Profile::Tcp`).
pub(crate) const EXEC_FD: RawFd = 1023;
/// `RLIMIT_FSIZE`: a write past it raises `SIGXFSZ`, left at its default action.
pub(crate) const FSIZE: u64 = 64 << 20;
/// The most bytes of a spec.
const SPEC_BOUND: u64 = 1 << 20;
/// The most descriptors (the spec's included) moved between fork and exec: a fixed array, as
/// nothing may allocate there. Level 1 hands over the status, seccomp, token, directory,
/// model, shim and marker descriptors and one per runtime file.
pub(crate) const MAX_HANDOVER: usize = 64;

/// The runner's exit codes when it does not reach its exec.
pub(crate) mod exit {
    pub const DEATH_SIGNAL: i32 = 120;
    pub const SPEC: i32 = 121;
    pub const PARENT: i32 = 122;
    pub const LIMITS: i32 = 123;
    pub const DESCRIPTORS: i32 = 124;
    pub const EXEC: i32 = 125;
    pub const SECCOMP: i32 = 126;
}

/// What the runner executes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub(crate) enum Exec {
    /// Level 1: bwrap at a fixed path with the profile's arguments.
    Bwrap { path: String, args: Vec<String> },
    /// The TCP levels: the descriptor `fd` (llama-server, or the probe), argv given whole.
    Server { fd: RawFd, argv: Vec<String> },
}

/// The runner's whole input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Spec {
    /// Mimik's PID.
    pub parent: u32,
    /// The handed-over descriptor numbers (besides 0, 1 and 2).
    pub keep: Vec<RawFd>,
    /// The exact environment of the exec.
    pub env: Vec<(String, String)>,
    pub exec: Exec,
}

fn read_spec(fd: RawFd) -> Option<Spec> {
    // SAFETY: the spec descriptor is handed over by the parent and owned by this process
    // from here on; nothing else uses that number.
    let file = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) });
    let mut text = String::new();
    file.take(SPEC_BOUND).read_to_string(&mut text).ok()?;
    serde_json::from_str(&text).ok()
}

/// Closes every descriptor but 0, 1, 2 and `keep`.
fn close_others(keep: &[RawFd]) -> std::io::Result<()> {
    let listing: Vec<RawFd> = std::fs::read_dir("/proc/self/fd")?
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
        .collect();
    for fd in listing {
        if fd > 2 && !keep.contains(&fd) {
            // SAFETY: closing a descriptor of this single-purpose process that it was not
            // handed; the directory listing's own descriptor is already closed (EBADF is
            // ignored).
            unsafe { libc::close(fd) };
        }
    }
    Ok(())
}

fn limits() -> std::io::Result<()> {
    use rustix::process::{Resource, Rlimit, setrlimit};
    setrlimit(
        Resource::Core,
        Rlimit {
            current: Some(0),
            maximum: Some(0),
        },
    )?;
    setrlimit(
        Resource::Fsize,
        Rlimit {
            current: Some(FSIZE),
            maximum: Some(FSIZE),
        },
    )?;
    Ok(())
}

fn c_strings(values: impl IntoIterator<Item = String>) -> Option<Vec<CString>> {
    values
        .into_iter()
        .map(|value| CString::new(value).ok())
        .collect()
}

/// Executes the TCP levels' server: the descriptor moved to [`EXEC_FD`] (close-on-exec),
/// `RLIMIT_NOFILE` set to that number, the seccomp program installed, then `execveat`.
fn exec_server(fd: RawFd, argv: Vec<String>, env: Vec<(String, String)>) -> i32 {
    // SAFETY: dup3 onto the fixed number; `fd` is a handed-over descriptor.
    if fd != EXEC_FD && unsafe { libc::dup3(fd, EXEC_FD, libc::O_CLOEXEC) } != EXEC_FD {
        return exit::EXEC;
    }
    if fd != EXEC_FD {
        // SAFETY: the original number of the handed-over descriptor, now duplicated.
        unsafe { libc::close(fd) };
    }
    let limit = rustix::process::Rlimit {
        current: Some(u64::try_from(EXEC_FD).unwrap_or(1023)),
        maximum: Some(u64::try_from(EXEC_FD).unwrap_or(1023)),
    };
    if rustix::process::setrlimit(rustix::process::Resource::Nofile, limit).is_err() {
        return exit::LIMITS;
    }
    let (Some(argv), Some(envp)) = (
        c_strings(argv),
        c_strings(
            env.into_iter()
                .map(|(name, value)| format!("{name}={value}")),
        ),
    ) else {
        return exit::SPEC;
    };
    let argv_ptrs: Vec<*const libc::c_char> = argv
        .iter()
        .map(|arg| arg.as_ptr())
        .chain(std::iter::once(std::ptr::null()))
        .collect();
    let envp_ptrs: Vec<*const libc::c_char> = envp
        .iter()
        .map(|var| var.as_ptr())
        .chain(std::iter::once(std::ptr::null()))
        .collect();
    let program = seccomp::program(seccomp::Profile::Tcp {
        exec_fd: u32::try_from(EXEC_FD).unwrap_or(1023),
    });
    if seccomp::install(&program).is_err() {
        return exit::SECCOMP;
    }
    // SAFETY: execveat with an empty path on the verified descriptor and NUL-terminated
    // argument and environment arrays that outlive the call; it returns only on failure.
    unsafe {
        libc::syscall(
            libc::SYS_execveat,
            EXEC_FD,
            c"".as_ptr(),
            argv_ptrs.as_ptr(),
            envp_ptrs.as_ptr(),
            libc::AT_EMPTY_PATH,
        );
    }
    exit::EXEC
}

/// The runner: returns only when it could not reach its exec, with the exit code.
pub(crate) fn run(spec_fd: RawFd) -> i32 {
    if rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::KILL))
        .is_err()
    {
        return exit::DEATH_SIGNAL;
    }
    let Some(spec) = read_spec(spec_fd) else {
        return exit::SPEC;
    };
    if rustix::process::getppid().map(|pid| pid.as_raw_nonzero().get())
        != i32::try_from(spec.parent).ok()
    {
        return exit::PARENT;
    }
    if limits().is_err() {
        return exit::LIMITS;
    }
    let mut keep = spec.keep.clone();
    if let Exec::Server { fd, .. } = &spec.exec {
        keep.push(*fd);
    }
    if close_others(&keep).is_err() {
        return exit::DESCRIPTORS;
    }
    match spec.exec {
        Exec::Bwrap { path, args } => {
            let error = Command::new(path)
                .args(args)
                .env_clear()
                .envs(spec.env)
                .exec();
            drop(error);
            exit::EXEC
        }
        Exec::Server { fd, argv } => exec_server(fd, argv, spec.env),
    }
}

/// How to start the runner: Mimik's own binary with `assistant-runner --spec-fd 3`, or in
/// tests the test binary re-executed as its runner helper.
#[derive(Clone, Debug)]
pub(crate) struct RunnerCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Variables the runner process itself gets (none in production; the helper mode in
    /// tests). The exec's environment is the spec's.
    pub env: Vec<(OsString, OsString)>,
}

impl RunnerCommand {
    /// Mimik's own binary.
    pub(crate) fn mimik() -> std::io::Result<Self> {
        Ok(Self {
            program: std::env::current_exe()?,
            args: vec![
                "assistant-runner".into(),
                "--spec-fd".into(),
                SPEC_FD.to_string().into(),
            ],
            env: Vec::new(),
        })
    }
}

/// A descriptor handed to the runner at a fixed number.
#[derive(Debug)]
pub(crate) struct Handover {
    pub fd: OwnedFd,
    pub target: RawFd,
}

type Request = (Command, mpsc::Sender<std::io::Result<Child>>);

/// The one long-lived thread every runner is spawned from.
static SPAWNER: LazyLock<Mutex<mpsc::Sender<Request>>> = LazyLock::new(|| {
    let (requests, incoming) = mpsc::channel::<Request>();
    std::thread::Builder::new()
        .name("assistant-spawner".into())
        .spawn(move || {
            while let Ok((mut command, reply)) = incoming.recv() {
                let _ = reply.send(command.spawn());
            }
        })
        .expect("the spawner thread starts");
    Mutex::new(requests)
});

fn spawn_on_spawner(command: Command) -> std::io::Result<Child> {
    let (reply, answer) = mpsc::channel();
    SPAWNER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .send((command, reply))
        .map_err(|_| std::io::Error::other("the spawner thread has gone"))?;
    answer
        .recv()
        .map_err(|_| std::io::Error::other("the spawner thread has gone"))?
}

/// Starts the runner with `spec` on [`SPEC_FD`] and every handover at its target number;
/// stdout and stderr are pipes for the drain.
///
/// # Errors
///
/// The pipe, the spec's encoding or the spawn.
pub(crate) fn launch(
    runner: &RunnerCommand,
    spec: &Spec,
    handover: &[Handover],
) -> std::io::Result<Child> {
    let text = serde_json::to_vec(spec).map_err(std::io::Error::other)?;
    if u64::try_from(text.len()).unwrap_or(u64::MAX) > SPEC_BOUND {
        return Err(std::io::Error::other(format!(
            "the runner's spec is {} bytes, more than the {SPEC_BOUND} it reads",
            text.len()
        )));
    }
    if handover.len() + 1 > MAX_HANDOVER {
        return Err(std::io::Error::other(format!(
            "{} descriptors to hand over, more than the runner's {MAX_HANDOVER}",
            handover.len() + 1
        )));
    }
    // A memory file, not a pipe: written whole before the spawn, whatever its size (a pipe
    // holds about 64 KiB and nothing reads it yet).
    let read = rustix::fs::memfd_create("mimik-runner-spec", rustix::fs::MemfdFlags::CLOEXEC)?;
    {
        let mut writer = std::fs::File::from(rustix::io::dup(&read)?);
        std::io::Write::write_all(&mut writer, &text)?;
    }
    rustix::fs::seek(&read, rustix::fs::SeekFrom::Start(0))?;
    let mut pairs: Vec<(RawFd, RawFd)> = vec![(read.as_raw_fd(), SPEC_FD)];
    pairs.extend(handover.iter().map(|h| (h.fd.as_raw_fd(), h.target)));
    let mut command = Command::new(&runner.program);
    command
        .args(&runner.args)
        .env_clear()
        .envs(runner.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let highest = pairs.iter().map(|(_, target)| *target).max().unwrap_or(0);
    // SAFETY: between fork and exec this only calls fcntl and dup2, which are
    // async-signal-safe, on numbers computed before the fork, and allocates nothing. Every
    // source is first duplicated (close-on-exec) above every target, so no source is
    // overwritten before it is moved; dup2 clears close-on-exec on each target.
    unsafe {
        command.pre_exec(move || {
            let mut moved = [0 as RawFd; MAX_HANDOVER];
            if pairs.len() > moved.len() {
                return Err(std::io::Error::from_raw_os_error(libc::EMFILE));
            }
            for (slot, (source, _)) in moved.iter_mut().zip(pairs.iter()) {
                let fd = libc::fcntl(*source, libc::F_DUPFD_CLOEXEC, highest + 1);
                if fd < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                *slot = fd;
            }
            for (moved, (_, target)) in moved.iter().zip(pairs.iter()) {
                if libc::dup2(*moved, *target) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    let child = spawn_on_spawner(command);
    drop(read);
    child
}

#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;
