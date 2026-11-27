//! The content-addressed store of source PDFs (SRC-15, decision 9): a private `sources/`
//! directory beside the database, one read-only file per SHA-256, named `<pdf_hash>.pdf`.
//!
//! * A name is always the 64-hex hash a caller passes (from a database row or a verified
//!   upload), never a path or other user input, and every access goes through the directory
//!   descriptor with `O_NOFOLLOW`: no path is built, no link is followed.
//! * [`PdfStore::put`] makes a verified copy (never a hard link, so a staged file keeps its link
//!   count of 1), writing a temporary name, fsyncing, renaming and fsyncing the directory. It is
//!   idempotent. Identical PDFs share one file.
//! * [`PdfStore::open`] hashes the descriptor it opened (not a path) before handing it out; a
//!   missing, replaced, tampered or non-regular file is the fixed [`AppError::PdfCorrupt`].
//! * Orphans: `put` returns a [`Pending`] that holds a shared `flock` on the directory. The
//!   caller keeps it until the database row naming the hash has committed (ordering: temp write,
//!   fsync, rename, directory fsync, database commit). [`PdfStore::sweep`] takes the lock
//!   exclusively and without waiting (a busy sweep is skipped) and removes only unreferenced
//!   files older than one hour. Route A's finalization follows the same rule: `put` before the
//!   first transaction that inserts the `sources` row, and keep the `Pending` until that
//!   transaction commits.
//! * The hash check of [`PdfStore::open`] happens at open. A consumer that needs the identity of
//!   what it reads (route A) hashes while it reads.
//! * Store identity: the database holds a random id (`pdf_store_identity`) and `sources/.store-id`
//!   holds the same. Two databases in one folder share `sources/`; each one's rows name only its
//!   own files, so a sweep would delete the other's. When the marker differs (or a
//!   `.store-shared` file exists) the sweep never deletes; `put` and `open` still work.
use crate::{AppError, Store};
use rusqlite::Connection;
use rustix::{
    fd::OwnedFd,
    fs::{AtFlags, Dir, FileType, FlockOperation, Mode, OFlags},
    io::Errno,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{Arc, OnceLock},
    time::{Duration, SystemTime},
};

use super::private_dir::{io, open_private_subfolder};

const DIRECTORY: &str = "sources";
const MARKER: &str = ".store-id";
const SHARED: &str = ".store-shared";
const TEMPORARY: &str = ".tmp-";
const SUFFIX: &str = ".pdf";
/// Files younger than this are never swept: a writer may be between its rename and its commit.
const MINIMUM_AGE: Duration = Duration::from_secs(3600);

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The directory of stored PDFs.
pub struct PdfStore {
    dir: OwnedFd,
    /// The database's store id, once bound (see [`PdfStore::bind_identity`]).
    identity: OnceLock<String>,
}

/// A stored PDF whose database row has not committed yet. Dropping it releases the shared lock
/// that keeps the orphan sweep away.
#[must_use = "keep the guard until the row naming the hash has committed"]
pub struct Pending(#[allow(dead_code)] OwnedFd);

impl PdfStore {
    /// Opens (creating, 0700) the `sources/` directory inside `data`.
    ///
    /// # Errors
    /// Refuses a link in place of the directory, a directory another user owns or one that is
    /// accessible to group or others, and filesystem failures.
    pub fn in_directory(data: &Path) -> Result<Self, AppError> {
        let dir =
            open_private_subfolder(data, DIRECTORY, "The PDF store directory is not private.")?;
        Ok(Self {
            dir,
            identity: OnceLock::new(),
        })
    }

    /// Reads a small marker file (no link followed); `None` when it does not exist.
    fn read_marker(&self, name: &str) -> Result<Option<Vec<u8>>, AppError> {
        match rustix::fs::openat(
            &self.dir,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        ) {
            Ok(fd) => {
                let mut bytes = Vec::new();
                File::from(fd).take(256).read_to_end(&mut bytes)?;
                Ok(Some(bytes))
            }
            Err(Errno::NOENT) => Ok(None),
            Err(error) => Err(io(error)),
        }
    }

    /// Creates `name` with `bytes` atomically and never replaces an existing file: a temporary
    /// file is written, fsynced and linked into place. `false` when the name already exists.
    fn create_marker(&self, name: &str, bytes: &[u8]) -> Result<bool, AppError> {
        let mut random = [0_u8; 8];
        getrandom::fill(&mut random).map_err(|_| AppError::Worker)?;
        let temporary = format!("{TEMPORARY}{}", hex::encode(random));
        let fd = rustix::fs::openat(
            &self.dir,
            &temporary,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR,
        )
        .map_err(io)?;
        let mut file = File::from(fd);
        let written = file.write_all(bytes).and_then(|()| file.sync_all());
        let linked = written.map_err(AppError::from).and_then(|()| {
            match rustix::fs::linkat(&self.dir, &temporary, &self.dir, name, AtFlags::empty()) {
                Ok(()) => Ok(true),
                Err(Errno::EXIST) => Ok(false),
                Err(error) => Err(io(error)),
            }
        });
        let _ = rustix::fs::unlinkat(&self.dir, &temporary, AtFlags::empty());
        if linked.as_ref().is_ok_and(|created| *created) {
            rustix::fs::fsync(&self.dir).map_err(io)?;
        }
        linked
    }

    /// Binds this store to its database's id: writes `.store-id` when it is missing; when it
    /// holds another id, records `.store-shared` too so no sweep of either database deletes
    /// anything. `put` and `open` are unaffected.
    ///
    /// # Errors
    /// Filesystem failures.
    pub fn bind_identity(&self, id: &str) -> Result<(), AppError> {
        let _ = self.identity.set(id.to_owned());
        if !self.create_marker(MARKER, id.as_bytes())?
            && self.read_marker(MARKER)?.as_deref() != Some(id.as_bytes())
        {
            self.create_marker(SHARED, b"shared")?;
        }
        Ok(())
    }

    /// Whether another database shares this folder (checked on disk, so a later opener is seen).
    ///
    /// # Errors
    /// Filesystem failures.
    pub fn shared(&self) -> Result<bool, AppError> {
        let Some(id) = self.identity.get() else {
            return Ok(false);
        };
        Ok(self.read_marker(SHARED)?.is_some()
            || self.read_marker(MARKER)?.as_deref() != Some(id.as_bytes()))
    }

    /// A new open file description of the directory, so each holder has its own lock.
    fn handle(&self) -> Result<OwnedFd, AppError> {
        rustix::fs::openat(
            &self.dir,
            ".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io)
    }

    fn lock(&self, operation: FlockOperation) -> Result<Option<OwnedFd>, AppError> {
        let handle = self.handle()?;
        loop {
            match rustix::fs::flock(&handle, operation) {
                Ok(()) => return Ok(Some(handle)),
                Err(Errno::INTR) => {}
                Err(Errno::WOULDBLOCK) => return Ok(None),
                Err(error) => return Err(io(error)),
            }
        }
    }

    /// Stores a verified copy of `source` under `sha256` (64 lowercase hex). The bytes are read
    /// from the reader's current position to its end and must hash to `sha256`; nothing is
    /// linked. Idempotent: an intact stored file is kept, a damaged or replaced one is
    /// rewritten.
    ///
    /// # Errors
    /// `Invalid` for a malformed hash or bytes that do not hash to it; filesystem failures.
    pub fn put(&self, mut source: impl Read, sha256: &str) -> Result<Pending, AppError> {
        if !valid_hash(sha256) {
            return Err(AppError::Invalid("The PDF hash is not valid."));
        }
        let name = format!("{sha256}{SUFFIX}");
        // Shared from before the file exists until the caller drops the guard.
        let lock = self
            .lock(FlockOperation::LockShared)?
            .ok_or(AppError::Worker)?;
        match self.open(sha256) {
            Ok(_) => {
                // An earlier writer may have renamed without reaching its directory fsync.
                rustix::fs::fsync(&self.dir).map_err(io)?;
                return Ok(Pending(lock));
            }
            Err(AppError::PdfCorrupt) => {}
            Err(error) => return Err(error),
        }
        let mut random = [0_u8; 8];
        getrandom::fill(&mut random).map_err(|_| AppError::Worker)?;
        let temporary = format!("{TEMPORARY}{}", hex::encode(random));
        let result = self
            .write_temporary(&temporary, &mut source, sha256)
            .and_then(|()| {
                rustix::fs::renameat(&self.dir, &temporary, &self.dir, &name).map_err(io)?;
                rustix::fs::fsync(&self.dir).map_err(io)
            });
        if result.is_err() {
            let _ = rustix::fs::unlinkat(&self.dir, &temporary, AtFlags::empty());
        }
        result.map(|()| Pending(lock))
    }

    fn write_temporary(
        &self,
        temporary: &str,
        source: &mut impl Read,
        sha256: &str,
    ) -> Result<(), AppError> {
        let fd = rustix::fs::openat(
            &self.dir,
            temporary,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR,
        )
        .map_err(io)?;
        let mut file = File::from(fd);
        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; 1 << 20];
        loop {
            let count = match source.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            };
            hasher.update(&buffer[..count]);
            file.write_all(&buffer[..count])?;
        }
        if hex::encode(hasher.finalize()) != sha256 {
            return Err(AppError::Invalid("The PDF does not match its hash."));
        }
        file.sync_all()?;
        Ok(())
    }

    /// Opens the stored PDF for `hash` after hashing the opened descriptor.
    ///
    /// # Errors
    /// [`AppError::PdfCorrupt`] when the file is missing, a link, not a regular file or does not
    /// hash to `hash`; other filesystem failures as they are.
    pub fn open(&self, hash: &str) -> Result<File, AppError> {
        if !valid_hash(hash) {
            return Err(AppError::PdfCorrupt);
        }
        let fd = match rustix::fs::openat(
            &self.dir,
            format!("{hash}{SUFFIX}"),
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK | OFlags::NOCTTY,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(Errno::NOENT | Errno::LOOP | Errno::NOTDIR | Errno::ACCESS | Errno::NXIO) => {
                return Err(AppError::PdfCorrupt);
            }
            Err(error) => return Err(io(error)),
        };
        let mut file = File::from(fd);
        if !file.metadata()?.is_file() {
            return Err(AppError::PdfCorrupt);
        }
        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; 1 << 20];
        loop {
            match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => hasher.update(&buffer[..count]),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error.into()),
            }
        }
        if hex::encode(hasher.finalize()) != hash {
            return Err(AppError::PdfCorrupt);
        }
        file.seek(SeekFrom::Start(0))?;
        Ok(file)
    }

    /// Whether a regular file is stored under `hash` (no content check, no link followed).
    fn present(&self, hash: &str) -> bool {
        valid_hash(hash)
            && rustix::fs::statat(
                &self.dir,
                format!("{hash}{SUFFIX}"),
                AtFlags::SYMLINK_NOFOLLOW,
            )
            .is_ok_and(|stat| FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile)
    }

    /// Removes stored files no row references and stale temporary files, both older than one
    /// hour. `referenced` runs after the exclusive lock is held, so a `put` still waiting for its
    /// row cannot commit between the read and the removals. `Ok(None)` when a writer holds the
    /// lock (the run is skipped), else how many files were removed.
    ///
    /// # Errors
    /// Filesystem failures and the error of `referenced`.
    pub fn sweep(
        &self,
        referenced: impl FnOnce() -> Result<HashSet<String>, AppError>,
    ) -> Result<Option<usize>, AppError> {
        self.sweep_older_than(MINIMUM_AGE, referenced)
    }

    fn sweep_older_than(
        &self,
        minimum_age: Duration,
        referenced: impl FnOnce() -> Result<HashSet<String>, AppError>,
    ) -> Result<Option<usize>, AppError> {
        let Some(_lock) = self.lock(FlockOperation::NonBlockingLockExclusive)? else {
            return Ok(None);
        };
        if self.shared()? {
            eprintln!(
                "warning: another database shares this sources/ folder; the stored-PDF sweep deletes nothing."
            );
            return Ok(None);
        }
        let keep = referenced()?;
        let now = SystemTime::now();
        let mut names = Vec::new();
        for entry in Dir::read_from(&self.dir).map_err(io)? {
            let entry = entry.map_err(io)?;
            let name = entry.file_name().to_bytes();
            if name != b"." && name != b".." {
                names.push(entry.file_name().to_owned());
            }
        }
        let mut removed = 0;
        for name in names {
            let Ok(text) = name.to_str() else { continue };
            let stored = text
                .strip_suffix(SUFFIX)
                .is_some_and(|hash| valid_hash(hash) && !keep.contains(hash));
            if !stored && !text.starts_with(TEMPORARY) {
                continue;
            }
            let Ok(stat) = rustix::fs::statat(&self.dir, &name, AtFlags::SYMLINK_NOFOLLOW) else {
                continue;
            };
            let seconds = u64::try_from(stat.st_mtime).unwrap_or(u64::MAX);
            let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
            let old = now
                .duration_since(modified)
                .is_ok_and(|age| age >= minimum_age);
            if FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile
                && old
                && rustix::fs::unlinkat(&self.dir, &name, AtFlags::empty()).is_ok()
            {
                removed += 1;
            }
        }
        if removed > 0 {
            rustix::fs::fsync(&self.dir).map_err(io)?;
        }
        Ok(Some(removed))
    }
}

/// The hashes of the stored PDFs the rows name.
fn referenced(db: &Connection) -> Result<HashSet<String>, AppError> {
    let mut statement = db.prepare("SELECT DISTINCT pdf_hash FROM sources")?;
    let hashes = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<_, _>>()?;
    Ok(hashes)
}

/// The sweep over a database connection (its rows decide what is referenced).
pub(crate) fn sweep(store: &PdfStore, db: &Connection) -> Result<Option<usize>, AppError> {
    store.sweep(|| referenced(db))
}

impl Store {
    pub(crate) fn pdf_store(&self) -> &Arc<PdfStore> {
        &self.3
    }

    /// Whether another database shares this store folder (its sweeps then delete nothing).
    ///
    /// # Errors
    /// Filesystem failures.
    pub fn pdf_store_shared(&self) -> Result<bool, AppError> {
        self.3.shared()
    }

    /// How many sources have no stored PDF file (a count only: no names, no content check).
    /// Sources imported before migration 037 kept their PDF inside the database, which the
    /// migration dropped; they must be imported again to have one.
    ///
    /// # Errors
    /// Database failures.
    pub async fn missing_pdf_count(&self) -> Result<u64, AppError> {
        let files = self.3.clone();
        self.run(move |db| {
            let mut statement = db.prepare("SELECT pdf_hash FROM sources")?;
            let mut rows = statement.query([])?;
            let mut missing = 0;
            while let Some(row) = rows.next()? {
                if !files.present(&row.get::<_, String>(0)?) {
                    missing += 1;
                }
            }
            Ok(missing)
        })
        .await
    }

    /// Runs the orphan sweep of the PDF store (see [`PdfStore::sweep`]).
    ///
    /// # Errors
    /// Database and filesystem failures.
    pub async fn sweep_pdf_store(&self) -> Result<Option<usize>, AppError> {
        let files = self.3.clone();
        self.run(move |db| sweep(&files, db)).await
    }

    /// Opens the stored PDF of a source, hash-checked.
    ///
    /// # Errors
    /// `SourceNotFound` for an unknown source; [`AppError::PdfCorrupt`] when its file is missing
    /// or damaged.
    pub async fn open_source_pdf(&self, source: i64) -> Result<File, AppError> {
        let files = self.3.clone();
        self.run(move |db| {
            let hash: String = db
                .query_row("SELECT pdf_hash FROM sources WHERE id=?1", [source], |r| {
                    r.get(0)
                })
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => AppError::SourceNotFound,
                    other => other.into(),
                })?;
            files.open(&hash)
        })
        .await
    }
}

#[cfg(test)]
#[path = "pdf_store_tests.rs"]
mod tests;
