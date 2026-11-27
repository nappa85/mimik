//! One helper for the data folder's private subfolders (`sources/`, `secrets/`): open the data
//! folder without following a link, create the subfolder 0700, open it without following a
//! link and refuse it when another user owns it or it is accessible to group or others. The two
//! stores differ only in the folder's name and in the refusal message, which the caller passes.
use crate::AppError;
use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;
use std::os::fd::OwnedFd;
use std::path::Path;

/// A rustix `Errno` as an [`AppError`].
pub(crate) fn io(error: Errno) -> AppError {
    std::io::Error::from(error).into()
}

/// Opens (creating 0700) the private subfolder `name` of the data folder `data`, and returns its
/// descriptor. `not_private` is the error message when the folder is another user's or accessible
/// to group or others.
///
/// # Errors
/// Refuses a link in place of the data folder or of the subfolder, a folder another user owns or
/// one that is accessible to group or others, and filesystem failures.
pub(crate) fn open_private_subfolder(
    data: &Path,
    name: &str,
    not_private: &'static str,
) -> Result<OwnedFd, AppError> {
    let parent = rustix::fs::open(
        data,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io)?;
    match rustix::fs::mkdirat(&parent, name, Mode::RWXU) {
        Ok(()) | Err(Errno::EXIST) => {}
        Err(error) => return Err(io(error)),
    }
    let dir = rustix::fs::openat(
        &parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io)?;
    let stat = rustix::fs::fstat(&dir).map_err(io)?;
    if stat.st_uid != rustix::process::geteuid().as_raw() || stat.st_mode & 0o077 != 0 {
        return Err(AppError::Invalid(not_private));
    }
    Ok(dir)
}
