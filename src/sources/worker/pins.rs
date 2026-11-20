//! Identity pins of a staged document and the descriptor they were taken from (SRC-15 slice 3).
//!
//! The staged PDF is read by the worker lazily for the whole run, so nothing but the file's own
//! identity says that the bytes the parent verified are the bytes the worker parsed. A job opens
//! the file without following links, refuses anything but a private regular file, takes the pins
//! before it hashes, and compares them again after every worker exit and at the job end.
use super::{Cancel, Error, Reason};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, Metadata, Permissions},
    os::unix::fs::{FileExt, MetadataExt, PermissionsExt},
};

/// Bytes per hashing read; one read is the cancellation granularity.
const HASH_CHUNK: usize = 1 << 20;

/// What a file must still look like: device, inode, size and both change times. The change time
/// cannot be set by an unprivileged user, so equal pins mean no write, truncation, link change or
/// replacement since they were taken (to the clock's granularity). Per job, never persisted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pins {
    dev: u64,
    ino: u64,
    size: u64,
    mtime_ns: i128,
    ctime_ns: i128,
}

fn nanoseconds(seconds: i64, nanoseconds: i64) -> i128 {
    i128::from(seconds) * 1_000_000_000 + i128::from(nanoseconds)
}

impl Pins {
    /// The pins of an open file's `fstat`.
    #[must_use]
    pub fn of(metadata: &Metadata) -> Self {
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            size: metadata.len(),
            mtime_ns: nanoseconds(metadata.mtime(), metadata.mtime_nsec()),
            ctime_ns: nanoseconds(metadata.ctime(), metadata.ctime_nsec()),
        }
    }

    /// Whether `metadata` is a regular file with exactly these pins.
    #[must_use]
    pub fn holds(&self, metadata: &Metadata) -> bool {
        metadata.is_file() && Self::of(metadata) == *self
    }

    /// Device and inode, the identity the sandbox compares with the mounted object.
    #[must_use]
    pub(crate) fn identity(&self) -> (u64, u64) {
        (self.dev, self.ino)
    }
}

/// A staged document held open with its pins. Everything the job does with the file goes
/// through this descriptor, never through a name resolved again.
pub struct Held {
    file: File,
    pins: Pins,
}

impl Held {
    /// Pin a file that was opened without following links.
    ///
    /// # Errors
    /// `DocumentChanged` unless the file is a regular file with one link, owned by the effective
    /// user; `Unreadable` if it cannot be examined.
    pub fn new(file: File) -> Result<Self, Reason> {
        let metadata = file.metadata().map_err(|_| Error::Unreadable)?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != rustix::process::geteuid().as_raw()
        {
            return Err(Reason::DocumentChanged);
        }
        Ok(Self {
            pins: Pins::of(&metadata),
            file,
        })
    }

    /// The pins taken when the file was opened, before any hashing.
    #[must_use]
    pub fn pins(&self) -> &Pins {
        &self.pins
    }

    /// Whether the held descriptor still shows the pinned file with one link. A failed `fstat`
    /// counts as changed.
    #[must_use]
    pub fn unchanged(&self) -> bool {
        self.file
            .metadata()
            .is_ok_and(|metadata| self.pins.holds(&metadata) && metadata.nlink() == 1)
    }

    /// SHA-256 of the whole file in 1 MiB reads, checking `cancel` before each read.
    ///
    /// # Errors
    /// `Cancelled`, or `Unreadable` on a read failure.
    pub fn sha256(&self, cancel: &Cancel) -> Result<String, Reason> {
        let mut digest = Sha256::new();
        let mut buffer = vec![0; HASH_CHUNK];
        let mut at = 0_u64;
        loop {
            if cancel.cancelled() {
                return Err(Reason::Cancelled);
            }
            match self.file.read_at(&mut buffer, at) {
                Ok(0) => break,
                Ok(count) => {
                    digest.update(&buffer[..count]);
                    at += count as u64;
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return Err(Error::Unreadable.into()),
            }
        }
        Ok(hex::encode(digest.finalize()))
    }

    /// Owner read-only if the file is still 0600 (hardening, not a boundary). Changes the
    /// change time: call it after the last pin check.
    ///
    /// # Errors
    /// Returns the `fchmod` failure.
    pub fn harden(&self) -> std::io::Result<()> {
        if self.file.metadata()?.permissions().mode() & 0o777 == 0o600 {
            self.file.set_permissions(Permissions::from_mode(0o400))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn staged(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("document.pdf");
        std::fs::write(&path, bytes).unwrap();
        (dir, path)
    }

    #[test]
    fn pins_see_a_same_size_in_place_write() {
        let (_dir, path) = staged(b"%PDF-1.7\n0123456789");
        let held = Held::new(File::open(&path).unwrap()).unwrap();
        assert!(held.unchanged());
        std::thread::sleep(std::time::Duration::from_millis(20));
        let writer = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        writer.write_all_at(b"X", 10).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 19);
        assert!(
            !held.unchanged(),
            "only mtime and ctime tell a same-size write"
        );
    }

    #[test]
    fn pins_see_a_write_a_truncation_and_a_new_link() {
        let (dir, path) = staged(b"%PDF-1.7\n0123456789");
        let held = Held::new(File::open(&path).unwrap()).unwrap();
        assert!(held.unchanged());
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"x").unwrap();
        assert!(!held.unchanged(), "an append changes size and times");

        let (dir2, path2) = staged(b"%PDF-1.7\n0123456789");
        let held = Held::new(File::open(&path2).unwrap()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path2)
            .unwrap()
            .set_len(4)
            .unwrap();
        assert!(!held.unchanged(), "a truncation is seen");

        let (dir3, path3) = staged(b"%PDF-1.7\n0123456789");
        let held = Held::new(File::open(&path3).unwrap()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::hard_link(&path3, dir3.path().join("other")).unwrap();
        assert!(!held.unchanged(), "a second link is seen");
        drop((dir, dir2, dir3));
    }

    #[test]
    fn a_hard_linked_or_non_regular_file_is_refused_at_pinning() {
        let (dir, path) = staged(b"%PDF-1.7\n");
        std::fs::hard_link(&path, dir.path().join("other")).unwrap();
        assert_eq!(
            Held::new(File::open(&path).unwrap()).err(),
            Some(Reason::DocumentChanged)
        );
        assert_eq!(
            Held::new(File::open(dir.path()).unwrap()).err(),
            Some(Reason::DocumentChanged)
        );
    }

    #[test]
    fn the_hash_is_the_sha256_of_the_whole_file_and_stops_when_cancelled() {
        let bytes = vec![7_u8; HASH_CHUNK * 2 + 5];
        let (_dir, path) = staged(&bytes);
        let held = Held::new(File::open(&path).unwrap()).unwrap();
        assert_eq!(
            held.sha256(&Cancel::default()).unwrap(),
            hex::encode(Sha256::digest(&bytes))
        );
        let cancel = Cancel::default();
        cancel.cancel();
        assert_eq!(held.sha256(&cancel), Err(Reason::Cancelled));
    }

    #[test]
    fn harden_makes_a_private_file_read_only_and_leaves_other_modes() {
        let (_dir, path) = staged(b"%PDF-1.7\n");
        std::fs::set_permissions(&path, Permissions::from_mode(0o600)).unwrap();
        let held = Held::new(File::open(&path).unwrap()).unwrap();
        held.harden().unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o400);
        std::fs::set_permissions(&path, Permissions::from_mode(0o640)).unwrap();
        held.harden().unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o640);
    }
}
