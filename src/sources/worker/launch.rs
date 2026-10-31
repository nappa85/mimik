//! Synchronous, streaming parent API. The watchdog owns and reaps the process tree.
use super::{Error, Inspection, Page, RenderSettings, RenderedPage, Spec, profile, records};
use crate::sandbox::{
    self,
    process::{self, Watched},
    runner::{self, Exec, Handover, RunnerCommand},
};
use std::{
    fs::File,
    io::{BufRead, Read},
    os::{
        fd::OwnedFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// Caller-controlled cancellation. Create before starting the blocking operation and clone
/// it into the calling task. Cancellation is permanent and observed within 10 ms.
#[derive(Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);
impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    fn cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Bounded upload failure; sandbox failures retain a fixed setup reason, never child text.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Reason {
    #[error("{0}")]
    Document(Error),
    #[error("upload unavailable: {0}")]
    Unavailable(String),
    #[error("cancelled")]
    Cancelled,
}
impl From<Error> for Reason {
    fn from(error: Error) -> Self {
        Self::Document(error)
    }
}

/// Inspect a PDF in the worker sandbox.
/// # Errors
/// Returns a fixed document failure or a sandbox setup failure.
pub fn inspect(path: &Path) -> Result<Inspection, Reason> {
    inspect_cancellable(path, &Cancel::default())
}

/// Inspect with a handle that another thread may cancel.
/// # Errors
/// As for [`inspect`], or cancellation.
pub fn inspect_cancellable(path: &Path, cancel: &Cancel) -> Result<Inspection, Reason> {
    let spec = spec(records::Operation::Inspect, vec![]);
    execute(path, &spec, cancel, &Options::production()?, |pipe| {
        records::read_inspection_tracked(pipe)
    })
}

/// Yield validated pages with bounded memory. Pages are provisional until this returns
/// success: a later worker failure invalidates the run. Callbacks must return promptly.
/// # Errors
/// Invalid requests, malformed worker output, setup failures, cancellation or deadlines.
pub fn render(
    path: &Path,
    pages: Vec<Page>,
    cancel: &Cancel,
    mut page: impl FnMut(RenderedPage) -> Result<(), Error>,
) -> Result<(), Reason> {
    // Validate geometry and request cardinality before spawning anything.
    records::RenderReader::new(std::io::empty(), pages.clone())?;
    let spec = spec(
        records::Operation::Render,
        pages.iter().map(|p| p.n).collect(),
    );
    execute(path, &spec, cancel, &Options::production()?, |pipe| {
        let mut reader = match records::RenderReader::new(pipe, pages) {
            Ok(reader) => reader,
            Err(error) => return (Err(error), None),
        };
        let result = (|| {
            while let Some(value) = reader.next_page()? {
                page(value)?;
            }
            Ok(())
        })();
        (result, reader.terminal_error())
    })
}

fn spec(operation: records::Operation, pages: Vec<u32>) -> Spec {
    Spec {
        operation,
        pages,
        settings: RenderSettings::default(),
        input: profile::INPUT.into(),
    }
}

pub(super) struct Options {
    #[cfg(test)]
    pub observed_exit: Arc<std::sync::atomic::AtomicI32>,
    pub runner: RunnerCommand,
    pub executable: PathBuf,
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub wall: Option<Duration>,
    pub cpu: Option<u64>,
}
impl Options {
    fn production() -> Result<Self, Reason> {
        let runner = RunnerCommand::mimik().map_err(|_| unavailable("runner executable"))?;
        Ok(Self {
            #[cfg(test)]
            observed_exit: Arc::default(),
            executable: runner.program.clone(),
            runner,
            argv: vec![
                profile::EXECUTABLE.into(),
                "pdf-worker".into(),
                "--spec".into(),
                profile::SPEC.into(),
            ],
            env: vec![
                ("LANG".into(), "C.UTF-8".into()),
                ("HOME".into(), "/tmp".into()),
            ],
            wall: None,
            cpu: None,
        })
    }
}
fn unavailable(reason: &str) -> Reason {
    Reason::Unavailable(reason.into())
}

struct Root(PathBuf);
impl Root {
    fn new() -> std::io::Result<Self> {
        let mut random = [0; 16];
        getrandom::fill(&mut random).map_err(std::io::Error::other)?;
        let path = std::env::temp_dir().join(format!("mimik-pdf-{}", hex::encode(random)));
        std::fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open(path: &Path) -> Result<(File, PathBuf, sandbox::Identity), Reason> {
    let file = File::options()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)
        .map_err(|_| Reason::from(Error::Unreadable))?;
    let metadata = file.metadata().map_err(|_| Error::Unreadable)?;
    if !metadata.is_file() {
        return Err(Error::Unreadable.into());
    }
    Ok((
        file,
        std::fs::canonicalize(path).map_err(|_| Error::Unreadable)?,
        (metadata.dev(), metadata.ino()),
    ))
}

fn loader(file: &File) -> Result<PathBuf, Reason> {
    use object::{
        Endianness,
        read::elf::{FileHeader, ProgramHeader},
    };
    let mut bytes = Vec::new();
    file.take(256 << 20)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable("ELF interpreter"))?;
    let elf = object::elf::FileHeader64::<Endianness>::parse(&*bytes)
        .map_err(|_| unavailable("ELF interpreter"))?;
    let endian = elf.endian().map_err(|_| unavailable("ELF interpreter"))?;
    let headers = elf
        .program_headers(endian, &*bytes)
        .map_err(|_| unavailable("ELF interpreter"))?;
    for header in headers {
        if header.p_type(endian) == object::elf::PT_INTERP {
            let data = header
                .data(endian, &*bytes)
                .map_err(|()| unavailable("ELF interpreter"))?;
            let name = std::ffi::CStr::from_bytes_with_nul(data)
                .ok()
                .and_then(|s| s.to_str().ok())
                .ok_or_else(|| unavailable("ELF interpreter"))?;
            return std::fs::canonicalize(name).map_err(|_| unavailable("ELF interpreter"));
        }
    }
    Err(unavailable("ELF interpreter"))
}

struct Tree {
    child: std::process::Child,
    pins: Vec<Watched>,
}
impl Tree {
    fn kill(&self) {
        // Kill the namespace init first; the kernel then kills all namespace descendants.
        if let Some(pin) = self.pins.last() {
            let _ = rustix::process::pidfd_send_signal(&pin.pidfd, rustix::process::Signal::KILL);
        }
    }
    fn settle(&mut self) -> std::io::Result<std::process::ExitStatus> {
        let status = self.child.wait()?;
        while self.pins.iter().any(Watched::alive) {
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(status)
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        self.kill();
        let _ = self.settle();
    }
}

#[derive(Clone, Copy, serde::Deserialize)]
struct Started {
    #[serde(rename = "child-pid")]
    reaper: i32,
    #[serde(rename = "inner-pid")]
    inner: Option<i32>,
    #[serde(rename = "inner-start")]
    start: Option<u64>,
    #[serde(default)]
    ended: bool,
}

fn pin(tree: &mut Tree, report: Started) -> std::io::Result<()> {
    if report.ended {
        return Ok(());
    }
    let reaper = Watched::open(report.reaper)?;
    if process::stat_of(report.reaper)?.0 != tree.pins[0].pid {
        return Err(std::io::Error::other("reaper parent"));
    }
    tree.pins.push(reaper);
    match process::open_inner(
        report
            .inner
            .ok_or_else(|| std::io::Error::other("inner PID"))?,
        report
            .start
            .ok_or_else(|| std::io::Error::other("inner identity"))?,
    )? {
        process::Inner::Ours(inner) => tree.pins.push(inner),
        process::Inner::Gone => {}
        process::Inner::Unverified(_) => return Err(std::io::Error::other("inner identity")),
    }
    Ok(())
}

fn prepare(
    path: &Path,
    spec: &Spec,
    options: &Options,
) -> Result<(Root, File, File, sandbox::Profile), Reason> {
    sandbox::host::namespaces(Path::new("/proc")).map_err(|e| unavailable(&e.to_string()))?;
    let root = Root::new().map_err(|_| unavailable("private root"))?;
    let (input_file, input, identity) = open(path)?;
    let (exe, executable, exe_identity) = open(&options.executable)?;
    let loader = loader(&exe)?;
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo")
        .map_err(|_| unavailable("mount discovery"))?;
    let plan = profile::build(profile::Inputs {
        root: &root.0,
        executable: (&executable, exe_identity),
        input: (&input, identity),
        loader: &loader,
        spec: profile::encode(spec)?,
        mountinfo: &mountinfo,
        argv: options.argv.clone(),
        env: options.env.clone(),
    });
    Ok((root, input_file, exe, plan))
}

pub(super) fn execute<T>(
    path: &Path,
    spec: &Spec,
    cancel: &Cancel,
    options: &Options,
    read: impl FnOnce(std::process::ChildStdout) -> (Result<T, Error>, Option<Error>),
) -> Result<T, Reason> {
    if cancel.cancelled() {
        return Err(Reason::Cancelled);
    }
    let (_root, _input, _exe, plan) = prepare(path, spec, options)?;
    let pages = u64::try_from(spec.pages.len()).map_err(|_| Error::Unsupported)?;
    let inspecting = spec.operation == records::Operation::Inspect;
    let wall = options.wall.unwrap_or(Duration::from_secs(if inspecting {
        30
    } else {
        30 * pages
    }));
    let cpu_seconds = options
        .cpu
        .unwrap_or(if inspecting { 20 } else { 25 * pages });
    let pipe = || {
        rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)
            .map_err(|_| unavailable("transport pipe"))
    };
    let (status, status_write) = pipe()?;
    let (marker, marker_write) = pipe()?;
    let (ack_read, ack) = pipe()?;
    let handover = [
        Handover {
            fd: status_write,
            target: 4,
        },
        Handover {
            fd: marker_write,
            target: 12,
        },
        Handover {
            fd: ack_read,
            target: 13,
        },
    ];
    let runner_spec = runner::Spec {
        parent: std::process::id(),
        keep: vec![4, 12, 13],
        env: vec![],
        chdir: None,
        exec: Exec::PdfWorker {
            plan: Box::new(plan),
            status: 4,
            marker: 12,
            ack: 13,
            cpu_seconds,
        },
    };
    let started = Instant::now();
    let mut child = runner::launch(&options.runner, &runner_spec, &handover)
        .map_err(|_| unavailable("runner launch"))?;
    drop(handover);
    let Some(runner_pin) = i32::try_from(child.id())
        .ok()
        .and_then(|pid| Watched::open(pid).ok())
    else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(unavailable("runner pidfd"));
    };
    let stdout = child.stdout.take().expect("runner stdout pipe");
    let stderr = child.stderr.take().expect("runner stderr pipe");
    let tree = Tree {
        child,
        pins: vec![runner_pin],
    };
    let abort = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let err = scope.spawn(move || drain_stderr(stderr));
        let marker = scope.spawn(move || reached_exec(marker));
        let (tx, rx) = mpsc::channel();
        let status = scope.spawn(move || {
            let mut line = String::new();
            let report = std::io::BufReader::new(File::from(status).take(4096))
                .read_line(&mut line)
                .ok()
                .and_then(|_| serde_json::from_str::<Started>(&line).ok());
            let _ = tx.send(report);
        });
        let abort_ref = &abort;
        let watcher =
            scope.spawn(move || watch(tree, &rx, &ack, started + wall, cancel, abort_ref));
        let (result, terminal) = read(stdout);
        if result.is_err() && terminal.is_none() {
            abort.store(true, Ordering::Release);
        }
        let (exit, parent) = watcher.join().map_err(|_| unavailable("worker watcher"))?;
        #[cfg(test)]
        options
            .observed_exit
            .store(exit.unwrap_or(-1), Ordering::Release);
        let _ = status.join();
        let reached_exec = marker.join().unwrap_or(false);
        let _ = err.join();
        reconcile(exit, reached_exec, result.is_ok(), terminal, parent)?;
        result.map_err(Reason::from)
    })
}

fn drain_stderr(mut stderr: std::process::ChildStderr) {
    // Cap matching work at 64 KiB, but keep draining/discarding until EOF.
    sandbox::drain::drain((&mut stderr).take(64 << 10), |_| {});
    let _ = std::io::copy(&mut stderr, &mut std::io::sink());
}

fn reached_exec(marker: OwnedFd) -> bool {
    let mut bytes = Vec::new();
    File::from(marker).take(129).read_to_end(&mut bytes).is_ok()
        && bytes
            .strip_prefix(b"exec\n")
            .and_then(sandbox::Landlock::parse)
            .is_some()
}

fn watch(
    mut tree: Tree,
    report: &mpsc::Receiver<Option<Started>>,
    ack: &OwnedFd,
    deadline: Instant,
    cancel: &Cancel,
    abort: &AtomicBool,
) -> (Option<i32>, Option<Reason>) {
    let mut acknowledged = false;
    let mut parent = None;
    loop {
        // Observe natural exit before treating a reader's terminal error as an abort.
        if tree.pins[0].alive() {
            if cancel.cancelled() {
                parent = Some(Reason::Cancelled);
            } else if Instant::now() >= deadline {
                parent = Some(Error::Timeout.into());
            }
            if parent.is_some() || abort.load(Ordering::Acquire) {
                tree.kill();
                break;
            }
        } else {
            break;
        }
        if !acknowledged {
            match report.try_recv() {
                Ok(Some(report)) => {
                    if pin(&mut tree, report).is_err() {
                        parent = Some(unavailable("process identity"));
                        tree.kill();
                        break;
                    }
                    let _ = rustix::io::write(ack, b"1");
                    acknowledged = true;
                }
                Ok(None) | Err(mpsc::TryRecvError::Disconnected) => {
                    acknowledged = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // A completed error frame may precede the matching exit by a scheduling instant.
    // On parse failure killing is correct, but preserve typed terminal errors by waiting
    // for EOF/natural exit in the reader path (the reader has already observed EOF).
    let exit = tree.settle().ok().and_then(|s| s.code());
    (exit, parent)
}

pub(super) fn reconcile(
    exit: Option<i32>,
    marker: bool,
    done: bool,
    terminal: Option<Error>,
    parent: Option<Reason>,
) -> Result<(), Reason> {
    if !marker {
        let reason = match exit {
            Some(sandbox::exit::SESSION) => "sandbox session setup failed",
            Some(sandbox::exit::IDENTITY) => "sandbox bind identity changed",
            Some(sandbox::exit::CAPABILITIES) => "sandbox capability removal failed",
            Some(sandbox::exit::SECCOMP) => "sandbox first seccomp filter failed",
            Some(sandbox::exit::MARKER) => "sandbox marker failed",
            Some(sandbox::exit::DESCRIPTORS) => "sandbox descriptor cleanup failed",
            Some(sandbox::exit::LANDLOCK) => "sandbox Landlock setup failed",
            Some(runner::exit::LIMITS) => "sandbox resource limits failed",
            Some(runner::exit::PARENT) => "sandbox parent identity changed",
            _ => "sandbox namespace setup or runner failed",
        };
        return Err(unavailable(reason));
    }
    if let Some(reason) = parent {
        return Err(reason);
    }
    if done && exit == Some(0) {
        return Ok(());
    }
    if !done
        && let Some(code) = exit
        && let Some(reason) = Error::from_exit_code(code)
        && terminal.is_none_or(|error| error.exit_code() == code)
    {
        return Err(reason.into());
    }
    Err(Error::Unreadable.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reconciliation_requires_both_successes_and_agreeing_errors() {
        for code in 0..=255 {
            for done in [false, true] {
                for terminal in [
                    None,
                    Some(Error::Unreadable),
                    Some(Error::Timeout),
                    Some(Error::TooLarge),
                    Some(Error::TooManyPages),
                    Some(Error::PasswordProtected),
                    Some(Error::Unsupported),
                ] {
                    let actual = reconcile(Some(code), true, done, terminal, None);
                    let expected = if code == 0 && done {
                        Ok(())
                    } else if !done && terminal.is_none_or(|e| e.exit_code() == code) {
                        Err(Error::from_exit_code(code)
                            .unwrap_or(Error::Unreadable)
                            .into())
                    } else {
                        Err(Error::Unreadable.into())
                    };
                    assert_eq!(actual, expected, "{code} {done} {terminal:?}");
                    assert!(matches!(
                        reconcile(Some(code), false, done, terminal, None),
                        Err(Reason::Unavailable(_))
                    ));
                }
            }
        }
        assert_eq!(
            reconcile(None, true, true, None, None),
            Err(Error::Unreadable.into())
        );
        assert_eq!(
            reconcile(None, true, false, None, Some(Error::Timeout.into())),
            Err(Error::Timeout.into())
        );
        assert_eq!(
            reconcile(None, true, false, None, Some(Reason::Cancelled)),
            Err(Reason::Cancelled)
        );
    }
}
