//! Private, descriptor-relative staging and reservation accounting.
use super::worker::{Error, Held, Reason};
use crate::AppError;
use rustix::{
    fd::OwnedFd,
    fs::{Mode, OFlags},
};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub const MIB: u64 = 1024 * 1024;
/// Free space that always stays available to SQLite, its journal and the host.
pub const MARGIN: u64 = 512 * MIB;
/// The unit in which an upload's reservation grows when a body without a length (or a longer
/// one than declared) is written.
pub const TOP_UP: u64 = 64 * MIB;
/// The reservation of an upload body that declares no length.
pub const UPLOAD_FLOOR: u64 = MIB;
pub const RENDER_RESERVATION: u64 = 8 * 20 * MIB;

/// What a scan of the staging directory found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    /// Bytes of the regular files that were measured.
    pub bytes: u64,
    /// Some entry could not be measured: `bytes` is a lower bound.
    pub unknown: bool,
}

fn io(e: rustix::io::Errno) -> AppError {
    std::io::Error::from(e).into()
}
fn space() -> AppError {
    AppError::Invalid("Not enough space.")
}

#[derive(Default, Debug)]
struct Accounting {
    used: u64,
    reserved: u64,
    /// Bytes written to the active upload's spool. A scan skips that file, so they are kept
    /// apart from `used`, which a scan replaces.
    spool: u64,
    /// The last scan could not measure every entry; nothing is admitted until a scan succeeds.
    unknown: bool,
}

/// A single instance belongs to the process-wide upload manager.
///
/// Admission is by free space only: a reservation is admitted while the filesystem's free
/// space covers every outstanding reservation plus [`MARGIN`]. Bytes already written leave the
/// reservation (the filesystem's free space no longer includes them), so nothing is counted
/// twice.
pub struct Ledger {
    bytes: Mutex<Accounting>,
    free: Box<dyn Fn() -> Result<u64, AppError> + Send + Sync>,
    #[cfg(test)]
    pub bypass_free: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    injected_free: Mutex<Option<Arc<std::sync::atomic::AtomicU64>>>,
}
impl Ledger {
    pub fn new(
        used: u64,
        free: impl Fn() -> Result<u64, AppError> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Self::with_usage(
            Usage {
                bytes: used,
                unknown: false,
            },
            free,
        )
    }
    pub fn with_usage(
        usage: Usage,
        free: impl Fn() -> Result<u64, AppError> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            bytes: Mutex::new(Accounting {
                used: usage.bytes,
                reserved: 0,
                spool: 0,
                unknown: usage.unknown,
            }),
            free: Box::new(free),
            #[cfg(test)]
            bypass_free: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            injected_free: Mutex::new(None),
        })
    }
    /// Test seam: report this value as the filesystem's free space.
    #[cfg(test)]
    pub fn inject_free(&self, free: Arc<std::sync::atomic::AtomicU64>) {
        if let Ok(mut injected) = self.injected_free.lock() {
            *injected = Some(free);
        }
    }
    /// # Errors
    /// Refuses reservations the free space cannot cover, an unknown usage or a poisoned ledger.
    pub fn reserve(self: &Arc<Self>, bytes: u64) -> Result<Reservation, AppError> {
        self.admit(bytes)?;
        Ok(Reservation {
            ledger: self.clone(),
            remaining: bytes,
        })
    }
    /// A reservation that holds nothing yet; [`Reservation::top_up`] admits its bytes.
    #[must_use]
    pub fn empty_reservation(self: &Arc<Self>) -> Reservation {
        Reservation {
            ledger: self.clone(),
            remaining: 0,
        }
    }
    fn admit(&self, bytes: u64) -> Result<(), AppError> {
        let mut a = self.bytes.lock().map_err(|_| AppError::Worker)?;
        if a.unknown {
            return Err(space());
        }
        let reserved = a.reserved.checked_add(bytes).ok_or_else(space)?;
        if self.free_bytes()? < reserved.checked_add(MARGIN).ok_or_else(space)? {
            return Err(space());
        }
        a.reserved = reserved;
        Ok(())
    }
    fn free_bytes(&self) -> Result<u64, AppError> {
        #[cfg(test)]
        if self.bypass_free.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(u64::MAX);
        }
        #[cfg(test)]
        if let Some(free) = self
            .injected_free
            .lock()
            .map_err(|_| AppError::Worker)?
            .as_ref()
        {
            return Ok(free.load(std::sync::atomic::Ordering::SeqCst));
        }
        (self.free)()
    }
    /// # Errors
    /// Returns an error if the ledger lock was poisoned.
    pub fn usage(&self) -> Result<(u64, u64), AppError> {
        let a = self.bytes.lock().map_err(|_| AppError::Worker)?;
        Ok((a.used.saturating_add(a.spool), a.reserved))
    }
    /// The active upload's spool became a staged document of `actual` bytes: its charged
    /// `written` bytes move from the spool figure to the used figure.
    /// # Errors
    /// Returns an error if the ledger lock was poisoned.
    pub fn spool_published(&self, written: u64, actual: u64) -> Result<(), AppError> {
        let mut a = self.bytes.lock().map_err(|_| AppError::Worker)?;
        a.spool = a.spool.saturating_sub(written);
        a.used = a.used.saturating_add(actual);
        Ok(())
    }
    /// The active upload's spool ended: unlinked (`kept` false) or left on disk after a failed
    /// unlink (`kept` true, so it stays charged until the next scan).
    /// # Errors
    /// Returns an error if the ledger lock was poisoned.
    pub fn spool_ended(&self, written: u64, kept: bool) -> Result<(), AppError> {
        let mut a = self.bytes.lock().map_err(|_| AppError::Worker)?;
        a.spool = a.spool.saturating_sub(written);
        if kept {
            a.used = a.used.saturating_add(written);
        }
        Ok(())
    }
    /// # Errors
    /// Refuses accounting underflow or a poisoned ledger.
    pub fn deleted(&self, bytes: u64) -> Result<(), AppError> {
        let mut a = self.bytes.lock().map_err(|_| AppError::Worker)?;
        a.used = a
            .used
            .checked_sub(bytes)
            .ok_or(AppError::Invalid("Staging accounting mismatch."))?;
        Ok(())
    }
    /// Replace disk usage after a locked scan, preserving active reservations. A complete scan
    /// clears the unknown state; an incomplete one sets it.
    /// # Errors
    /// Returns an error if the ledger lock was poisoned.
    pub fn reconcile(&self, usage: Usage) -> Result<(), AppError> {
        let mut a = self.bytes.lock().map_err(|_| AppError::Worker)?;
        a.used = usage.bytes;
        a.unknown = usage.unknown;
        Ok(())
    }
}
pub struct Reservation {
    ledger: Arc<Ledger>,
    remaining: u64,
}
impl Reservation {
    /// Admit `bytes` more under the same free-space rule as a new reservation.
    /// # Errors
    /// Refuses what the free space cannot cover, an unknown usage or a poisoned ledger.
    pub fn top_up(&mut self, bytes: u64) -> Result<(), AppError> {
        let remaining = self.remaining.checked_add(bytes).ok_or_else(space)?;
        self.ledger.admit(bytes)?;
        self.remaining = remaining;
        Ok(())
    }
    /// Bytes still reserved.
    #[must_use]
    pub fn remaining(&self) -> u64 {
        self.remaining
    }
    /// Charge a successful physical write before relinquishing its reservation.
    /// # Errors
    /// Refuses writes larger than the reservation or a poisoned ledger.
    pub fn written(&mut self, bytes: u64) -> Result<(), AppError> {
        if bytes > self.remaining {
            return Err(space());
        }
        let mut a = self.ledger.bytes.lock().map_err(|_| AppError::Worker)?;
        a.used += bytes;
        a.reserved -= bytes;
        self.remaining -= bytes;
        Ok(())
    }
    /// Like [`Self::written`] for the active upload's spool: the bytes are charged to the spool
    /// figure, which a scan of the directory (it skips the active spool) never overwrites.
    /// # Errors
    /// Refuses writes larger than the reservation or a poisoned ledger.
    pub fn written_spool(&mut self, bytes: u64) -> Result<(), AppError> {
        if bytes > self.remaining {
            return Err(space());
        }
        let mut a = self.ledger.bytes.lock().map_err(|_| AppError::Worker)?;
        a.spool += bytes;
        a.reserved -= bytes;
        self.remaining -= bytes;
        Ok(())
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if let Ok(mut a) = self.ledger.bytes.lock() {
            a.reserved -= self.remaining;
        }
    }
}

pub struct Files {
    root: OwnedFd,
    path: PathBuf,
    #[cfg(test)]
    pub fail_remove: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    pub fail_cleanup: Mutex<Option<std::ffi::OsString>>,
    #[cfg(test)]
    pub fail_spool_chmod: std::sync::atomic::AtomicBool,
    /// Test seam: `write` calls that still succeed before one fails with ENOSPC.
    #[cfg(test)]
    pub enospc_after: std::sync::atomic::AtomicI64,
    /// Test seam: the sync after a published spool's rename fails.
    #[cfg(test)]
    pub fail_publish_sync: std::sync::atomic::AtomicBool,
    /// Test seam: `remove_spool` fails.
    #[cfg(test)]
    pub fail_remove_spool: std::sync::atomic::AtomicBool,
}
impl Files {
    pub(crate) fn read_text(&self, id: &str, page: u32) -> Result<Vec<u8>, AppError> {
        let dir = self.dir(id)?;
        let fd = rustix::fs::openat(
            &dir,
            format!("text-{page}.json"),
            crate::local_file::flags(),
            Mode::empty(),
        )
        .map_err(io)?;
        crate::local_file::read(fd, 8 * 1024 * 1024)
    }
    pub(crate) fn read_image(&self, id: &str, page: u32) -> Result<Vec<u8>, AppError> {
        let dir = self.dir(id)?;
        let fd = rustix::fs::openat(
            &dir,
            format!("page-{page}.png"),
            crate::local_file::flags(),
            Mode::empty(),
        )
        .map_err(io)?;
        crate::local_file::read(fd, 12 * 1024 * 1024)
    }
    /// # Errors
    /// Returns entropy or private-file creation failures.
    pub fn spool(&self) -> Result<(String, File), AppError> {
        let mut random = [0; 16];
        getrandom::fill(&mut random).map_err(|_| AppError::Worker)?;
        let name = format!("upload-{}.part", hex::encode(random));
        let fd = rustix::fs::openat(
            &self.root,
            &name,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(io)?;
        #[cfg(test)]
        let chmod = self.chmod_spool(&fd);
        #[cfg(not(test))]
        let chmod = rustix::fs::fchmod(&fd, Mode::RUSR | Mode::WUSR).map_err(io);
        if let Err(error) = chmod {
            self.remove_spool(&name)?;
            return Err(error);
        }
        Ok((name, File::from(fd)))
    }
    #[cfg(test)]
    fn chmod_spool(&self, fd: &OwnedFd) -> Result<(), AppError> {
        if self
            .fail_spool_chmod
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(std::io::Error::from_raw_os_error(libc::EPERM).into());
        }
        rustix::fs::fchmod(fd, Mode::RUSR | Mode::WUSR).map_err(io)
    }
    /// The path of a spool file, for a confined reader of its bytes.
    /// # Errors
    /// Refuses an invalid name.
    pub fn spool_path(&self, name: &str) -> Result<PathBuf, AppError> {
        valid_name(name)?;
        Ok(self.path.join(name))
    }
    /// # Errors
    /// Returns invalid-name, unlink or directory-sync failures.
    pub fn remove_spool(&self, name: &str) -> Result<(), AppError> {
        #[cfg(test)]
        if self
            .fail_remove_spool
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(std::io::Error::from_raw_os_error(libc::EACCES).into());
        }
        valid_name(name)?;
        match rustix::fs::unlinkat(&self.root, name, rustix::fs::AtFlags::empty()) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => (),
            Err(e) => return Err(io(e)),
        }
        rustix::fs::fsync(&self.root).map_err(io)?;
        Ok(())
    }
    /// Move a spool into an upload directory. Its bytes were charged as they were written.
    /// # Errors
    /// Refuses identity mismatches and propagates rename/sync failures.
    pub fn publish_spool(&self, name: &str, file: &File, id: &str) -> Result<(), AppError> {
        valid_name(name)?;
        let dir = self.dir(id)?;
        let meta = file.metadata()?;
        let stat = rustix::fs::statat(&self.root, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)
            .map_err(io)?;
        if !meta.is_file() || stat.st_dev != meta.dev() || stat.st_ino != meta.ino() {
            return Err(AppError::Invalid("Invalid upload spool."));
        }
        file.sync_all()?;
        rustix::fs::renameat(&self.root, name, &dir, "document.pdf").map_err(io)?;
        Ok(())
    }
    /// Make a published spool durable. A separate step so that the caller records the rename
    /// (the file now belongs to the upload directory) before any sync can fail.
    /// # Errors
    /// Propagates directory-sync failures.
    pub fn sync_published(&self, id: &str) -> Result<(), AppError> {
        #[cfg(test)]
        if self
            .fail_publish_sync
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(std::io::Error::from_raw_os_error(libc::EIO).into());
        }
        let dir = self.dir(id)?;
        rustix::fs::fsync(&dir).map_err(io)?;
        rustix::fs::fsync(&self.root).map_err(io)?;
        Ok(())
    }
    /// # Errors
    /// Refuses symlinks and propagates private-directory creation/open failures.
    pub fn open(data: &Path) -> Result<Self, AppError> {
        let parent = rustix::fs::open(
            data,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io)?;
        match rustix::fs::mkdirat(&parent, "staging", Mode::RWXU) {
            Ok(()) | Err(rustix::io::Errno::EXIST) => (),
            Err(e) => return Err(io(e)),
        }
        let root = normalized_dir(&parent, "staging")?;
        Ok(Self {
            root,
            path: data.join("staging"),
            #[cfg(test)]
            fail_remove: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            fail_cleanup: Mutex::new(None),
            #[cfg(test)]
            fail_spool_chmod: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            enospc_after: std::sync::atomic::AtomicI64::new(i64::MAX),
            #[cfg(test)]
            fail_publish_sync: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            fail_remove_spool: std::sync::atomic::AtomicBool::new(false),
        })
    }
    /// # Errors
    /// Refuses invalid upload ids.
    pub fn path(&self, id: &str) -> Result<PathBuf, AppError> {
        valid_id(id)?;
        Ok(self.path.join(id).join("document.pdf"))
    }
    /// Open the staged PDF relative to its directory descriptor without following links and
    /// pin it (regular file, one link, owned by the effective user).
    /// # Errors
    /// `DocumentChanged` for a symbolic link or a file that fails the pin checks; `Unreadable`
    /// for a missing file, an invalid id or any other failure.
    pub fn open_document(&self, id: &str) -> Result<Held, Reason> {
        let dir = self.dir(id).map_err(|_| Reason::from(Error::Unreadable))?;
        match rustix::fs::openat(
            &dir,
            "document.pdf",
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK | OFlags::NOCTTY,
            Mode::empty(),
        ) {
            Ok(fd) => Held::new(File::from(fd)),
            Err(rustix::io::Errno::LOOP) => Err(Reason::DocumentChanged),
            Err(_) => Err(Error::Unreadable.into()),
        }
    }
    /// # Errors
    /// Returns filesystem-statistics failures.
    pub fn free(&self) -> Result<u64, AppError> {
        let s = rustix::fs::fstatvfs(&self.root).map_err(io)?;
        Ok(s.f_bavail.saturating_mul(s.f_frsize))
    }
    /// # Errors
    /// Returns invalid ids and directory creation/sync failures.
    pub fn create(&self, id: &str) -> Result<(), AppError> {
        valid_id(id)?;
        rustix::fs::mkdirat(&self.root, id, Mode::RWXU).map_err(io)?;
        normalized_dir(&self.root, id)?;
        rustix::fs::fsync(&self.root).map_err(io)?;
        Ok(())
    }
    fn dir(&self, id: &str) -> Result<OwnedFd, AppError> {
        valid_id(id)?;
        rustix::fs::openat(
            &self.root,
            id,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io)
    }
    /// # Errors
    /// Refuses invalid names/ids, insufficient reservations, and write/rename/sync failures.
    pub fn write(
        &self,
        id: &str,
        name: &str,
        bytes: &[u8],
        reservation: &mut Reservation,
    ) -> Result<(), AppError> {
        valid_name(name)?;
        if u64::try_from(bytes.len()).map_err(|_| space())? > reservation.remaining {
            return Err(space());
        }
        let dir = self.dir(id)?;
        let part = format!("{name}.part");
        let old_bytes = match rustix::fs::statat(&dir, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)
        {
            Ok(stat)
                if rustix::fs::FileType::from_raw_mode(stat.st_mode)
                    == rustix::fs::FileType::RegularFile =>
            {
                u64::try_from(stat.st_size).map_err(|_| space())?
            }
            Ok(_) | Err(rustix::io::Errno::NOENT) => 0,
            Err(e) => return Err(io(e)),
        };
        let fd = rustix::fs::openat(
            &dir,
            &part,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(io)?;
        let result = (|| {
            rustix::fs::fchmod(&fd, Mode::RUSR | Mode::WUSR).map_err(io)?;
            let mut f = File::from(fd);
            #[cfg(test)]
            if self
                .enospc_after
                .fetch_sub(1, std::sync::atomic::Ordering::AcqRel)
                == 0
            {
                return Err(std::io::Error::from_raw_os_error(libc::ENOSPC).into());
            }
            f.write_all(bytes)?;
            f.sync_all()?;
            rustix::fs::renameat(&dir, &part, &dir, name).map_err(io)?;
            reservation.written(bytes.len() as u64)?;
            reservation.ledger.deleted(old_bytes)?;
            rustix::fs::fsync(&dir).map_err(io)?;
            Ok(())
        })();
        if result.is_err()
            && let Err(error) = rustix::fs::unlinkat(&dir, &part, rustix::fs::AtFlags::empty())
            && error != rustix::io::Errno::NOENT
        {
            let stat = rustix::fs::statat(&dir, &part, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)
                .map_err(io)?;
            if rustix::fs::FileType::from_raw_mode(stat.st_mode)
                == rustix::fs::FileType::RegularFile
            {
                reservation.written(u64::try_from(stat.st_size).map_err(|_| space())?)?;
            }
            return Err(io(error));
        }
        result
    }
    /// # Errors
    /// Returns invalid paths, unlink/sync failures or accounting mismatches.
    pub fn remove(&self, id: &str, name: &str, ledger: &Ledger) -> Result<(), AppError> {
        #[cfg(test)]
        if self.fail_remove.load(std::sync::atomic::Ordering::Acquire) {
            return Err(std::io::Error::from_raw_os_error(libc::EACCES).into());
        }
        valid_name(name)?;
        let dir = self.dir(id)?;
        let stat = match rustix::fs::statat(&dir, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW) {
            Ok(s) => s,
            Err(rustix::io::Errno::NOENT) => return Ok(()),
            Err(e) => return Err(io(e)),
        };
        rustix::fs::unlinkat(&dir, name, rustix::fs::AtFlags::empty()).map_err(io)?;
        if rustix::fs::FileType::from_raw_mode(stat.st_mode) == rustix::fs::FileType::RegularFile {
            ledger.deleted(u64::try_from(stat.st_size).map_err(|_| space())?)?;
        }
        rustix::fs::fsync(&dir).map_err(io)?;
        Ok(())
    }
    /// Startup reconciliation runs before the manager accepts writes. Directory enumeration
    /// is followed only by descriptor-relative, no-follow stat/open/unlink operations.
    /// # Errors
    /// Returns unexpected directories, invalid sizes or filesystem failures.
    pub fn reconcile(&self, mut keep: impl FnMut(&str, &str) -> bool) -> Result<Usage, AppError> {
        self.reconcile_active(&mut keep, None)
    }
    #[must_use]
    pub fn private_file(&self, id: &str, name: &str) -> bool {
        self.dir(id)
            .ok()
            .and_then(|dir| {
                rustix::fs::statat(&dir, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW).ok()
            })
            .is_some_and(|s| {
                rustix::fs::FileType::from_raw_mode(s.st_mode) == rustix::fs::FileType::RegularFile
                    && s.st_mode & 0o777 == 0o600
            })
    }
    /// Reconcile under the manager gate, excluding the exact active spool name.
    /// Cleanup failures are logged and retried; all surviving regular bytes are counted.
    /// # Errors
    /// Returns enumeration or accounting failures when disk usage cannot be established.
    pub fn reconcile_active(
        &self,
        mut keep: impl FnMut(&str, &str) -> bool,
        active_spool: Option<&str>,
    ) -> Result<Usage, AppError> {
        for name in names(&self.root)? {
            if name.to_str().is_some_and(|name| Some(name) == active_spool) {
                continue;
            }
            let result = (|| {
                #[cfg(test)]
                if self
                    .fail_cleanup
                    .lock()
                    .map_err(|_| AppError::Worker)?
                    .as_ref()
                    == Some(&name)
                {
                    return Err(std::io::Error::from_raw_os_error(libc::EACCES).into());
                }
                let s =
                    rustix::fs::statat(&self.root, &name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)
                        .map_err(io)?;
                if rustix::fs::FileType::from_raw_mode(s.st_mode) != rustix::fs::FileType::Directory
                {
                    rustix::fs::unlinkat(&self.root, &name, rustix::fs::AtFlags::empty())
                        .map_err(io)?;
                    return Ok(());
                }
                let dir = rustix::fs::openat(
                    &self.root,
                    &name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(io)?;
                let Some(id) = name
                    .to_str()
                    .filter(|id| valid_id(id).is_ok() && keep(id, ""))
                else {
                    purge(&dir)?;
                    rustix::fs::unlinkat(&self.root, &name, rustix::fs::AtFlags::REMOVEDIR)
                        .map_err(io)?;
                    return Ok(());
                };
                rustix::fs::fchmod(&dir, Mode::RWXU).map_err(io)?;
                for n in names(&dir)? {
                    let s = rustix::fs::statat(&dir, &n, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)
                        .map_err(io)?;
                    let regular = rustix::fs::FileType::from_raw_mode(s.st_mode)
                        == rustix::fs::FileType::RegularFile;
                    if regular
                        && n.to_str().is_some_and(|n| {
                            Path::new(n).extension().is_none_or(|ext| ext != "part") && keep(id, n)
                        })
                    {
                        // Kept files are counted from disk after all cleanup attempts.
                    } else {
                        maintenance(remove_entry(&dir, &n));
                    }
                }
                rustix::fs::fsync(&dir).map_err(io)?;
                if !keep(id, "") {
                    rustix::fs::unlinkat(&self.root, id, rustix::fs::AtFlags::REMOVEDIR)
                        .map_err(io)?;
                }
                Ok(())
            })();
            maintenance(result);
        }
        maintenance(rustix::fs::fsync(&self.root).map_err(io));
        Ok(disk_bytes(&self.root, active_spool).unwrap_or_else(|_| {
            eprintln!(
                "Source staging usage is unknown; writes remain blocked until reconciliation."
            );
            Usage {
                bytes: 0,
                unknown: true,
            }
        }))
    }
    /// # Errors
    /// Returns invalid paths, deletion/sync failures or accounting mismatches.
    pub fn discard(&self, id: &str, ledger: &Ledger) -> Result<(), AppError> {
        let dir = self.dir(id)?;
        for n in names(&dir)? {
            self.remove(
                id,
                n.to_str()
                    .ok_or(AppError::Invalid("Invalid staging name."))?,
                ledger,
            )?;
        }
        rustix::fs::unlinkat(&self.root, id, rustix::fs::AtFlags::REMOVEDIR).map_err(io)?;
        rustix::fs::fsync(&self.root).map_err(io)?;
        Ok(())
    }
}
fn normalized_dir(parent: &OwnedFd, name: &str) -> Result<OwnedFd, AppError> {
    normalized_dir_impl(
        parent,
        name,
        #[cfg(test)]
        |_| {},
    )
}
#[cfg(test)]
fn normalized_dir_with_hook(
    parent: &OwnedFd,
    name: &str,
    hook: impl FnMut(u8),
) -> Result<OwnedFd, AppError> {
    normalized_dir_impl(parent, name, hook)
}
fn normalized_dir_impl(
    parent: &OwnedFd,
    name: &str,
    #[cfg(test)] mut hook: impl FnMut(u8),
) -> Result<OwnedFd, AppError> {
    use std::os::fd::AsRawFd;
    #[cfg(test)]
    hook(0);
    // O_PATH needs no read permission, even if umask removed owner read/search.
    // Keep this inode pinned across chmod and the subsequent readable open.
    let pinned = rustix::fs::openat(
        parent,
        name,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io)?;
    let stat = rustix::fs::fstat(&pinned).map_err(io)?;
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::Directory {
        return Err(AppError::Invalid("Invalid staging directory."));
    }
    #[cfg(test)]
    hook(1);
    // This procfs magic link resolves to the pinned descriptor, never the mutable
    // directory entry. Refuse if procfs is unavailable; no following-name fallback.
    rustix::fs::chmodat(
        rustix::fs::CWD,
        format!("/proc/self/fd/{}", pinned.as_raw_fd()),
        Mode::RWXU,
        rustix::fs::AtFlags::empty(),
    )
    .map_err(|error| {
        eprintln!("Staging directory permission normalisation failed: {error}");
        AppError::Invalid("Staging directory permissions could not be normalised safely.")
    })?;
    #[cfg(test)]
    hook(2);
    let dir = rustix::fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io)?;
    let opened = rustix::fs::fstat(&dir).map_err(io)?;
    if (stat.st_dev, stat.st_ino) != (opened.st_dev, opened.st_ino) {
        return Err(AppError::Invalid("Staging directory changed."));
    }
    Ok(dir)
}

#[test]
fn normalization_never_chmods_a_swapped_symlink_target() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    for point in 0..=2 {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path();
        std::fs::create_dir(path.join("directory")).unwrap();
        std::fs::write(path.join("target"), b"unchanged").unwrap();
        std::fs::set_permissions(path.join("target"), std::fs::Permissions::from_mode(0o600))
            .unwrap();
        let parent = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .unwrap();
        let result = normalized_dir_with_hook(&parent, "directory", |at| {
            if at == point {
                std::fs::rename(path.join("directory"), path.join("pinned")).unwrap();
                symlink(path.join("target"), path.join("directory")).unwrap();
            }
        });
        assert!(result.is_err(), "swap point {point}");
        assert_eq!(
            std::fs::metadata(path.join("target"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600,
            "swap point {point} changed target mode"
        );
    }
}
#[test]
fn normalization_refuses_a_different_real_directory() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path();
    std::fs::create_dir(path.join("directory")).unwrap();
    let parent = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .unwrap();
    let result = normalized_dir_with_hook(&parent, "directory", |at| {
        if at == 2 {
            std::fs::rename(path.join("directory"), path.join("pinned")).unwrap();
            std::fs::create_dir(path.join("directory")).unwrap();
        }
    });
    assert!(matches!(
        result,
        Err(AppError::Invalid("Staging directory changed."))
    ));
}

fn remove_entry(dir: &OwnedFd, name: &std::ffi::OsStr) -> Result<(), AppError> {
    let stat = rustix::fs::statat(dir, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW).map_err(io)?;
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) == rustix::fs::FileType::Directory {
        let child = rustix::fs::openat(
            dir,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io)?;
        purge(&child)?;
        rustix::fs::unlinkat(dir, name, rustix::fs::AtFlags::REMOVEDIR).map_err(io)?;
    } else {
        rustix::fs::unlinkat(dir, name, rustix::fs::AtFlags::empty()).map_err(io)?;
    }
    Ok(())
}
fn purge(dir: &OwnedFd) -> Result<(), AppError> {
    let mut failed = false;
    for name in names(dir)? {
        if let Err(e) = remove_entry(dir, &name) {
            maintenance(Err(e));
            failed = true;
        }
    }
    rustix::fs::fsync(dir).map_err(io)?;
    if failed {
        Err(AppError::Worker)
    } else {
        Ok(())
    }
}
fn maintenance(result: Result<(), AppError>) {
    result.unwrap_or_else(|_| {
        eprintln!("Source staging cleanup failed; retry on the next sweep.");
    });
}
fn disk_bytes(dir: &OwnedFd, skip_name: Option<&str>) -> Result<Usage, AppError> {
    let mut total = Usage::default();
    for name in names(dir)? {
        if name.to_str().is_some_and(|name| Some(name) == skip_name) {
            continue;
        }
        let measured = (|| {
            let s = rustix::fs::statat(dir, &name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)
                .map_err(io)?;
            let usage = match rustix::fs::FileType::from_raw_mode(s.st_mode) {
                rustix::fs::FileType::Directory => {
                    let child = rustix::fs::openat(
                        dir,
                        &name,
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map_err(io)?;
                    disk_bytes(&child, None)?
                }
                rustix::fs::FileType::RegularFile => Usage {
                    bytes: u64::try_from(s.st_size).map_err(|_| space())?,
                    unknown: false,
                },
                _ => Usage::default(),
            };
            Ok::<_, AppError>(usage)
        })();
        let usage = measured.unwrap_or_else(|_| {
            eprintln!(
                "Source staging usage is unknown; writes remain blocked until reconciliation."
            );
            Usage {
                bytes: 0,
                unknown: true,
            }
        });
        total.bytes = total.bytes.saturating_add(usage.bytes);
        total.unknown |= usage.unknown;
    }
    Ok(total)
}
fn names(fd: &OwnedFd) -> Result<Vec<std::ffi::OsString>, AppError> {
    let mut dir = rustix::fs::Dir::read_from(fd).map_err(io)?;
    let mut result = Vec::new();
    for entry in &mut dir {
        let entry = entry.map_err(io)?;
        let n = entry.file_name().to_bytes();
        if n != b"." && n != b".." {
            result.push(std::ffi::OsStr::from_bytes(n).to_os_string());
        }
    }
    Ok(result)
}
fn valid_id(id: &str) -> Result<(), AppError> {
    if id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(AppError::Invalid("Invalid upload id."))
    }
}
fn valid_name(n: &str) -> Result<(), AppError> {
    if n.is_empty() || n.contains('/') || n == "." || n == ".." || n.contains('\0') {
        Err(AppError::Invalid("Invalid staging name."))
    } else {
        Ok(())
    }
}
