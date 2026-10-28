//! The files of a download (`docs/model-downloads.md`): every operation is
//! descriptor-relative to the checked models folder and its `.partial` folder, blocking, and
//! meant to run off the async executor. Nothing here follows a link, truncates a linked file,
//! opens a FIFO blockingly, or replaces a file that exists.
use crate::assistant::runtime::files::{self, Owners, Refusal};
use rustix::fs::{AtFlags, FileType, Mode, OFlags, RenameFlags, Stat};
use std::{
    fs::File,
    os::{
        fd::{AsFd, BorrowedFd, OwnedFd},
        unix::fs::FileExt,
    },
    path::Path,
    sync::Arc,
};

/// The folder of partial downloads inside the models folder.
pub(crate) const PARTIAL_DIR: &str = ".partial";

/// A place a test may fail or stop the work: a crash is the operation returning early with the
/// disk left as it is; an I/O error is the same with an error the caller must not paper over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fault {
    /// Before each write of received bytes (a test may block here).
    BeforeWrite,
    /// After the last byte is written.
    AfterWrite,
    /// A read error while hashing, before the first read.
    ReadError,
    /// A failure of `fsync` on the file, before it.
    FileFsyncError,
    /// After the file is synced and verified, before it is published.
    AfterVerify,
    /// After the final name exists by `link` and before the partial name is removed.
    AfterLink,
    /// After the partial name was removed (the `link` fallback only).
    AfterUnlink,
    /// A failure of `fsync` on a directory, before it.
    DirFsyncError,
    /// Between the two directory syncs.
    BetweenDirSyncs,
    /// After the publication is durable, before the row says so.
    AfterPublish,
    /// Immediately before the row is written.
    BeforeCommit,
}

/// The fault seam: production never fails.
pub(crate) trait Faults: Send + Sync {
    fn hit(&self, point: Fault) -> std::io::Result<()>;
}

pub(crate) struct NoFaults;

impl Faults for NoFaults {
    fn hit(&self, _point: Fault) -> std::io::Result<()> {
        Ok(())
    }
}

/// Why a disk operation failed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum DiskError {
    #[error("{0}")]
    Refused(Refusal),
    #[error("the file {0} is unsafe to use ({1})")]
    Unsafe(&'static str, &'static str),
    #[error("the name is already taken by another file")]
    Exists,
    #[error("a disk operation failed ({0})")]
    Io(&'static str),
    #[error("stopped")]
    Cancelled,
}

impl From<Refusal> for DiskError {
    fn from(refusal: Refusal) -> Self {
        if matches!(refusal, Refusal::Cancelled) {
            Self::Cancelled
        } else {
            Self::Refused(refusal)
        }
    }
}

/// What a partial file is, after the safety checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Partial {
    pub len: u64,
    pub inode: u64,
    pub links: u64,
}

pub(crate) type Space = Arc<dyn Fn(BorrowedFd<'_>) -> u64 + Send + Sync>;

/// The checked models folder and its `.partial` folder.
pub(crate) struct Disk {
    models: OwnedFd,
    partial: OwnedFd,
    faults: Arc<dyn Faults>,
    space: Space,
    /// Tests: pretend the file system lacks `RENAME_NOREPLACE` to exercise the `link` fallback.
    #[cfg(test)]
    pub(crate) no_rename2: std::sync::atomic::AtomicBool,
}

fn io_error(what: &'static str) -> impl Fn(rustix::io::Errno) -> DiskError {
    move |_| DiskError::Io(what)
}

fn injected(error: &std::io::Error) -> DiskError {
    // The kind says what was simulated; the message is for the test.
    let _ = error;
    DiskError::Io("injected")
}

fn real_space(fd: BorrowedFd<'_>) -> u64 {
    rustix::fs::fstatvfs(fd).map_or(0, |stat| stat.f_bavail.saturating_mul(stat.f_frsize))
}

impl Disk {
    /// Opens the models folder (owned by Mimik's user, mode 0700) and creates `.partial` in it.
    ///
    /// # Errors
    ///
    /// A folder that is missing, a link, not Mimik's, or not private.
    pub(crate) fn open(models: &Path, faults: Arc<dyn Faults>) -> Result<Self, DiskError> {
        let models = files::open_dir(models, Owners::Mine, true)?;
        match rustix::fs::mkdirat(&models, PARTIAL_DIR, Mode::from_raw_mode(0o700)) {
            Ok(()) | Err(rustix::io::Errno::EXIST) => {}
            Err(_) => return Err(DiskError::Io("the partial folder cannot be created")),
        }
        let partial = files::open_dir_beneath(models.as_fd(), PARTIAL_DIR, Owners::Mine, true)?;
        Ok(Self {
            models,
            partial,
            faults,
            space: Arc::new(real_space),
            #[cfg(test)]
            no_rename2: std::sync::atomic::AtomicBool::new(false),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_space(mut self, space: Space) -> Self {
        self.space = space;
        self
    }

    fn hit(&self, point: Fault) -> Result<(), DiskError> {
        self.faults.hit(point).map_err(|error| injected(&error))
    }

    /// Bytes available to Mimik's user in the models folder's file system.
    pub(crate) fn free_space(&self) -> u64 {
        (self.space)(self.models.as_fd())
    }

    fn check(stat: &Stat, what: &'static str) -> Result<(), DiskError> {
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
            return Err(DiskError::Unsafe(what, "not a regular file"));
        }
        if stat.st_uid != rustix::process::getuid().as_raw() {
            return Err(DiskError::Unsafe(what, "not owned by Mimik's user"));
        }
        if stat.st_mode & 0o022 != 0 {
            return Err(DiskError::Unsafe(what, "writable by group or others"));
        }
        Ok(())
    }

    /// The partial `name`'s state without the link-count rule (reconciliation needs to see a
    /// second link), or `None` when absent.
    pub(crate) fn partial_state(&self, name: &str) -> Result<Option<Partial>, DiskError> {
        match rustix::fs::statat(&self.partial, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => {
                Self::check(&stat, "partial")?;
                Ok(Some(Partial {
                    len: u64::try_from(stat.st_size).unwrap_or(0),
                    inode: stat.st_ino,
                    links: stat.st_nlink,
                }))
            }
            Err(rustix::io::Errno::NOENT) => Ok(None),
            Err(_) => Err(DiskError::Io("the partial cannot be examined")),
        }
    }

    /// Creates the partial `name` (mode 0600), refusing an existing name, a link and a FIFO.
    pub(crate) fn create_partial(&self, name: &str) -> Result<File, DiskError> {
        let fd = rustix::fs::openat(
            &self.partial,
            name,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(io_error("the partial cannot be created"))?;
        Ok(File::from(fd))
    }

    /// Opens the existing partial `name` for appending: a regular file of Mimik's user, not
    /// writable by group or others, with exactly one link. Never blocks on a FIFO.
    pub(crate) fn open_partial(&self, name: &str) -> Result<(File, u64), DiskError> {
        let fd = rustix::fs::openat(
            &self.partial,
            name,
            OFlags::RDWR | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| match error {
            rustix::io::Errno::LOOP => DiskError::Unsafe("partial", "a symbolic link"),
            _ => DiskError::Io("the partial cannot be opened"),
        })?;
        let stat = rustix::fs::fstat(&fd).map_err(io_error("the partial cannot be examined"))?;
        Self::check(&stat, "partial")?;
        if stat.st_nlink != 1 {
            return Err(DiskError::Unsafe("partial", "it has another hard link"));
        }
        rustix::fs::fcntl_setfl(&fd, OFlags::empty())
            .map_err(io_error("the partial cannot be opened"))?;
        Ok((File::from(fd), u64::try_from(stat.st_size).unwrap_or(0)))
    }

    /// Writes `data` at `offset`.
    pub(crate) fn write_at(&self, file: &File, offset: u64, data: &[u8]) -> Result<(), DiskError> {
        self.hit(Fault::BeforeWrite)?;
        file.write_all_at(data, offset)
            .map_err(|_| DiskError::Io("writing the download failed"))
    }

    /// Shortens (or keeps) the partial to `len`.
    pub(crate) fn truncate(file: &File, len: u64) -> Result<(), DiskError> {
        file.set_len(len)
            .map_err(|_| DiskError::Io("the partial cannot be shortened"))
    }

    /// Flushes the file to disk.
    pub(crate) fn sync_file(&self, file: &File) -> Result<(), DiskError> {
        self.hit(Fault::FileFsyncError)?;
        rustix::fs::fsync(file).map_err(io_error("the file cannot be flushed"))
    }

    /// Hits a fault point from outside the disk operations (the task's own boundaries).
    pub(crate) fn fault(&self, point: Fault) -> Result<(), DiskError> {
        self.hit(point)
    }

    /// The SHA-256 of the whole file, hex, stopping once `stop` says so.
    pub(crate) fn hash(
        &self,
        file: &File,
        name: &str,
        stop: &dyn Fn() -> bool,
    ) -> Result<String, DiskError> {
        self.hit(Fault::ReadError)?;
        let sha = files::hash(file.as_fd(), name, stop)?;
        Ok(hex::encode(sha))
    }

    /// The state of the final file `name` in the models folder (`None` when absent). Not a
    /// regular file of Mimik's user is an error: nothing may be replaced or adopted from it.
    pub(crate) fn final_state(&self, name: &str) -> Result<Option<Partial>, DiskError> {
        match rustix::fs::statat(&self.models, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => {
                Self::check(&stat, "model")?;
                Ok(Some(Partial {
                    len: u64::try_from(stat.st_size).unwrap_or(0),
                    inode: stat.st_ino,
                    links: stat.st_nlink,
                }))
            }
            Err(rustix::io::Errno::NOENT) => Ok(None),
            Err(_) => Err(DiskError::Io("the model file cannot be examined")),
        }
    }

    /// Opens the final file `name` for hashing.
    pub(crate) fn open_final(&self, name: &str) -> Result<File, DiskError> {
        let (fd, _) = files::open_file(self.models.as_fd(), name, Owners::Mine)?;
        Ok(File::from(fd))
    }

    /// Publishes the verified partial `from` as the final file `name` without ever replacing
    /// a file: `renameat2(RENAME_NOREPLACE)`, or where the file system lacks it `link` then
    /// `unlink`. Then the file's directory and the models folder are synced.
    ///
    /// # Errors
    ///
    /// [`DiskError::Exists`] when the name is taken; an I/O failure otherwise (nothing is
    /// then published as complete: the caller keeps the partial for reconciliation).
    pub(crate) fn publish(&self, from: &str, name: &str) -> Result<(), DiskError> {
        #[cfg(test)]
        let skip_rename = self.no_rename2.load(std::sync::atomic::Ordering::SeqCst);
        #[cfg(not(test))]
        let skip_rename = false;
        let renamed = if skip_rename {
            Err(rustix::io::Errno::INVAL)
        } else {
            rustix::fs::renameat_with(
                &self.partial,
                from,
                &self.models,
                name,
                RenameFlags::NOREPLACE,
            )
        };
        match renamed {
            Ok(()) => {}
            Err(rustix::io::Errno::EXIST) => return Err(DiskError::Exists),
            Err(
                rustix::io::Errno::INVAL | rustix::io::Errno::NOSYS | rustix::io::Errno::OPNOTSUPP,
            ) => {
                match rustix::fs::linkat(&self.partial, from, &self.models, name, AtFlags::empty())
                {
                    Ok(()) => {}
                    Err(rustix::io::Errno::EXIST) => return Err(DiskError::Exists),
                    Err(rustix::io::Errno::PERM) => {
                        return Err(DiskError::Io(
                            "this file system cannot publish a download safely",
                        ));
                    }
                    Err(_) => return Err(DiskError::Io("the download cannot be published")),
                }
                self.hit(Fault::AfterLink)?;
                rustix::fs::unlinkat(&self.partial, from, AtFlags::empty())
                    .map_err(io_error("the partial cannot be removed"))?;
                self.hit(Fault::AfterUnlink)?;
            }
            Err(_) => return Err(DiskError::Io("the download cannot be published")),
        }
        // A directory that cannot be flushed leaves the publication undone: the file goes back
        // to its partial name, so nothing is complete that may not survive a crash.
        if let Err(error) = self.sync_directories() {
            let _ = rustix::fs::renameat(&self.models, name, &self.partial, from);
            return Err(error);
        }
        self.hit(Fault::AfterPublish)
    }

    fn sync_directories(&self) -> Result<(), DiskError> {
        self.hit(Fault::DirFsyncError)?;
        rustix::fs::fsync(&self.partial).map_err(io_error("the folder cannot be flushed"))?;
        self.hit(Fault::BetweenDirSyncs)?;
        self.hit(Fault::DirFsyncError)?;
        rustix::fs::fsync(&self.models).map_err(io_error("the folder cannot be flushed"))
    }

    /// Removes the partial `name` (absent is fine).
    pub(crate) fn remove_partial(&self, name: &str) -> Result<(), DiskError> {
        match rustix::fs::unlinkat(&self.partial, name, AtFlags::empty()) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(()),
            Err(_) => Err(DiskError::Io("the partial cannot be removed")),
        }
    }

    /// Removes the final file `name` (absent is fine).
    pub(crate) fn remove_final(&self, name: &str) -> Result<(), DiskError> {
        match rustix::fs::unlinkat(&self.models, name, AtFlags::empty()) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(()),
            Err(_) => Err(DiskError::Io("the model file cannot be removed")),
        }
    }

    /// The names in the partial folder.
    pub(crate) fn partials(&self) -> Result<Vec<String>, DiskError> {
        // A fresh descriptor: a duplicate would share the read offset with earlier listings.
        let fresh = rustix::fs::openat(
            &self.partial,
            ".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io_error("the partial folder cannot be listed"))?;
        let mut dir =
            rustix::fs::Dir::new(fresh).map_err(io_error("the partial folder cannot be listed"))?;
        let mut names = Vec::new();
        while let Some(entry) = dir.read() {
            let entry = entry.map_err(io_error("the partial folder cannot be listed"))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name != "." && name != ".." {
                names.push(name);
            }
        }
        Ok(names)
    }
}

#[cfg(test)]
#[path = "disk_tests.rs"]
mod tests;
