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
    fn text(&self) -> String {
        let mut text = format!("{}\n{}\n{}\n", self.generation, self.pid, self.start_time);
        for executable in &self.executables {
            text.push_str(executable);
            text.push('\n');
        }
        text
    }

    fn parse(text: &str) -> Option<Self> {
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

fn executable(pid: i32) -> Option<String> {
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

    /// The entry a previous Mimik left, if any.
    pub(crate) fn leftover(&mut self) -> Option<Entry> {
        let mut text = String::new();
        self.file.rewind().ok()?;
        Read::by_ref(&mut self.file).take(4096).read_to_string(&mut text).ok()?;
        Entry::parse(&text)
    }

    /// Records the running child.
    pub(crate) fn record(&mut self, entry: &Entry) -> std::io::Result<()> {
        self.file.set_len(0)?;
        self.file.rewind()?;
        self.file.write_all(entry.text().as_bytes())?;
        self.file.sync_data()
    }

    /// No child runs.
    pub(crate) fn clear(&mut self) -> std::io::Result<()> {
        self.file.set_len(0)?;
        self.file.sync_data()
    }

    /// Kills a leftover child only when its PID, start time and executable all match, then
    /// clears the entry. Returns whether one was killed.
    pub(crate) fn sweep(&mut self) -> bool {
        let Some(entry) = self.leftover() else {
            return false;
        };
        let matches = start_time(entry.pid) == Some(entry.start_time)
            && executable(entry.pid).is_some_and(|exe| entry.executables.contains(&exe));
        let killed = matches
            && rustix::process::Pid::from_raw(entry.pid).is_some_and(|pid| {
                rustix::process::kill_process(pid, rustix::process::Signal::KILL).is_ok()
            });
        let _ = self.clear();
        killed
    }
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
        if name.bytes().all(|b| b.is_ascii_digit()) {
            remove_generation(run, &name);
        }
    }
}

/// Creates `run/<generation>/` with mode 0700, opens it without following links and checks
/// it is a 0700 directory of Mimik's user. A directory already there refuses the start.
pub(crate) fn create_generation(run: BorrowedFd<'_>, generation: u64) -> Result<OwnedFd, Refusal> {
    let name = generation.to_string();
    rustix::fs::mkdirat(run, &name, Mode::from_raw_mode(0o700)).map_err(|error| match error {
        rustix::io::Errno::EXIST => Refusal::Extra(format!("run/{name}")),
        _ => Refusal::Io(format!("run/{name}")),
    })?;
    files::open_dir_beneath(run, &name, Owners::Mine, true)
}

/// Removes a generation's directory: its entries (not descending into directories), then
/// the directory.
pub(crate) fn remove_generation(run: BorrowedFd<'_>, name: &str) {
    if let Ok(dir) = rustix::fs::openat2(
        run,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    ) && let Ok(listing) = rustix::fs::Dir::read_from(&dir)
    {
        for entry in listing.flatten() {
            let inner = entry.file_name().to_string_lossy().into_owned();
            if inner != "." && inner != ".." {
                let _ = rustix::fs::unlinkat(&dir, &*inner, AtFlags::empty());
            }
        }
    }
    let _ = rustix::fs::unlinkat(run, name, AtFlags::REMOVEDIR)
        .or_else(|_| rustix::fs::unlinkat(run, name, AtFlags::empty()));
}

/// A generation's directory, removed when dropped.
#[derive(Debug)]
pub(crate) struct Generation {
    pub fd: OwnedFd,
    run: OwnedFd,
    name: String,
}

impl Generation {
    pub(crate) fn create(run: &OwnedFd, generation: u64) -> Result<Self, Refusal> {
        let fd = create_generation(run.as_fd(), generation)?;
        Ok(Self {
            fd,
            run: run.try_clone().map_err(|_| Refusal::Io("run".into()))?,
            name: generation.to_string(),
        })
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
