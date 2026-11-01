//! Private, descriptor-relative staging and reservation accounting.
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
pub const TOTAL: u64 = 2048 * MIB;
pub const MARGIN: u64 = 512 * MIB;
pub const UPLOAD_RESERVATION: u64 = 128 * MIB + 65536;
pub const RENDER_RESERVATION: u64 = 8 * 20 * MIB;

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
}

/// A single instance belongs to the process-wide upload manager.
pub struct Ledger {
    bytes: Mutex<Accounting>,
    free: Box<dyn Fn() -> Result<u64, AppError> + Send + Sync>,
    #[cfg(test)]
    pub bypass_free: std::sync::atomic::AtomicBool,
}
impl Ledger {
    pub fn new(
        used: u64,
        free: impl Fn() -> Result<u64, AppError> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            bytes: Mutex::new(Accounting { used, reserved: 0 }),
            free: Box::new(free),
            #[cfg(test)]
            bypass_free: std::sync::atomic::AtomicBool::new(false),
        })
    }
    /// # Errors
    /// Refuses reservations exceeding total/free-space budgets or a poisoned ledger.
    pub fn reserve(self: &Arc<Self>, bytes: u64) -> Result<Reservation, AppError> {
        let mut a = self.bytes.lock().map_err(|_| AppError::Worker)?;
        let reserved = a.reserved.checked_add(bytes).ok_or_else(space)?;
        if a.used.checked_add(reserved).is_none_or(|n| n > TOTAL)
            || self.free_bytes()? < reserved.checked_add(MARGIN).ok_or_else(space)?
        {
            return Err(space());
        }
        a.reserved = reserved;
        Ok(Reservation {
            ledger: self.clone(),
            remaining: bytes,
        })
    }
    fn free_bytes(&self) -> Result<u64, AppError> {
        #[cfg(test)]
        if self.bypass_free.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(u64::MAX);
        }
        (self.free)()
    }
    /// # Errors
    /// Returns an error if the ledger lock was poisoned.
    pub fn usage(&self) -> Result<(u64, u64), AppError> {
        let a = self.bytes.lock().map_err(|_| AppError::Worker)?;
        Ok((a.used, a.reserved))
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
    /// Replace disk usage after a locked scan, preserving active reservations.
    /// # Errors
    /// Returns an error if the ledger lock was poisoned.
    pub fn reconcile(&self, used: u64) -> Result<(), AppError> {
        self.bytes.lock().map_err(|_| AppError::Worker)?.used = used;
        Ok(())
    }
}
pub struct Reservation {
    ledger: Arc<Ledger>,
    remaining: u64,
}
impl Reservation {
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
}
impl Files {
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
    /// # Errors
    /// Returns invalid-name, unlink or directory-sync failures.
    pub fn remove_spool(&self, name: &str) -> Result<(), AppError> {
        valid_name(name)?;
        match rustix::fs::unlinkat(&self.root, name, rustix::fs::AtFlags::empty()) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => (),
            Err(e) => return Err(io(e)),
        }
        rustix::fs::fsync(&self.root).map_err(io)?;
        Ok(())
    }
    /// # Errors
    /// Refuses identity/size mismatches and propagates rename/sync failures.
    pub fn publish_spool(
        &self,
        name: &str,
        file: &File,
        id: &str,
        reservation: &mut Reservation,
    ) -> Result<(), AppError> {
        valid_name(name)?;
        let dir = self.dir(id)?;
        let meta = file.metadata()?;
        let stat = rustix::fs::statat(&self.root, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)
            .map_err(io)?;
        if !meta.is_file()
            || stat.st_dev != meta.dev()
            || stat.st_ino != meta.ino()
            || meta.len() > reservation.remaining
        {
            return Err(AppError::Invalid("Invalid upload spool."));
        }
        file.sync_all()?;
        rustix::fs::renameat(&self.root, name, &dir, "document.pdf").map_err(io)?;
        reservation.written(meta.len())?;
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
        })
    }
    /// # Errors
    /// Refuses invalid upload ids.
    pub fn path(&self, id: &str) -> Result<PathBuf, AppError> {
        valid_id(id)?;
        Ok(self.path.join(id).join("document.pdf"))
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
    pub fn reconcile(&self, mut keep: impl FnMut(&str, &str) -> bool) -> Result<u64, AppError> {
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
    ) -> Result<u64, AppError> {
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
            TOTAL
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
    normalized_dir_with_hook(parent, name, |_| {})
}
fn normalized_dir_with_hook(
    parent: &OwnedFd,
    name: &str,
    mut hook: impl FnMut(u8),
) -> Result<OwnedFd, AppError> {
    use std::os::fd::AsRawFd;
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
    hook(1);
    // This procfs magic link resolves to the pinned descriptor, never the mutable
    // directory entry. Refuse if procfs is unavailable; no following-name fallback.
    rustix::fs::chmodat(
        rustix::fs::CWD,
        format!("/proc/self/fd/{}", pinned.as_raw_fd()),
        Mode::RWXU,
        rustix::fs::AtFlags::empty(),
    )
    .map_err(|_| {
        AppError::Invalid(
            "The umask removes owner permissions; safe directory normalization is unavailable.",
        )
    })?;
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
fn disk_bytes(dir: &OwnedFd, skip_name: Option<&str>) -> Result<u64, AppError> {
    let mut total = 0_u64;
    for name in names(dir)? {
        if name.to_str().is_some_and(|name| Some(name) == skip_name) {
            continue;
        }
        let measured = (|| {
            let s = rustix::fs::statat(dir, &name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)
                .map_err(io)?;
            let bytes = match rustix::fs::FileType::from_raw_mode(s.st_mode) {
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
                rustix::fs::FileType::RegularFile => {
                    u64::try_from(s.st_size).map_err(|_| space())?
                }
                _ => 0,
            };
            Ok::<_, AppError>(bytes)
        })();
        let bytes = measured.unwrap_or_else(|_| {
            eprintln!(
                "Source staging usage is unknown; writes remain blocked until reconciliation."
            );
            TOTAL
        });
        total = total.saturating_add(bytes);
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
