//! `<data dir>/run/` (assisted sheet plan §1.4a, "Confinement levels", "Generations"): the
//! pidfile under an exclusive `flock`, so a second Mimik on the same data directory cannot run
//! the assistant, and one 0700 directory per generation (`run/<generation>/`) holding the
//! level-1 socket or the TCP levels' token file.
use super::files::{self, Owners, Refusal};
use rustix::fs::{AtFlags, FileType, FlockOperation, Mode, OFlags, ResolveFlags};
use std::io::{Read, Seek, Write};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::Path;

/// The pidfile's name in `run/`.
pub(crate) const PIDFILE: &str = "assistant.pid";

/// `run/` itself, created 0700 when missing and checked.
pub(crate) fn open(data_dir: &Path) -> Result<OwnedFd, Refusal> {
    let path = data_dir.join("run");
    match rustix::fs::mkdir(&path, Mode::from_raw_mode(0o700)) {
        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
        Err(_) => return Err(Refusal::Io(path.display().to_string())),
    }
    files::open_dir(&path, Owners::Mine, true)
}

/// What the pidfile records of the running child.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub generation: u64,
    pub pid: i32,
    /// The process's start time (`/proc/<pid>/stat` field 22), so a reused PID is told apart.
    pub start_time: u64,
    /// What `/proc/<pid>/exe` may show: the runner (Mimik itself, before its exec) or the
    /// command it executes (bwrap, or llama-server on the TCP levels).
    pub executables: Vec<String>,
}

impl Entry {
    /// The entry's lines, then `sha256 <hex>` of them: a torn write never parses.
    fn text(&self) -> String {
        let mut text = format!("{}\n{}\n{}\n", self.generation, self.pid, self.start_time);
        for executable in &self.executables {
            text.push_str(executable);
            text.push('\n');
        }
        let sum = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(text.as_bytes()));
        text.push_str("sha256 ");
        text.push_str(&sum);
        text.push('\n');
        text
    }

    /// The entry the text begins with, when its checksum line matches (anything after it,
    /// left by a longer earlier entry before the truncate, is ignored).
    fn parse(text: &str) -> Option<Self> {
        // The checksum line ends the entry, but a longer earlier entry may leave bytes after
        // it. Each `sha256 ` line is a candidate terminator; the body before it that hashes to
        // its hex is the entry (a path line that happened to read `sha256 …` cannot forge the
        // hash of the bytes before itself).
        let mut offset = 0;
        let mut body = None;
        for line in text.split_inclusive('\n') {
            let start = offset;
            offset += line.len();
            let Some(hex_sum) = line.trim_end().strip_prefix("sha256 ") else {
                continue;
            };
            let candidate = &text[..start];
            if hex::encode(<sha2::Sha256 as sha2::Digest>::digest(candidate.as_bytes())) == hex_sum
            {
                body = Some(candidate);
                break;
            }
        }
        let text = body?;
        let mut lines = text.lines();
        let generation = lines.next()?.parse().ok()?;
        let pid = lines.next()?.parse().ok()?;
        let start_time = lines.next()?.parse().ok()?;
        let executables: Vec<String> = lines.map(str::to_owned).collect();
        (!executables.is_empty()).then_some(Self {
            generation,
            pid,
            start_time,
            executables,
        })
    }

    /// The entry for a running process, when `/proc` can tell its start time.
    pub(crate) fn of(generation: u64, pid: i32, executables: Vec<String>) -> Option<Self> {
        Some(Self {
            generation,
            pid,
            start_time: start_time(pid)?,
            executables,
        })
    }
}

/// A process's start time in clock ticks since boot.
pub(crate) fn start_time(pid: i32) -> Option<u64> {
    procfs::process::Process::new(pid)
        .ok()?
        .stat()
        .ok()
        .map(|stat| stat.starttime)
}

#[cfg(test)]
pub(crate) fn executable(pid: i32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .map(|path| path.display().to_string())
}

/// The locked pidfile.
#[derive(Debug)]
pub(crate) struct Pidfile {
    file: std::fs::File,
}

/// Another Mimik holds the lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Busy;

impl Pidfile {
    /// Takes the exclusive lock, without waiting.
    pub(crate) fn acquire(run: BorrowedFd<'_>) -> Result<Result<Self, Busy>, Refusal> {
        let fd = rustix::fs::openat2(
            run,
            PIDFILE,
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        )
        .map_err(|_| Refusal::Io(PIDFILE.into()))?;
        let stat = rustix::fs::fstat(&fd).map_err(|_| Refusal::Io(PIDFILE.into()))?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
            return Err(Refusal::NotRegular(PIDFILE.into()));
        }
        if stat.st_uid != rustix::process::getuid().as_raw() {
            return Err(Refusal::Owner(PIDFILE.into()));
        }
        match rustix::fs::flock(&fd, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(Ok(Self {
                file: std::fs::File::from(fd),
            })),
            Err(rustix::io::Errno::WOULDBLOCK) => Ok(Err(Busy)),
            Err(_) => Err(Refusal::Io(PIDFILE.into())),
        }
    }

    /// The entry a previous Mimik left, if any (`None` for an unreadable one too).
    pub(crate) fn leftover(&mut self) -> Option<Entry> {
        self.entry().ok().flatten()
    }

    /// The recorded entry: `Ok(None)` for an empty file (no child), an error for one that
    /// cannot be read or does not parse (a torn write, a disk error), which decides nothing.
    pub(crate) fn entry(&mut self) -> Result<Option<Entry>, Unreadable> {
        let mut text = String::new();
        self.file.rewind().map_err(|_| Unreadable)?;
        Read::by_ref(&mut self.file)
            .take(4096)
            .read_to_string(&mut text)
            .map_err(|_| Unreadable)?;
        if text.is_empty() {
            return Ok(None);
        }
        Entry::parse(&text).map(Some).ok_or(Unreadable)
    }

    /// Records the running child: the entry is written over the old one, synced, and only
    /// then the file truncated to it, so an interruption leaves either the old entry, a torn
    /// one (which never parses), or the new one followed by old bytes (which parses).
    pub(crate) fn record(&mut self, entry: &Entry) -> std::io::Result<()> {
        let text = entry.text();
        self.file.rewind()?;
        self.file.write_all(text.as_bytes())?;
        self.file.sync_data()?;
        self.file
            .set_len(u64::try_from(text.len()).unwrap_or(u64::MAX))?;
        self.file.sync_data()
    }

    /// Test only: writes raw bytes, as an interrupted or corrupted record would leave them.
    #[cfg(test)]
    pub(crate) fn write_raw(&mut self, bytes: &[u8]) {
        self.file.set_len(0).unwrap();
        self.file.rewind().unwrap();
        self.file.write_all(bytes).unwrap();
    }

    /// No child runs.
    pub(crate) fn clear(&mut self) -> std::io::Result<()> {
        self.file.set_len(0)?;
        self.file.sync_data()
    }

    /// Kills a leftover child only when its PID, start time and executable all match; clears
    /// the entry unless the killed leftover is still running.
    ///
    /// A pidfd is opened first and the identity read after it: if the PID was reused before
    /// the pidfd, the identity is the newcomer's and does not match; if the leftover exits and
    /// its PID is reused after, the signal still goes through the pidfd to the old process
    /// (gone, so nothing is signalled). A kill waits (up to 5 s) for the process to end.
    /// Blocking: run it off the async runtime.
    pub(crate) fn sweep(&mut self) -> Swept {
        self.sweep_with(
            || {},
            |watched| {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while watched.alive() && std::time::Instant::now() < deadline {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                !watched.alive()
            },
        )
    }

    /// [`Pidfile::sweep`] with `between` run once the pidfd is open, and `gone` deciding
    /// whether the killed leftover ended.
    pub(crate) fn sweep_with(
        &mut self,
        between: impl FnOnce(),
        gone: impl FnOnce(&super::endpoint::Watched) -> bool,
    ) -> Swept {
        self.sweep_in(&Host, between, gone)
    }

    /// The sweep against `processes` (the host's, or a test's model of PID reuse).
    pub(crate) fn sweep_in<P: Processes>(
        &mut self,
        processes: &P,
        between: impl FnOnce(),
        gone: impl FnOnce(&P::Handle) -> bool,
    ) -> Swept {
        let entry = match self.entry() {
            Ok(Some(entry)) => entry,
            Ok(None) => return Swept::Nothing,
            // It cannot be told what ran: nothing is killed, cleared or removed.
            Err(Unreadable) => return Swept::Unreadable,
        };
        let outcome = match processes.open(entry.pid) {
            // No such process: nothing to kill.
            Err(error) if error.raw_os_error() == Some(rustix::io::Errno::SRCH.raw_os_error()) => {
                Swept::Nothing
            }
            // It cannot be told whether the leftover runs: the entry stays.
            Err(_) => Swept::StillRunning,
            Ok(handle) => {
                between();
                let (start, exe) = processes.identity(entry.pid);
                // A definite mismatch (another process has the PID) clears the entry; an
                // identity that cannot be read decides nothing while the pinned process runs.
                let mismatch = start.as_ref().is_ok_and(|start| *start != entry.start_time)
                    || exe
                        .as_ref()
                        .is_ok_and(|exe| !entry.executables.contains(exe));
                let known = start.is_ok() && exe.is_ok();
                if mismatch {
                    Swept::Nothing
                } else if !known || processes.kill(&handle).is_err() {
                    // Unknown, or the kill failed: decided by whether it still runs.
                    if processes.alive(&handle) {
                        Swept::StillRunning
                    } else {
                        Swept::Nothing
                    }
                } else if gone(&handle) {
                    Swept::Killed
                } else {
                    Swept::StillRunning
                }
            }
        };
        if outcome != Swept::StillRunning {
            let _ = self.clear();
        }
        outcome
    }
}

/// What the sweep needs of processes: a handle pinning one process (a pidfd on the host),
/// a PID's current identity, and a kill through the handle (never by PID).
pub(crate) trait Processes {
    type Handle;
    fn open(&self, pid: i32) -> std::io::Result<Self::Handle>;
    /// The start time and executable of whatever process has `pid` now (each an error when
    /// it cannot be read).
    fn identity(&self, pid: i32) -> (std::io::Result<u64>, std::io::Result<String>);
    fn kill(&self, handle: &Self::Handle) -> std::io::Result<()>;
    fn alive(&self, handle: &Self::Handle) -> bool;
}

/// The host's processes, through pidfds.
pub(crate) struct Host;

impl Processes for Host {
    type Handle = super::endpoint::Watched;

    fn open(&self, pid: i32) -> std::io::Result<Self::Handle> {
        super::endpoint::Watched::open(pid)
    }

    fn identity(&self, pid: i32) -> (std::io::Result<u64>, std::io::Result<String>) {
        (
            start_time(pid).ok_or_else(|| std::io::Error::other("no start time")),
            std::fs::read_link(format!("/proc/{pid}/exe")).map(|path| path.display().to_string()),
        )
    }

    fn kill(&self, handle: &Self::Handle) -> std::io::Result<()> {
        rustix::process::pidfd_send_signal(&handle.pidfd, rustix::process::Signal::KILL)
            .map_err(Into::into)
    }

    fn alive(&self, handle: &Self::Handle) -> bool {
        handle.alive()
    }
}

/// The pidfile cannot be read or does not parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Unreadable;

/// What a sweep found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Swept {
    /// No leftover (or another process now has its PID).
    Nothing,
    /// The leftover was killed and has ended.
    Killed,
    /// A leftover runs (killed, not yet ended, or it cannot be told): its entry stays, and
    /// nothing may start or remove generation directories until it ends.
    StillRunning,
    /// The pidfile cannot be read or is torn: the entry stays, and nothing starts or is
    /// removed until the owner has looked.
    Unreadable,
}

/// Removes what earlier runs left under `run/`: every generation directory (its plain
/// entries, then itself). Links are unlinked, never followed.
pub(crate) fn sweep_generations(run: BorrowedFd<'_>) {
    let Ok(listing) = rustix::fs::Dir::read_from(run) else {
        return;
    };
    for entry in listing.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "." || name == ".." || name == PIDFILE {
            continue;
        }
        if name.bytes().all(|b| b.is_ascii_digit()) || name.starts_with("probe") {
            remove_generation(run, &name);
        }
    }
}

/// Creates `run/<name>/` with mode 0700, opens it without following links and checks
/// it is a 0700 directory of Mimik's user. A directory already there refuses the start.
pub(crate) fn create_generation(run: BorrowedFd<'_>, name: &str) -> Result<OwnedFd, Refusal> {
    rustix::fs::mkdirat(run, name, Mode::from_raw_mode(0o700)).map_err(|error| match error {
        rustix::io::Errno::EXIST => Refusal::Exists(format!("run/{name}")),
        _ => Refusal::Io(format!("run/{name}")),
    })?;
    files::open_dir_beneath(run, name, Owners::Mine, true)
}

/// Removes a generation's directory and everything in it (the level-3 private home may
/// hold what the server wrote), never following a link: entries are unlinked, directories
/// emptied the same way, at most 8 levels deep.
pub(crate) fn remove_generation(run: BorrowedFd<'_>, name: &str) {
    empty(run, name, 8);
    let _ = rustix::fs::unlinkat(run, name, AtFlags::REMOVEDIR)
        .or_else(|_| rustix::fs::unlinkat(run, name, AtFlags::empty()));
}

fn empty(parent: BorrowedFd<'_>, name: &str, depth: usize) {
    let Ok(dir) = rustix::fs::openat2(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    ) else {
        return;
    };
    let Ok(listing) = rustix::fs::Dir::read_from(&dir) else {
        return;
    };
    for entry in listing.flatten() {
        let inner = entry.file_name().to_string_lossy().into_owned();
        if inner == "." || inner == ".." {
            continue;
        }
        if rustix::fs::unlinkat(&dir, &*inner, AtFlags::empty()).is_err() && depth > 0 {
            empty(dir.as_fd(), &inner, depth - 1);
            let _ = rustix::fs::unlinkat(&dir, &*inner, AtFlags::REMOVEDIR);
        }
    }
}

/// A generation's directory, removed when dropped.
#[derive(Debug)]
pub(crate) struct Generation {
    pub fd: OwnedFd,
    run: OwnedFd,
    name: String,
}

impl Generation {
    /// `run/<name>/`: a generation's number, or `probe-<level>` for a start-time probe.
    pub(crate) fn create(run: &OwnedFd, name: &str) -> Result<Self, Refusal> {
        let fd = create_generation(run.as_fd(), name)?;
        Ok(Self {
            fd,
            run: run.try_clone().map_err(|_| Refusal::Io("run".into()))?,
            name: name.to_owned(),
        })
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for Generation {
    fn drop(&mut self) {
        remove_generation(self.run.as_fd(), &self.name);
    }
}

#[cfg(test)]
#[path = "rundir_tests.rs"]
mod tests;
