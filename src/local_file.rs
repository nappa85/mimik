//! Shared descriptor-based bounded local reads; no archive or remote retrieval.
use crate::AppError;
use rustix::{
    fd::OwnedFd,
    fs::{Mode, OFlags},
};
use std::{fs::File, io::Read, path::Path};

pub(crate) fn flags() -> OFlags {
    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK
}

pub(crate) fn read(fd: OwnedFd, limit: usize) -> Result<Vec<u8>, AppError> {
    let file = File::from(fd);
    let metadata = file.metadata()?;
    let limit_u64 =
        u64::try_from(limit).map_err(|_| AppError::Invalid("Local file limit is invalid."))?;
    if !metadata.is_file() || metadata.len() > limit_u64 {
        return Err(AppError::Invalid(
            "Local input must be a bounded regular file.",
        ));
    }
    let mut bytes = Vec::new();
    file.take(
        limit_u64
            .checked_add(1)
            .ok_or(AppError::Invalid("Local file limit overflow."))?,
    )
    .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(AppError::Invalid("Local input exceeds its byte limit."));
    }
    Ok(bytes)
}

pub(crate) fn read_path(path: &Path, limit: usize) -> Result<Vec<u8>, AppError> {
    read(
        rustix::fs::open(path, flags(), Mode::empty()).map_err(std::io::Error::from)?,
        limit,
    )
}
