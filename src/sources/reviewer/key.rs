//! The reviewer's API key file (docs/api-reviewer-plan.md step 3): `reviewer.key` in the data
//! folder's `secrets/` directory, outside the database and outside backups (a backup is the
//! database file plus the `sources/` folder beside it). The folder is 0700 and the file 0600
//! whatever the umask (the temporary file is `fchmod`ed, not just created with a mode); a write
//! is atomic (a temporary file in the same folder, fsync, rename, directory fsync), so a rename
//! replaces the file whatever its previous mode or kind was. The key is never shown back (the
//! page reports only "set" and the change time, or that the file is not private), never logged
//! and never stored in the database; its identity for later binding is the change time and
//! nanoseconds of the very descriptor it was read from, plus a digest, held in memory only.
use crate::AppError;
use crate::sources::private_dir::{io, open_private_subfolder};
use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use rustix::io::Errno;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::path::Path;
use zeroize::Zeroizing;

/// The folder of the data folder that holds secret files.
pub(crate) const DIRECTORY: &str = "secrets";
/// The reviewer's key file inside [`DIRECTORY`].
pub(crate) const KEY_FILE: &str = "reviewer.key";
/// The most bytes a key may have.
pub(crate) const KEY_MAX_BYTES: usize = 512;
/// The mode the key file must have: owner read and write only.
const KEY_MODE: u32 = 0o600;

/// The save rule for a key: after trimming the ends, 1 to [`KEY_MAX_BYTES`] visible ASCII
/// characters (0x21 to 0x7E) — no spaces, no line breaks, no other control byte, no non-ASCII
/// byte. Enforced on the page's save and again by [`Secrets::write`].
pub(crate) fn valid_key(key: &[u8]) -> bool {
    let trimmed = trim_ends(key);
    !trimmed.is_empty()
        && trimmed.len() <= KEY_MAX_BYTES
        && trimmed.iter().all(|byte| (0x21..=0x7E).contains(byte))
}

/// The read rule, with no trimming at read time: the stored bytes themselves must satisfy the
/// save rule — 1 to [`KEY_MAX_BYTES`] bytes, each visible ASCII (0x21 to 0x7E), so no leading
/// or trailing whitespace. A file holding `key\n` is read as stored but is not a usable key.
pub(crate) fn valid_stored_key(key: &[u8]) -> bool {
    !key.is_empty()
        && key.len() <= KEY_MAX_BYTES
        && key.iter().all(|byte| (0x21..=0x7E).contains(byte))
}

/// The fixed refusal message for a stored key that breaks the read rule
/// ([`valid_stored_key`]). It never carries the key.
pub(crate) const INVALID_STORED_KEY: &str =
    "The key file does not hold a valid key: replace or delete it.";

/// The fixed refusal message for a key that breaks the save rule. It states [`KEY_MAX_BYTES`];
/// the assertion below keeps the two from drifting apart.
pub(crate) const KEY_RULE: &str =
    "The API key must be 1 to 512 visible ASCII characters (no spaces or line breaks)";
const _: () = assert!(
    KEY_MAX_BYTES == 512,
    "KEY_RULE states the bound: update it too"
);

/// The ends trimmed (ASCII whitespace), as the save rule trims them.
fn trim_ends(key: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = key.len();
    while start < end && key[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && key[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    &key[start..end]
}

/// The key file's status, as the page shows it: whether it is set, and its change time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct KeyStatus {
    /// The file's change time, Unix seconds.
    pub(crate) changed_at: i64,
}

/// The key file as the page sees it: not set, a private file (with its change time), or an entry
/// that is not a private regular file owned by this user (a wrong mode, a link or another
/// user's file) — the page still renders then and says so, with the replace and delete forms
/// kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyPageStatus {
    /// No entry for the key file.
    NotSet,
    /// A private regular file.
    Set(KeyStatus),
    /// An entry exists (a link counts) but is not a private 0600 regular file owned by this
    /// user.
    NotPrivate,
}

/// The key's identity for later binding (step 4's send hash): the file's change time plus a
/// digest of the key. Held in memory only: never stored, never shown.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
pub struct KeyIdentity {
    pub changed_at: i64,
    pub changed_at_nsec: i64,
    pub digest: [u8; 32],
}

/// The data folder's `secrets/` directory, opened and checked like the PDF store's folder.
pub(crate) struct Secrets {
    dir: OwnedFd,
}

/// The key read from its descriptor, with that descriptor's own stat.
type ReadWithStat = (Zeroizing<Vec<u8>>, rustix::fs::Stat);

impl Secrets {
    /// Opens (creating, 0700) the `secrets/` directory inside `data`.
    ///
    /// # Errors
    /// Refuses a link in place of the directory, a directory another user owns or one that is
    /// accessible to group or others, and filesystem failures.
    pub(crate) fn in_directory(data: &Path) -> Result<Self, AppError> {
        let dir = open_private_subfolder(data, DIRECTORY, "The secrets directory is not private.")?;
        Ok(Self { dir })
    }

    /// The key file's entry, stat-ed without following a link; `None` when there is none.
    fn stat_entry(&self) -> Result<Option<rustix::fs::Stat>, AppError> {
        match rustix::fs::statat(&self.dir, KEY_FILE, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => Ok(Some(stat)),
            Err(Errno::NOENT) => Ok(None),
            Err(error) => Err(io(error)),
        }
    }

    /// Whether an entry for the key file exists (a link counts), stat-ed without following it.
    ///
    /// # Errors
    /// Filesystem failures.
    pub(crate) fn entry_exists(&self) -> Result<bool, AppError> {
        Ok(self.stat_entry()?.is_some())
    }

    /// The key file as the page sees it: [`KeyPageStatus`].
    ///
    /// # Errors
    /// Filesystem failures.
    pub(crate) fn page_status(&self) -> Result<KeyPageStatus, AppError> {
        match self.stat_entry()? {
            None => Ok(KeyPageStatus::NotSet),
            Some(stat) => match check_private(&stat) {
                Ok(()) => Ok(KeyPageStatus::Set(KeyStatus {
                    changed_at: stat.st_ctime,
                })),
                // A wrong mode, a link, or a file another user owns: not private.
                Err(_) => Ok(KeyPageStatus::NotPrivate),
            },
        }
    }

    /// The key file's status, read without following a link; `None` when it is not set. A file
    /// that is not a regular 0600 file owned by this user is refused.
    ///
    /// # Errors
    /// The file is a link, is owned by another user or is not private, or a filesystem failure.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "called by the send path of docs/api-reviewer-plan.md step 4"
        )
    )]
    pub(crate) fn status(&self) -> Result<Option<KeyStatus>, AppError> {
        match self.stat_entry()? {
            None => Ok(None),
            Some(stat) => {
                check_private(&stat)?;
                Ok(Some(KeyStatus {
                    changed_at: stat.st_ctime,
                }))
            }
        }
    }

    /// Reads the key with the stat of the very descriptor it was read from, bounded; `None` when
    /// it is not set. The bytes are zeroized on drop.
    fn read_with_stat(&self) -> Result<Option<ReadWithStat>, AppError> {
        let fd = match rustix::fs::openat(
            &self.dir,
            KEY_FILE,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(Errno::NOENT) => return Ok(None),
            Err(error) => return Err(io(error)),
        };
        let stat = rustix::fs::fstat(&fd).map_err(io)?;
        check_private(&stat)?;
        let file = File::from(fd);
        let mut bytes = Zeroizing::new(Vec::new());
        file.take(u64::try_from(KEY_MAX_BYTES).unwrap_or(u64::MAX) + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > KEY_MAX_BYTES {
            return Err(AppError::Invalid("The reviewer key file is too long."));
        }
        if bytes.is_empty() {
            return Ok(None);
        }
        Ok(Some((bytes, stat)))
    }

    /// Reads the key, bounded; `None` when it is not set. The bytes are zeroized on drop.
    ///
    /// # Errors
    /// The file is a link, is owned by another user or not private, is longer than
    /// [`KEY_MAX_BYTES`], or a filesystem failure.
    pub(crate) fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>, AppError> {
        Ok(self.read_with_stat()?.map(|(bytes, _stat)| bytes))
    }

    /// Whether a key is set and the stored bytes themselves satisfy the save rule
    /// ([`valid_stored_key`]: no trimming at read time), without exposing it; `Ok(false)` when it
    /// is not set. The page's readiness check uses this instead of building a reviewer.
    ///
    /// # Errors
    /// As [`Secrets::read`].
    pub(crate) fn ready(&self) -> Result<bool, AppError> {
        Ok(matches!(self.read()?, Some(bytes) if valid_stored_key(&bytes)))
    }

    /// Writes (or replaces) the key atomically: a temporary file in the same folder, `fchmod`ed
    /// 0600 whatever the umask, fsynced, renamed over [`KEY_FILE`], then the folder is fsynced.
    /// The rename replaces the file anyway, so this does not depend on the old file's status.
    /// The ends are trimmed and the save rule ([`valid_key`]) is enforced again here.
    ///
    /// # Errors
    /// The key breaks the save rule, or a filesystem failure.
    pub(crate) fn write(&self, key: &[u8]) -> Result<KeyStatus, AppError> {
        let trimmed = trim_ends(key);
        if !valid_key(trimmed) {
            return Err(AppError::Invalid(KEY_RULE));
        }
        let mut random = [0_u8; 8];
        getrandom::fill(&mut random).map_err(|_| AppError::Worker)?;
        let temporary = format!(".tmp-{}", hex::encode(random));
        let result = (|| {
            let fd = rustix::fs::openat(
                &self.dir,
                &temporary,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(KEY_MODE),
            )
            .map_err(io)?;
            // The create mode is masked by the umask; set the mode exactly.
            rustix::fs::fchmod(&fd, Mode::from_raw_mode(KEY_MODE)).map_err(io)?;
            let mut file = File::from(fd);
            file.write_all(trimmed)?;
            file.sync_all()?;
            drop(file);
            rustix::fs::renameat(&self.dir, &temporary, &self.dir, KEY_FILE).map_err(io)?;
            rustix::fs::fsync(&self.dir).map_err(io)?;
            let stat = self.stat_entry()?.ok_or(AppError::Worker)?;
            check_private(&stat)?;
            Ok(KeyStatus {
                changed_at: stat.st_ctime,
            })
        })();
        let _ = rustix::fs::unlinkat(&self.dir, &temporary, AtFlags::empty());
        result
    }

    /// Deletes the key file and fsyncs the folder; `false` when it was not set.
    ///
    /// # Errors
    /// Filesystem failures.
    pub(crate) fn delete(&self) -> Result<bool, AppError> {
        match rustix::fs::unlinkat(&self.dir, KEY_FILE, AtFlags::empty()) {
            Ok(()) => {
                rustix::fs::fsync(&self.dir).map_err(io)?;
                Ok(true)
            }
            Err(Errno::NOENT) => Ok(false),
            Err(error) => Err(io(error)),
        }
    }

    /// The key's identity: the change time and nanoseconds of the same descriptor the key was
    /// read from, plus a digest of the key, computed on demand and never stored; `None` when no
    /// key is set.
    ///
    /// # Errors
    /// As [`Secrets::read`].
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "called by the send path of docs/api-reviewer-plan.md step 4"
        )
    )]
    pub fn identity(&self) -> Result<Option<KeyIdentity>, AppError> {
        let Some((key, stat)) = self.read_with_stat()? else {
            return Ok(None);
        };
        Ok(Some(KeyIdentity {
            changed_at: stat.st_ctime,
            changed_at_nsec: i64::try_from(stat.st_ctime_nsec).unwrap_or(0),
            digest: Sha256::digest(&**key).into(),
        }))
    }
}

/// Whether the stat is a regular file owned by this user with exactly the key's mode. A file
/// another user owns is not private, whether or not it could be opened.
fn check_private(stat: &rustix::fs::Stat) -> Result<(), AppError> {
    if stat.st_uid != rustix::process::geteuid().as_raw()
        || FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_mode & 0o777 != KEY_MODE
    {
        return Err(AppError::Invalid("The reviewer key file is not private."));
    }
    Ok(())
}

/// The reviewer key's identity (change time plus digest, in memory only) for later binding
/// (step 4's send hash); `None` when no key is set. No use yet.
///
/// # Errors
/// As [`Secrets::read`].
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
pub fn key_identity(store: &crate::Store) -> Result<Option<KeyIdentity>, AppError> {
    store.secrets().identity()
}
