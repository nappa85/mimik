use crate::AppError;
use rustix::fs::{Mode, OFlags};
use std::{
    fs::File,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::Path,
};
use zeroize::Zeroizing;

fn private(metadata: &std::fs::Metadata) -> bool {
    metadata.uid() == rustix::process::geteuid().as_raw() && metadata.mode().trailing_zeros() >= 6
}

pub(super) fn read(path: &Path, limit: usize) -> Result<Zeroizing<Vec<u8>>, AppError> {
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    let file = File::from(fd);
    let meta = file.metadata()?;
    if !meta.is_file() || !private(&meta) || meta.len() > limit as u64 {
        return Err(AppError::Invalid(
            "Credential input must be a bounded private owned regular file.",
        ));
    }
    // Allocate once, already inside Zeroizing; a concurrent growing input cannot
    // induce reallocations leaving earlier password allocations unwiped.
    let mut bytes = Zeroizing::new(vec![0; limit + 1]);
    let mut used = 0;
    let mut file = file;
    while used < bytes.len() {
        let n = file.read(&mut bytes[used..])?;
        if n == 0 {
            break;
        }
        used += n;
    }
    if used > limit {
        return Err(AppError::Invalid(
            "Credential input exceeds its byte limit.",
        ));
    }
    bytes.truncate(used);
    Ok(bytes)
}

#[derive(Clone, Copy, Default)]
pub(super) enum SyncFault {
    #[default]
    None,
    #[cfg(test)]
    File,
    #[cfg(test)]
    Directory,
}

pub(super) fn write(path: &Path, bytes: &[u8], fault: SyncFault) -> Result<(), AppError> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path.file_name().ok_or(AppError::Invalid(
        "Credential destination needs a basename.",
    ))?;
    let directory = File::from(
        rustix::fs::open(
            parent,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?,
    );
    let meta = directory.metadata()?;
    if !meta.is_dir() || !private(&meta) {
        return Err(AppError::Invalid(
            "Credential destination directory must be private and owned.",
        ));
    }
    let mut file = File::from(
        rustix::fs::openat(
            &directory,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(std::io::Error::from)?,
    );
    rustix::fs::fchmod(&file, Mode::RUSR | Mode::WUSR).map_err(std::io::Error::from)?;
    let meta = file.metadata()?;
    if !meta.is_file() || !private(&meta) || meta.mode() & 0o777 != 0o600 {
        return Err(AppError::Invalid(
            "Credential output descriptor is not private.",
        ));
    }
    file.write_all(bytes)?;
    file.flush()?;
    #[cfg(test)]
    if matches!(fault, SyncFault::File) {
        return Err(std::io::Error::other("injected file sync failure").into());
    }
    file.sync_all()?;
    #[cfg(test)]
    if matches!(fault, SyncFault::Directory) {
        return Err(std::io::Error::other("injected directory sync failure").into());
    }
    let _ = fault;
    directory.sync_all()?;
    Ok(())
}
