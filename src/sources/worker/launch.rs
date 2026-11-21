//! Synchronous, streaming parent API. The watchdog owns and reaps the process tree.
use super::{
    Error, Inspection, Page, PageWords, Pins, RenderSettings, RenderedPage, Spec, profile, records,
};
use crate::sandbox::{
    self,
    process::{self, Watched},
    runner::{self, Exec, Handover, RunnerCommand},
};
use std::{
    fs::File,
    io::{BufRead, Read},
    os::{
        fd::{AsFd, OwnedFd},
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// Caller-controlled cancellation. Create before starting the blocking operation and clone
/// it into the calling task. Cancellation is permanent; the watchdog polls every 10 ms.
#[derive(Clone, Default)]
pub struct Cancel(
    Arc<AtomicBool>,
    #[cfg(test)] Arc<std::sync::Mutex<Option<Arc<RenderHook>>>>,
);
#[cfg(test)]
#[derive(Default)]
pub(crate) struct RenderHook {
    pub entered: AtomicBool,
}
impl Cancel {
    #[cfg(test)]
    pub(crate) fn pause_render(&self, hook: Arc<RenderHook>) {
        *self.1.lock().unwrap() = Some(hook);
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    /// Whether cancellation has been requested; implementations must still reap the worker.
    #[must_use]
    pub fn cancelled(&self) -> bool {
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
    /// The staged file is no longer the pinned one (SRC-15 slice 3): never retried.
    #[error("document changed")]
    DocumentChanged,
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
    inspect_with(path, None, cancel)
}

/// Inspect a staged file whose identity the caller pinned: the launch refuses (and the run
/// ends as) `DocumentChanged` if the file is not the pinned one before the sandbox starts or
/// after the worker exits.
/// # Errors
/// As for [`inspect_cancellable`], or `DocumentChanged`.
pub fn inspect_pinned(path: &Path, pins: &Pins, cancel: &Cancel) -> Result<Inspection, Reason> {
    inspect_with(path, Some(pins), cancel)
}

fn inspect_with(path: &Path, pins: Option<&Pins>, cancel: &Cancel) -> Result<Inspection, Reason> {
    inspect_run(path, pins, cancel, &Options::production()?)
}

/// [`inspect_with`] under the given options (the live large-document gate reads the run's
/// measurements back from them; production passes `Options::production()`).
pub(super) fn inspect_run(
    path: &Path,
    pins: Option<&Pins>,
    cancel: &Cancel,
    options: &Options,
) -> Result<Inspection, Reason> {
    super::availability::require()?;
    let spec = spec(records::Operation::Inspect, vec![]);
    execute_with(path, pins, &spec, cancel, options, |pipe| {
        let extension = pipe.wall_extension();
        records::read_inspection_observed(pipe, || {
            extension.fetch_add(INSPECT_WALL_PER_PAGE_MS, Ordering::AcqRel);
        })
    })
}

// The inspection budget scales with the input (SRC-15). The host large-input gate (small
// synthetic files, `docs/results.md`) used at most 1.6% of the CPU limit and 0.1% of the wall
// deadline for 500, 5,000 and 20,000 pages, so the constants stay as ceilings for hostile
// input. Before the first page record the wall is a fixed `INSPECT_WALL_BASE` whatever the
// file size (a long repair scan of a large damaged file can miss it). The CPU limit must be set before the page count is
// known (the first frame carries it) and `runner::worker_limits` refuses more than 200 s, so
// it is sized from the file size up to that ceiling. The wall deadline grows with progress:
// each page frame the parent has received and validated adds `INSPECT_WALL_PER_PAGE_MS`
// (never a lump sum on the claimed count, which a compromised worker could inflate), and the
// deadline remains the enforcing bound.
const INSPECT_WALL_BASE: Duration = Duration::from_secs(30);
const INSPECT_WALL_PER_PAGE_MS: u64 = 100;
const INSPECT_CPU_BASE: u64 = 20;
const INSPECT_CPU_CEILING: u64 = 200;
/// One more CPU second per this many bytes of PDF.
const INSPECT_CPU_BYTES: u64 = 32 << 20;

pub(super) fn inspect_cpu_seconds(size: u64) -> u64 {
    (INSPECT_CPU_BASE + size / INSPECT_CPU_BYTES).min(INSPECT_CPU_CEILING)
}

/// The wall budget an inspection of `pages` pages has earned by the time its last page arrives.
#[cfg(test)]
pub(super) fn inspect_wall_seconds(pages: u64) -> f64 {
    let millis = u32::try_from(pages * INSPECT_WALL_PER_PAGE_MS).unwrap_or(u32::MAX);
    INSPECT_WALL_BASE.as_secs_f64() + f64::from(millis) / 1000.
}

/// Yield individually validated pages with bounded memory on a dedicated reader thread.
/// A later failure fails the run; already delivered pages may be retained for a retry.
/// Callbacks must return promptly; cancellation stops the worker, not caller code.
/// # Errors
/// Invalid requests (including an empty page list: `Unsupported`), malformed worker output,
/// setup failures, cancellation or deadlines.
pub fn render(
    path: &Path,
    pages: Vec<Page>,
    cancel: &Cancel,
    page: impl FnMut(RenderedPage) -> Result<(), Error> + Send,
) -> Result<(), Reason> {
    render_with(path, None, pages, cancel, page)
}

/// [`render`] of a staged file whose identity the caller pinned (see [`inspect_pinned`]).
/// # Errors
/// As for [`render`], or `DocumentChanged`.
pub fn render_pinned(
    path: &Path,
    pins: &Pins,
    pages: Vec<Page>,
    cancel: &Cancel,
    page: impl FnMut(RenderedPage) -> Result<(), Error> + Send,
) -> Result<(), Reason> {
    render_with(path, Some(pins), pages, cancel, page)
}

fn render_with(
    path: &Path,
    pins: Option<&Pins>,
    pages: Vec<Page>,
    cancel: &Cancel,
    page: impl FnMut(RenderedPage) -> Result<(), Error> + Send,
) -> Result<(), Reason> {
    render_run(path, pins, pages, cancel, &Options::production()?, page)
}

/// [`render_with`] under the given options (see [`inspect_run`]).
pub(super) fn render_run(
    path: &Path,
    pins: Option<&Pins>,
    pages: Vec<Page>,
    cancel: &Cancel,
    options: &Options,
    mut page: impl FnMut(RenderedPage) -> Result<(), Error> + Send,
) -> Result<(), Reason> {
    // Validate geometry and request cardinality before spawning anything.
    records::RenderReader::new(std::io::empty(), pages.clone())?;
    super::availability::require()?;
    let spec = spec(
        records::Operation::Render,
        pages.iter().map(|p| p.n).collect(),
    );
    execute_with(path, pins, &spec, cancel, options, |pipe| {
        #[cfg(test)]
        let pipe = {
            let mut pipe = pipe;
            let mut first = Vec::new();
            let hook = cancel
                .1
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(hook) = hook {
                let mut byte = [0; 1];
                if pipe.read_exact(&mut byte).is_err() {
                    return (Err(Error::Unreadable), None);
                }
                first.extend(byte);
                hook.entered.store(true, Ordering::Release);
                let deadline = Instant::now() + Duration::from_secs(30);
                while !cancel.cancelled() {
                    if Instant::now() >= deadline {
                        return (Err(Error::Timeout), None);
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            std::io::Cursor::new(first).chain(pipe)
        };
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

/// Yield the validated words of one to eight pages, each with its box in the corrected page
/// space. Same sandbox, wall deadline (30 s a page) and cancellation as [`render`].
/// # Errors
/// Invalid requests, malformed worker output, setup failures, cancellation or deadlines.
pub fn words(
    path: &Path,
    pages: Vec<Page>,
    cancel: &Cancel,
    page: impl FnMut(PageWords) -> Result<(), Error> + Send,
) -> Result<(), Reason> {
    words_with(path, None, pages, cancel, page)
}

/// [`words`] of a file whose identity the caller pinned (see [`inspect_pinned`]).
/// # Errors
/// As for [`words`], or `DocumentChanged`.
pub fn words_pinned(
    path: &Path,
    pins: &Pins,
    pages: Vec<Page>,
    cancel: &Cancel,
    page: impl FnMut(PageWords) -> Result<(), Error> + Send,
) -> Result<(), Reason> {
    words_with(path, Some(pins), pages, cancel, page)
}

fn words_with(
    path: &Path,
    pins: Option<&Pins>,
    pages: Vec<Page>,
    cancel: &Cancel,
    mut page: impl FnMut(PageWords) -> Result<(), Error> + Send,
) -> Result<(), Reason> {
    records::WordsReader::new(std::io::empty(), pages.clone())?;
    super::availability::require()?;
    let spec = spec(
        records::Operation::Words,
        pages.iter().map(|p| p.n).collect(),
    );
    execute_with(path, pins, &spec, cancel, &Options::production()?, |pipe| {
        let mut reader = match records::WordsReader::new(pipe, pages) {
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
    pub tmp_bytes: Option<u64>,
    #[cfg(test)]
    pub observed_exit: Arc<std::sync::atomic::AtomicI32>,
    #[cfg(test)]
    pub peak_rss_kib: Arc<std::sync::atomic::AtomicU64>,
    #[cfg(test)]
    pub worker_peak_rss_kib: Arc<std::sync::atomic::AtomicU64>,
    #[cfg(test)]
    pub parent_killed: Arc<AtomicBool>,
    /// CPU milliseconds of the finished run's tree, from the runner's `wait4` rusage.
    #[cfg(test)]
    pub cpu_ms: Arc<std::sync::atomic::AtomicU64>,
    pub runner: RunnerCommand,
    pub executable: PathBuf,
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub wall: Option<Duration>,
    pub cpu: Option<u64>,
}
impl Options {
    #[cfg(not(test))]
    fn observe(&self, outcome: &Outcome) {
        let _ = (
            self,
            outcome.wait4_rss_kib,
            outcome.worker_rss_kib,
            outcome.wait4_cpu_ms,
            outcome.parent_killed,
        );
    }

    #[cfg(test)]
    fn observe(&self, outcome: &Outcome) {
        self.peak_rss_kib
            .store(outcome.wait4_rss_kib, Ordering::Release);
        self.worker_peak_rss_kib
            .store(outcome.worker_rss_kib, Ordering::Release);
        self.cpu_ms.store(outcome.wait4_cpu_ms, Ordering::Release);
        self.parent_killed
            .store(outcome.parent_killed, Ordering::Release);
        self.observed_exit
            .store(outcome.exit.unwrap_or(-1), Ordering::Release);
    }

    pub(super) fn production() -> Result<Self, Reason> {
        let runner = RunnerCommand::mimik().map_err(|_| unavailable("runner executable"))?;
        // The libtest executable has no `pdf-worker` CLI. The host live harness supplies
        // the built application as runner; all production probe/inspect/render paths remain.
        #[cfg(test)]
        let runner =
            std::env::var_os("MIMIK_LIVE_RUNNER").map_or(runner, |program| RunnerCommand {
                program: program.into(),
                args: vec![
                    "assistant-runner".into(),
                    "--spec-fd".into(),
                    runner::SPEC_FD.to_string().into(),
                ],
                env: vec![],
            });
        Ok(Self {
            #[cfg(test)]
            tmp_bytes: None,
            #[cfg(test)]
            observed_exit: Arc::default(),
            #[cfg(test)]
            peak_rss_kib: Arc::default(),
            #[cfg(test)]
            worker_peak_rss_kib: Arc::default(),
            #[cfg(test)]
            parent_killed: Arc::default(),
            #[cfg(test)]
            cpu_ms: Arc::default(),
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
    eprintln!("upload unavailable: {reason}");
    Reason::Unavailable(reason.into())
}

pub(super) fn probe() -> Result<Inspection, Reason> {
    probe_with(&Options::production()?)
}

pub(super) fn probe_with(options: &Options) -> Result<Inspection, Reason> {
    use std::io::Write;
    let root = Root::new().map_err(|_| unavailable("probe directory"))?;
    let path = root.0.join("probe.pdf");
    File::options()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .and_then(|mut file| file.write_all(super::availability::PDF))
        .map_err(|_| unavailable("probe file"))?;
    execute(
        &path,
        &spec(records::Operation::Inspect, vec![]),
        &Cancel::default(),
        options,
        records::read_inspection_tracked,
    )
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

/// Open a file for the sandbox: its descriptor, canonical name and identity. With `pins` (a
/// staged upload) the final component is never followed and the descriptor's `fstat` must equal
/// the pins, else `DocumentChanged`; the identity handed to the sandbox's bind check is then the
/// pinned one, so a name that resolves to something else is refused after the mounts.
fn open(path: &Path, pins: Option<&Pins>) -> Result<(File, PathBuf, sandbox::Identity), Reason> {
    let follow = if pins.is_some() { libc::O_NOFOLLOW } else { 0 };
    let file = match File::options()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY | follow)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if pins.is_some() && error.raw_os_error() == Some(libc::ELOOP) => {
            return Err(Reason::DocumentChanged);
        }
        Err(_) => return Err(Error::Unreadable.into()),
    };
    let metadata = file.metadata().map_err(|_| Error::Unreadable)?;
    match pins {
        Some(pins) if !pins.holds(&metadata) => return Err(Reason::DocumentChanged),
        None if !metadata.is_file() => return Err(Error::Unreadable.into()),
        _ => {}
    }
    let identity = pins.map_or_else(|| (metadata.dev(), metadata.ino()), Pins::identity);
    Ok((
        file,
        std::fs::canonicalize(path).map_err(|_| Error::Unreadable)?,
        identity,
    ))
}

fn loader(file: &File) -> Result<PathBuf, Reason> {
    use object::{
        Endianness,
        read::elf::{FileHeader, ProgramHeader},
    };
    // ReadCache requests just the ELF/program headers and interpreter, not the text,
    // debug symbols or MuPDF's embedded resources (hundreds of MiB in debug builds).
    let cache = object::read::ReadCache::new(file);
    let bytes = &cache;
    let elf = object::elf::FileHeader64::<Endianness>::parse(bytes)
        .map_err(|_| unavailable("ELF interpreter"))?;
    let endian = elf.endian().map_err(|_| unavailable("ELF interpreter"))?;
    let headers = elf
        .program_headers(endian, bytes)
        .map_err(|_| unavailable("ELF interpreter"))?;
    for header in headers {
        if header.p_type(endian) == object::elf::PT_INTERP {
            let data = header
                .data(endian, bytes)
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
    unverified: Option<Watched>,
    settled: bool,
    reaped: Option<std::process::ExitStatus>,
    peak_rss_kib: u64,
    worker_peak_rss_kib: u64,
    /// User plus system CPU of the runner and the descendants it waited for (`wait4`).
    cpu_ms: u64,
}
impl Tree {
    fn new(child: std::process::Child, pin: Watched) -> Self {
        Self {
            child,
            pins: vec![pin],
            unverified: None,
            settled: false,
            reaped: None,
            peak_rss_kib: 0,
            worker_peak_rss_kib: 0,
            cpu_ms: 0,
        }
    }
    fn sample_inner(&mut self) {
        // Only the verified third pin is the worker; never sample an unverified PID.
        if let Some(rss) = self.pins.get(2).and_then(worker_rss_kib) {
            self.worker_peak_rss_kib = self.worker_peak_rss_kib.max(rss);
        }
    }
    fn kill(&mut self) {
        self.sample_inner();
        // Signal inner, reaper and runner, in that order. Never leave the runner waiting
        // on an acknowledgement or on a child whose report has not arrived yet.
        for pin in self.pins.iter().rev() {
            let _ = rustix::process::pidfd_send_signal(&pin.pidfd, rustix::process::Signal::KILL);
        }
    }
    fn settle(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.settle_within(Duration::from_secs(5))
    }
    fn settle_within(&mut self, within: Duration) -> std::io::Result<std::process::ExitStatus> {
        self.settled = true;
        self.kill();
        let deadline = Instant::now() + within;
        loop {
            let status = self.reap()?;
            if let Some(status) = status
                && self.pins.iter().all(|pin| !pin.alive())
                && !self.unverified.as_ref().is_some_and(Watched::alive)
            {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "worker cleanup",
                ));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn reap(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        use std::os::unix::process::ExitStatusExt;
        if self.reaped.is_none() {
            let mut status = 0;
            let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
            // SAFETY: wait4 writes status/rusage only on successful reaping of our child.
            // This Tree exclusively owns the Child and never calls Child::wait/try_wait.
            let result = unsafe {
                libc::wait4(
                    i32::try_from(self.child.id()).expect("PID fits i32"),
                    &raw mut status,
                    libc::WNOHANG,
                    usage.as_mut_ptr(),
                )
            };
            if result < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(error);
                }
            } else if result > 0 {
                // SAFETY: successful wait4 initialized usage.
                let usage = unsafe { usage.assume_init() };
                self.peak_rss_kib = u64::try_from(usage.ru_maxrss).unwrap_or(u64::MAX);
                let millis = |time: libc::timeval| {
                    u64::try_from(time.tv_sec).unwrap_or(0) * 1000
                        + u64::try_from(time.tv_usec).unwrap_or(0) / 1000
                };
                self.cpu_ms = millis(usage.ru_utime) + millis(usage.ru_stime);
                self.reaped = Some(std::process::ExitStatus::from_raw(status));
            }
        }
        Ok(self.reaped)
    }
}

fn worker_rss_kib(pin: &Watched) -> Option<u64> {
    if !pin.alive() {
        return None;
    }
    let text = std::fs::read_to_string(format!("/proc/{}/status", pin.pid)).ok()?;
    // A live pidfd on both sides of the read excludes exit/reuse of the numeric PID.
    // Once it reports exit we cannot trust /proc's numeric path, even if it exists.
    if !pin.alive() {
        return None;
    }
    text.lines().find_map(|line| {
        let mut fields = line.strip_prefix("VmHWM:")?.split_whitespace();
        let value = fields.next()?.parse().ok()?;
        (fields.next()? == "kB" && fields.next().is_none()).then_some(value)
    })
}
impl Drop for Tree {
    fn drop(&mut self) {
        self.kill();
        if !self.settled {
            let _ = self.settle();
        }
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
        process::Inner::Unverified(inner) => {
            tree.unverified = Some(inner);
            return Err(std::io::Error::other("inner identity"));
        }
    }
    Ok(())
}

fn prepare(
    path: &Path,
    pins: Option<&Pins>,
    spec: &Spec,
    options: &Options,
) -> Result<(Root, File, File, sandbox::Profile), Reason> {
    sandbox::host::namespaces(Path::new("/proc")).map_err(|e| unavailable(&e.to_string()))?;
    let root = Root::new().map_err(|_| unavailable("private root"))?;
    let (input_file, input, identity) = open(path, pins)?;
    let (exe, executable, exe_identity) = open(&options.executable, None)?;
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
    #[cfg(test)]
    let plan = {
        let mut plan = plan;
        if let Some(bytes) = options.tmp_bytes {
            plan.tmpfs
                .iter_mut()
                .find(|mount| mount.target == "/tmp")
                .unwrap()
                .bytes = bytes;
        }
        plan
    };
    Ok((root, input_file, exe, plan))
}

/// Counts bytes independently of the parser, distinguishing pre-framing exit statuses
/// from malformed output that happens to accompany a typed exit status.
pub(super) struct Output {
    pipe: Interruptible<std::process::ChildStdout>,
    bytes: Arc<AtomicUsize>,
    /// Milliseconds granted to the wall deadline for progress (inspection only).
    extension: Arc<AtomicU64>,
}
impl Output {
    /// The handle through which the reader extends the wall deadline.
    pub(super) fn wall_extension(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.extension)
    }
}

// Even an unkillable pin must not turn cleanup into an unbounded reader join.
struct Interruptible<R> {
    pipe: R,
    stop: Arc<AtomicBool>,
}
impl<R> Interruptible<R> {
    fn new(pipe: R, stop: &Arc<AtomicBool>) -> Self {
        Self {
            pipe,
            stop: Arc::clone(stop),
        }
    }
}
impl<R: Read + AsFd> Read for Interruptible<R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        while !self.stop.load(Ordering::Acquire) {
            let mut fds = [rustix::event::PollFd::new(
                &self.pipe,
                rustix::event::PollFlags::IN,
            )];
            let timeout = rustix::time::Timespec {
                tv_sec: 0,
                tv_nsec: 50_000_000,
            };
            match rustix::event::poll(&mut fds, Some(&timeout)) {
                Ok(0) | Err(rustix::io::Errno::INTR) => {}
                Ok(_) => return self.pipe.read(bytes),
                Err(error) => return Err(error.into()),
            }
        }
        Ok(0)
    }
}
impl Read for Output {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let size = self.pipe.read(buffer)?;
        self.bytes.fetch_add(size, Ordering::Relaxed);
        Ok(size)
    }
}

fn read_output<T>(
    output: Output,
    read: impl FnOnce(Output) -> (Result<T, Error>, Option<Error>),
    abort: &AtomicBool,
) -> (Result<T, Error>, Option<Error>, bool) {
    // A caller callback that panics must not leave the watchdog waiting for the wall
    // deadline. It still owns the process tree until all pins are gone.
    struct PanicAbort<'a>(&'a AtomicBool);
    impl Drop for PanicAbort<'_> {
        fn drop(&mut self) {
            if std::thread::panicking() {
                self.0.store(true, Ordering::Release);
            }
        }
    }
    let _panic = PanicAbort(abort);
    let bytes = Arc::clone(&output.bytes);
    let (result, terminal) = read(output);
    let empty = bytes.load(Ordering::Relaxed) == 0;
    if result.is_err() && terminal.is_none() && !empty {
        abort.store(true, Ordering::Release);
    }
    (result, terminal, empty)
}

/// Wall deadline and CPU seconds of one run: an explicit option wins, otherwise inspection
/// uses its base wall and a file-size CPU budget, and other operations 30 s and 25 s a page.
fn budgets(spec: &Spec, options: &Options, size: u64) -> Result<(Duration, u64), Reason> {
    let pages = u64::try_from(spec.pages.len()).map_err(|_| Error::Unsupported)?;
    let inspecting = spec.operation == records::Operation::Inspect;
    let wall = options.wall.unwrap_or(if inspecting {
        INSPECT_WALL_BASE
    } else {
        Duration::from_secs(30 * pages)
    });
    let cpu_seconds = options.cpu.unwrap_or(if inspecting {
        inspect_cpu_seconds(size)
    } else {
        25 * pages
    });
    Ok((wall, cpu_seconds))
}

/// The runner's request for one worker: descriptors 4, 12 and 13 carry the status, marker and
/// acknowledgement pipes.
fn runner_spec(plan: sandbox::Profile, cpu_seconds: u64) -> runner::Spec {
    runner::Spec {
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
    }
}

pub(super) fn execute<T: Send>(
    path: &Path,
    spec: &Spec,
    cancel: &Cancel,
    options: &Options,
    read: impl FnOnce(Output) -> (Result<T, Error>, Option<Error>) + Send,
) -> Result<T, Reason> {
    execute_with(path, None, spec, cancel, options, read)
}

/// One worker run. With `pins` the launch descriptor must equal them before the profile is
/// built, and after the worker has exited (whatever its result: it reads the file lazily for
/// the whole run) the same descriptor must still equal them, else `DocumentChanged`.
pub(super) fn execute_with<T: Send>(
    path: &Path,
    pins: Option<&Pins>,
    spec: &Spec,
    cancel: &Cancel,
    options: &Options,
    read: impl FnOnce(Output) -> (Result<T, Error>, Option<Error>) + Send,
) -> Result<T, Reason> {
    if cancel.cancelled() {
        return Err(Reason::Cancelled);
    }
    let (_root, input, _exe, plan) = prepare(path, pins, spec, options)?;
    let size = input.metadata().map_or(0, |meta| meta.len());
    let (wall, cpu_seconds) = budgets(spec, options, size)?;
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
    let runner_spec = runner_spec(plan, cpu_seconds);
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
    let tree = Tree::new(child, runner_pin);
    let stop = Arc::new(AtomicBool::new(false));
    let stdout = Output {
        pipe: Interruptible::new(stdout, &stop),
        bytes: Arc::new(AtomicUsize::new(0)),
        extension: Arc::new(AtomicU64::new(0)),
    };
    let extension = stdout.wall_extension();
    // An explicit wall (tests) is exact: progress never extends it.
    let no_extension = AtomicU64::new(0);
    let granted: &AtomicU64 = if options.wall.is_some() {
        &no_extension
    } else {
        &extension
    };
    let abort = AtomicBool::new(false);
    let stderr = Interruptible::new(stderr, &stop);
    let marker = Interruptible::new(File::from(marker), &stop);
    let status = Interruptible::new(File::from(status), &stop);
    let result = std::thread::scope(|scope| {
        let err = scope.spawn(move || drain_stderr(stderr));
        let marker = scope.spawn(move || reached_exec(marker));
        let (tx, rx) = mpsc::channel();
        let status = scope.spawn(move || {
            let _ = tx.send(read_started(status));
        });
        let abort_ref = &abort;
        let reader = scope.spawn(move || read_output(stdout, read, abort_ref));
        let outcome = watch(tree, &rx, ack, started + wall, granted, cancel, &abort);
        options.observe(&outcome);
        let Outcome { exit, parent, .. } = outcome;
        if matches!(&parent, Some(Reason::Unavailable(reason)) if reason == "process cleanup timed out")
        {
            stop.store(true, Ordering::Release);
        }
        let (result, terminal, empty) = reader.join().map_err(|_| unavailable("record reader"))?;
        let _ = status.join();
        let reached_exec = marker.join().unwrap_or(false);
        let _ = err.join();
        reconcile_output(exit, reached_exec, result, terminal, empty, parent)
    });
    // The worker is gone (or killed). A changed file outranks every other outcome.
    if let Some(pins) = pins
        && !input.metadata().is_ok_and(|meta| pins.holds(&meta))
    {
        return Err(Reason::DocumentChanged);
    }
    result
}

fn read_started(status: impl Read) -> Option<Started> {
    let mut line = String::new();
    std::io::BufReader::new(status.take(4096))
        .read_line(&mut line)
        .ok()
        .and_then(|_| serde_json::from_str(&line).ok())
}

fn reconcile_output<T>(
    exit: Option<i32>,
    marker: bool,
    result: Result<T, Error>,
    terminal: Option<Error>,
    empty: bool,
    parent: Option<Reason>,
) -> Result<T, Reason> {
    // Local payload/callback failures retain their identity after cleanup. Worker
    // terminal errors still require a matching process exit; empty output may use
    // the pre-framing typed exit codes instead.
    if marker && parent.is_none() && terminal.is_none() && !empty && result.is_err() {
        return result.map_err(Reason::from);
    }
    reconcile(exit, marker, result.is_ok(), terminal, empty, parent)?;
    result.map_err(Reason::from)
}

fn drain_stderr(mut stderr: impl Read) {
    // Cap matching work at 64 KiB, but keep draining/discarding until EOF.
    sandbox::drain::drain((&mut stderr).take(64 << 10), |_| {});
    let _ = std::io::copy(&mut stderr, &mut std::io::sink());
}

/// Helper-level check of the interruptible stderr drain and exit mapping.
/// Spawns a shell directly; does not exercise the full sandboxed runner launch path.
#[cfg(test)]
pub(crate) fn stderr_fixture(success: bool) -> Result<(), Reason> {
    use std::process::{Command, Stdio};
    let mut child = Command::new("/bin/sh")
        .args(["-c", "i=0; while [ $i -lt 3000 ]; do printf 'WORKER-STDERR-PLANTED-MARKER\\n' >&2; i=$((i+1)); done; exit \"$1\"", "fixture", if success { "0" } else { "1" }])
        .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stderr = Interruptible::new(child.stderr.take().unwrap(), &stop);
    std::thread::scope(|scope| {
        let drain = scope.spawn(move || drain_stderr(stderr));
        let status = child.wait().unwrap();
        drain.join().unwrap();
        reconcile_output(status.code(), true, Ok(()), None, false, None)
    })
}

fn reached_exec(marker: impl Read) -> bool {
    let mut bytes = Vec::new();
    marker.take(129).read_to_end(&mut bytes).is_ok()
        && bytes
            .strip_prefix(b"exec\n")
            .and_then(sandbox::Landlock::parse)
            .is_some()
}

struct Outcome {
    exit: Option<i32>,
    parent: Option<Reason>,
    wait4_rss_kib: u64,
    worker_rss_kib: u64,
    wait4_cpu_ms: u64,
    parent_killed: bool,
}

fn watch(
    mut tree: Tree,
    report: &mpsc::Receiver<Option<Started>>,
    ack: OwnedFd,
    deadline: Instant,
    extension: &AtomicU64,
    cancel: &Cancel,
    abort: &AtomicBool,
) -> Outcome {
    let mut ack = Some(ack);
    let mut parent = None;
    let mut aborted_at = None;
    let mut parent_killed = false;
    loop {
        // Pin an already-pending report before any cancellation/abort decision.
        if ack.is_some() {
            match report.try_recv() {
                Ok(Some(report)) => {
                    if pin(&mut tree, report).is_err() {
                        parent_killed = true;
                        parent = Some(unavailable("process identity"));
                        drop(ack.take());
                        break;
                    }
                    if let Some(fd) = ack.take() {
                        let _ = rustix::io::write(&fd, b"1");
                    }
                }
                Ok(None) | Err(mpsc::TryRecvError::Disconnected) => drop(ack.take()),
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        tree.sample_inner();
        // Observe natural exit before treating a reader's terminal error as an abort.
        if tree.pins[0].alive() {
            if cancel.cancelled() {
                parent = Some(Reason::Cancelled);
            } else if Instant::now()
                >= deadline + Duration::from_millis(extension.load(Ordering::Acquire))
            {
                parent = Some(Error::Timeout.into());
            }
            let malformed = abort.load(Ordering::Acquire)
                && aborted_at.get_or_insert_with(Instant::now).elapsed()
                    >= Duration::from_millis(100);
            if parent.is_some() || malformed {
                parent_killed = true;
                tree.kill();
                break;
            }
        } else {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // A completed error frame may precede the matching exit by a scheduling instant.
    // On parse failure killing is correct, but preserve typed terminal errors by waiting
    // for EOF/natural exit in the reader path (the reader has already observed EOF).
    drop(ack);
    let exit = if let Ok(status) = tree.settle() {
        use std::os::unix::process::ExitStatusExt;
        status
            .code()
            // Negative values retain a runner signal independently of real exit codes.
            .or_else(|| status.signal().map(|signal| -signal))
    } else {
        parent = Some(unavailable("process cleanup timed out"));
        None
    };
    Outcome {
        exit,
        parent,
        wait4_rss_kib: tree.peak_rss_kib,
        worker_rss_kib: tree.worker_peak_rss_kib,
        wait4_cpu_ms: tree.cpu_ms,
        parent_killed,
    }
}

pub(super) fn reconcile(
    exit: Option<i32>,
    marker: bool,
    done: bool,
    terminal: Option<Error>,
    empty: bool,
    parent: Option<Reason>,
) -> Result<(), Reason> {
    if let Some(reason) = parent {
        return Err(reason);
    }
    if !marker {
        let step = match exit {
            Some(sandbox::exit::SESSION) => Some("session"),
            Some(sandbox::exit::IDENTITY) => Some("identity"),
            Some(sandbox::exit::CAPABILITIES) => Some("capabilities"),
            Some(sandbox::exit::SECCOMP) => Some("seccomp"),
            Some(sandbox::exit::MARKER) => Some("marker"),
            Some(sandbox::exit::DESCRIPTORS) => Some("descriptors"),
            Some(sandbox::exit::MARKER_DESCRIPTOR) => Some("marker-descriptor"),
            Some(sandbox::exit::STDIN) => Some("stdin"),
            Some(sandbox::exit::LANDLOCK) => Some("landlock"),
            Some(code) => sandbox::setup_failure_step(code),
            None => None,
        };
        return Err(unavailable(&step.map_or_else(
            || {
                exit.map_or_else(
                    || "sandbox runner terminated by signal".into(),
                    |code| {
                        if code < 0 {
                            format!("sandbox runner terminated by signal {}", -code)
                        } else {
                            format!("sandbox runner exit {code}")
                        }
                    },
                )
            },
            |name| format!("sandbox step {name} failed"),
        )));
    }
    if done && terminal.is_none() && exit == Some(0) {
        return Ok(());
    }
    if !done
        && let Some(code) = exit
        && let Some(reason) = Error::from_exit_code(code)
        && (terminal.is_some() || empty)
        && terminal.is_none_or(|error| error.exit_code() == code)
    {
        return Err(reason.into());
    }
    Err(Error::Unreadable.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn pinned_file(dir: &Path) -> (PathBuf, Pins) {
        let path = dir.join("document.pdf");
        std::fs::write(&path, b"%PDF-1.7\n%%EOF\n").unwrap();
        let pins = Pins::of(&std::fs::metadata(&path).unwrap());
        (path, pins)
    }

    #[test]
    fn the_launch_descriptor_must_equal_the_pins_and_hands_their_identity_to_the_bind() {
        let dir = tempfile::tempdir().unwrap();
        let (path, pins) = pinned_file(dir.path());
        let (_file, canonical, identity) = open(&path, Some(&pins)).unwrap();
        assert_eq!(canonical, std::fs::canonicalize(&path).unwrap());
        assert_eq!(identity, pins.identity());
        // Any change since the pins were taken (here an append) is refused before the profile.
        std::thread::sleep(Duration::from_millis(20));
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        std::io::Write::write_all(&mut file, b"x").unwrap();
        assert_eq!(
            open(&path, Some(&pins)).err(),
            Some(Reason::DocumentChanged)
        );
    }

    #[test]
    fn a_replacement_by_rename_or_a_symlink_at_the_final_component_is_document_changed() {
        let dir = tempfile::tempdir().unwrap();
        let (path, pins) = pinned_file(dir.path());
        let other = dir.path().join("other.pdf");
        std::fs::write(&other, b"%PDF-1.7\n%%EOF\n").unwrap();
        std::fs::rename(&other, &path).unwrap();
        assert_eq!(
            open(&path, Some(&pins)).err(),
            Some(Reason::DocumentChanged),
            "a different inode"
        );
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let (path, pins) = pinned_file(&sub);
        let target = dir.path().join("target.pdf");
        std::fs::rename(&path, &target).unwrap();
        symlink(&target, &path).unwrap();
        assert_eq!(
            open(&path, Some(&pins)).err(),
            Some(Reason::DocumentChanged),
            "O_NOFOLLOW refuses the link"
        );
        // Without pins (the CLI) a link is still followed, as before.
        assert!(open(&path, None).is_ok());
    }

    #[test]
    fn worker_hwm_requires_a_live_pin_even_if_numeric_proc_path_exists() {
        let me = i32::try_from(std::process::id()).unwrap();
        let live = Watched::open(me).unwrap();
        assert!(worker_rss_kib(&live).unwrap() > 0);
        let mut child = std::process::Command::new("/bin/true").spawn().unwrap();
        let mut dead = Watched::open(i32::try_from(child.id()).unwrap()).unwrap();
        child.wait().unwrap();
        // Model a numeric PID reused by a live process: the original pidfd is dead.
        dead.pid = me;
        assert_eq!(worker_rss_kib(&dead), None);
    }
    #[test]
    fn pre_marker_signal_is_distinct_from_exit_137() {
        for (script, expected) in [
            ("kill -KILL $$", "sandbox runner terminated by signal 9"),
            ("exit 137", "sandbox runner exit 137"),
        ] {
            let child = std::process::Command::new("/bin/sh")
                .args(["-c", script])
                .spawn()
                .unwrap();
            let pin = Watched::open(i32::try_from(child.id()).unwrap()).unwrap();
            let tree = Tree::new(child, pin);
            let (_tx, rx) = mpsc::channel();
            let (_read, ack) = rustix::pipe::pipe().unwrap();
            let Outcome { exit, parent, .. } = watch(
                tree,
                &rx,
                ack,
                Instant::now() + Duration::from_secs(2),
                &AtomicU64::new(0),
                &Cancel::default(),
                &AtomicBool::new(false),
            );
            assert_eq!(
                reconcile(exit, false, false, None, true, parent),
                Err(Reason::Unavailable(expected.into()))
            );
        }
    }
    #[test]
    fn inspection_budget_scales_with_the_input_within_the_runner_ceiling() {
        assert_eq!(inspect_cpu_seconds(0), 20);
        assert_eq!(inspect_cpu_seconds(64 << 20), 22);
        assert_eq!(inspect_cpu_seconds(u64::MAX), 200);
    }
    #[test]
    fn validated_pages_extend_the_wall_deadline_and_nothing_else_does() {
        let run = |extension: u64, cancel_after: Option<Duration>| {
            let child = std::process::Command::new("/bin/sh")
                .args(["-c", "sleep 30"])
                .spawn()
                .unwrap();
            let pin = Watched::open(i32::try_from(child.id()).unwrap()).unwrap();
            let tree = Tree::new(child, pin);
            let (_tx, rx) = mpsc::channel();
            let (_read, ack) = rustix::pipe::pipe().unwrap();
            let cancel = Cancel::default();
            let canceller = cancel.clone();
            let stopper = cancel_after.map(|after| {
                std::thread::spawn(move || {
                    std::thread::sleep(after);
                    canceller.cancel();
                })
            });
            let Outcome { parent, .. } = watch(
                tree,
                &rx,
                ack,
                Instant::now() + Duration::from_millis(200),
                &AtomicU64::new(extension),
                &cancel,
                &AtomicBool::new(false),
            );
            if let Some(stopper) = stopper {
                stopper.join().unwrap();
            }
            parent
        };
        assert_eq!(run(0, None), Some(Error::Timeout.into()));
        // The deadline is 200 ms; with 60 s added, a cancel at 700 ms must win.
        assert_eq!(
            run(60_000, Some(Duration::from_millis(700))),
            Some(Reason::Cancelled)
        );
    }
    #[test]
    fn invalid_render_is_rejected_before_availability_probe() {
        assert_eq!(
            render(
                Path::new("/does-not-exist"),
                vec![],
                &Cancel::default(),
                |_| Ok(())
            ),
            Err(Error::Unsupported.into())
        );
    }

    #[test]
    fn a_local_reader_or_callback_error_is_preserved_after_natural_exit() {
        for error in [Error::TooLarge, Error::Unsupported, Error::TooManyPages] {
            assert_eq!(
                reconcile_output::<()>(Some(0), true, Err(error), None, false, None),
                Err(error.into())
            );
        }
    }

    #[test]
    fn cleanup_interrupts_a_reader_even_if_a_writer_remains_open() {
        let (read, _write) = rustix::pipe::pipe().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let mut reader = Interruptible::new(File::from(read), &stop);
        std::thread::scope(|scope| {
            let read = scope.spawn(move || reader.read(&mut [0]));
            std::thread::sleep(Duration::from_millis(20));
            stop.store(true, Ordering::Release);
            assert_eq!(read.join().unwrap().unwrap(), 0);
        });
    }

    #[test]
    fn settle_kills_runner_and_every_pin_before_waiting() {
        let child = std::process::Command::new("/bin/sleep")
            .arg("1")
            .spawn()
            .unwrap();
        let mut other = std::process::Command::new("/bin/sleep")
            .arg("1")
            .spawn()
            .unwrap();
        let pins = [&child, &other].map(|c| Watched::open(i32::try_from(c.id()).unwrap()).unwrap());
        let mut tree = Tree {
            child,
            pins: pins.into(),
            unverified: None,
            settled: false,
            reaped: None,
            peak_rss_kib: 0,
            worker_peak_rss_kib: 0,
            cpu_ms: 0,
        };
        let start = Instant::now();
        let result = tree.settle();
        let _ = other.kill();
        let _ = other.wait();
        assert!(result.is_ok());
        assert!(start.elapsed() < Duration::from_millis(400));
    }

    #[test]
    fn an_unverified_pin_bounds_cleanup_without_authorizing_a_signal() {
        let child = std::process::Command::new("/bin/true").spawn().unwrap();
        let mut other = std::process::Command::new("/bin/sleep")
            .arg("10")
            .spawn()
            .unwrap();
        let unverified = Some(Watched::open(i32::try_from(other.id()).unwrap()).unwrap());
        let mut tree = Tree {
            child,
            pins: vec![],
            unverified,
            settled: false,
            reaped: None,
            peak_rss_kib: 0,
            worker_peak_rss_kib: 0,
            cpu_ms: 0,
        };
        let error = tree.settle_within(Duration::from_millis(20)).unwrap_err();
        let alive = other.try_wait().unwrap().is_none();
        let start = Instant::now();
        drop(tree);
        let drop_elapsed = start.elapsed();
        other.kill().unwrap();
        other.wait().unwrap();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(alive);
        assert!(
            drop_elapsed < Duration::from_millis(100),
            "drop settled twice: {drop_elapsed:?}"
        );
    }
    #[test]
    fn parent_abort_before_marker_keeps_its_reason() {
        for reason in [Reason::Cancelled, Error::Timeout.into()] {
            assert_eq!(
                reconcile(None, false, false, None, true, Some(reason.clone())),
                Err(reason)
            );
        }
    }
    #[test]
    fn contradictory_done_and_error_is_never_success() {
        assert_eq!(
            reconcile(Some(0), true, true, Some(Error::TooLarge), false, None),
            Err(Error::Unreadable.into())
        );
    }

    #[test]
    fn malformed_output_cannot_borrow_a_typed_exit_reason() {
        for code in 20..=25 {
            assert_eq!(
                reconcile(Some(code), true, false, None, false, None),
                Err(Error::Unreadable.into())
            );
            assert_eq!(
                reconcile(Some(code), true, false, None, true, None),
                Err(Error::from_exit_code(code).unwrap().into())
            );
        }
        assert_eq!(
            reconcile(
                Some(super::super::filter::INSTALL_FAILED),
                true,
                false,
                None,
                true,
                None
            ),
            Err(Error::Unreadable.into())
        );
    }

    #[test]
    fn natural_runner_exit_still_stops_pinned_processes() {
        let child = std::process::Command::new("/bin/true").spawn().unwrap();
        let mut leftover = std::process::Command::new("/bin/sleep")
            .arg("0.5")
            .spawn()
            .unwrap();
        let pin = Watched::open(i32::try_from(leftover.id()).unwrap()).unwrap();
        let mut tree = Tree {
            child,
            pins: vec![pin],
            unverified: None,
            settled: false,
            reaped: None,
            peak_rss_kib: 0,
            worker_peak_rss_kib: 0,
            cpu_ms: 0,
        };
        let started = Instant::now();
        tree.settle().unwrap();
        let _ = leftover.wait();
        assert!(started.elapsed() < Duration::from_millis(400));
    }
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
                    let actual = reconcile(Some(code), true, done, terminal, true, None);
                    let expected = if code == 0 && done && terminal.is_none() {
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
                        reconcile(Some(code), false, done, terminal, true, None),
                        Err(Reason::Unavailable(_))
                    ));
                }
            }
        }
        assert_eq!(
            reconcile(None, true, true, None, false, None),
            Err(Error::Unreadable.into())
        );
        assert_eq!(
            reconcile(None, true, false, None, false, Some(Error::Timeout.into())),
            Err(Error::Timeout.into())
        );
        assert_eq!(
            reconcile(None, true, false, None, false, Some(Reason::Cancelled)),
            Err(Reason::Cancelled)
        );
    }
}
