//! Verified descriptors (assisted sheet plan §1.4a, "Model and runtime files"): the model and
//! every runtime file is opened relative to its folder's descriptor with `openat2`
//! (`RESOLVE_BENEATH`, `RESOLVE_NO_SYMLINKS`, `RESOLVE_NO_MAGICLINKS`) and `O_NOFOLLOW |
//! O_NONBLOCK` (a FIFO cannot block), checked with `fstat`, and hashed from that descriptor.
//! The descriptor stays open and is what the child loads, so a file cannot be swapped between
//! the check and the load. A hash is reused while the file's `fstat` identity (device, inode,
//! size, modification and change times) is the one last hashed.
//!
//! Threat boundary: a process running as Mimik's user could still rewrite an inode in place;
//! it could rewrite Mimik itself too.
use super::manifest::Build;
use rustix::fs::{FileType, Mode, OFlags, ResolveFlags, Stat};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::Mutex;

/// Why a file or folder is refused, with the name shown on the assistant page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Absent, or a symbolic link anywhere in its path.
    Missing(String),
    Link(String),
    NotRegular(String),
    NotDirectory(String),
    /// Owned by another user (the runtime may also be root's).
    Owner(String),
    /// Writable by group or others.
    Writable(String),
    /// A folder's mode is not 0700.
    Mode(String),
    /// Size or SHA-256 differ from the pinned value.
    Changed(String),
    /// A file the manifest does not name, or one it names is missing.
    Extra(String),
    /// Something already exists where a fresh directory must be made.
    Exists(String),
    Io(String),
    /// The work that asked was cancelled.
    Cancelled,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "cancelled"),
            Self::Missing(name) => write!(f, "{name} is missing"),
            Self::Link(name) => write!(f, "{name} is or passes through a symbolic link"),
            Self::NotRegular(name) => write!(f, "{name} is not a regular file"),
            Self::NotDirectory(name) => write!(f, "{name} is not a directory"),
            Self::Owner(name) => write!(f, "{name} belongs to another user"),
            Self::Writable(name) => write!(f, "{name} is writable by group or others"),
            Self::Mode(name) => write!(f, "{name} must have mode 0700"),
            Self::Changed(name) => write!(f, "{name} differs from the pinned file"),
            Self::Extra(name) => write!(f, "{name} is not part of the pinned build"),
            Self::Exists(name) => write!(f, "{name} already exists"),
            Self::Io(name) => write!(f, "{name} cannot be read"),
        }
    }
}

/// The `fstat` identity a hash is valid for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

impl Identity {
    fn of(stat: &Stat) -> Self {
        Self {
            device: stat.st_dev,
            inode: stat.st_ino,
            size: u64::try_from(stat.st_size).unwrap_or(0),
            mtime: (
                stat.st_mtime,
                i64::try_from(stat.st_mtime_nsec).unwrap_or(0),
            ),
            ctime: (
                stat.st_ctime,
                i64::try_from(stat.st_ctime_nsec).unwrap_or(0),
            ),
        }
    }
}

/// An open, checked, hashed file.
#[derive(Debug)]
pub(crate) struct Verified {
    pub name: String,
    pub fd: OwnedFd,
    pub identity: Identity,
    pub sha256: [u8; 32],
}

/// Who may own a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Owners {
    /// Mimik's user only (models, the data directory).
    Mine,
    /// Mimik's user or root (an installed runtime).
    MineOrRoot,
}

impl Owners {
    fn accepts(self, uid: u32) -> bool {
        let me = rustix::process::getuid().as_raw();
        uid == me || (self == Self::MineOrRoot && uid == 0)
    }
}

fn io_refusal(name: &str, error: rustix::io::Errno) -> Refusal {
    match error {
        rustix::io::Errno::NOENT => Refusal::Missing(name.to_owned()),
        // ELOOP: a symbolic link refused by RESOLVE_NO_SYMLINKS or O_NOFOLLOW; EXDEV: an
        // escape refused by RESOLVE_BENEATH.
        rustix::io::Errno::LOOP | rustix::io::Errno::XDEV => Refusal::Link(name.to_owned()),
        rustix::io::Errno::NOTDIR => Refusal::NotDirectory(name.to_owned()),
        _ => Refusal::Io(name.to_owned()),
    }
}

fn resolve() -> ResolveFlags {
    ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS
}

/// Opens a folder by path (its last component not a link) and checks it is a directory of
/// `owners`, not writable by group or others, and of mode 0700 when `private`.
pub(crate) fn open_dir(path: &Path, owners: Owners, private: bool) -> Result<OwnedFd, Refusal> {
    let name = path.display().to_string();
    if rustix::fs::lstat(path)
        .is_ok_and(|stat| FileType::from_raw_mode(stat.st_mode) == FileType::Symlink)
    {
        return Err(Refusal::Link(name));
    }
    // Links in earlier components (a symlinked home) are resolved once; the real path is then
    // opened without following any link, so one put in meanwhile fails the open.
    let real = std::fs::canonicalize(path).map_err(|error| {
        error
            .raw_os_error()
            .map_or(Refusal::Io(name.clone()), |code| {
                io_refusal(&name, rustix::io::Errno::from_raw_os_error(code))
            })
    })?;
    let root = rustix::fs::open(
        "/",
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| io_refusal("/", error))?;
    let relative = real.strip_prefix("/").unwrap_or(&real);
    let fd = rustix::fs::openat2(
        &root,
        if relative.as_os_str().is_empty() {
            Path::new(".")
        } else {
            relative
        },
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
    )
    .map_err(|error| match error {
        // `O_DIRECTORY | O_NOFOLLOW` on a link answers ENOTDIR; tell a link apart.
        rustix::io::Errno::NOTDIR
            if rustix::fs::lstat(path)
                .is_ok_and(|stat| FileType::from_raw_mode(stat.st_mode) == FileType::Symlink) =>
        {
            Refusal::Link(name.clone())
        }
        error => io_refusal(&name, error),
    })?;
    check_dir(&fd, &name, owners, private)?;
    Ok(fd)
}

/// Opens folder `name` beneath `parent` with the same checks as [`open_dir`].
pub(crate) fn open_dir_beneath(
    parent: BorrowedFd<'_>,
    name: &str,
    owners: Owners,
    private: bool,
) -> Result<OwnedFd, Refusal> {
    let fd = rustix::fs::openat2(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        resolve(),
    )
    .map_err(|error| io_refusal(name, error))?;
    check_dir(&fd, name, owners, private)?;
    Ok(fd)
}

fn check_dir(fd: &OwnedFd, name: &str, owners: Owners, private: bool) -> Result<(), Refusal> {
    let stat = rustix::fs::fstat(fd).map_err(|error| io_refusal(name, error))?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
        return Err(Refusal::NotDirectory(name.to_owned()));
    }
    if !owners.accepts(stat.st_uid) {
        return Err(Refusal::Owner(name.to_owned()));
    }
    let mode = stat.st_mode & 0o7777;
    if private && mode != 0o700 {
        return Err(Refusal::Mode(name.to_owned()));
    }
    if mode & 0o022 != 0 {
        return Err(Refusal::Writable(name.to_owned()));
    }
    Ok(())
}

/// Opens regular file `name` beneath `dir` and checks it: a regular file of `owners`, not
/// writable by group or others. Returns the descriptor and its `fstat`.
pub(crate) fn open_file(
    dir: BorrowedFd<'_>,
    name: &str,
    owners: Owners,
) -> Result<(OwnedFd, Stat), Refusal> {
    let fd = rustix::fs::openat2(
        dir,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
        resolve(),
    )
    .map_err(|error| io_refusal(name, error))?;
    let stat = rustix::fs::fstat(&fd).map_err(|error| io_refusal(name, error))?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(Refusal::NotRegular(name.to_owned()));
    }
    if !owners.accepts(stat.st_uid) {
        return Err(Refusal::Owner(name.to_owned()));
    }
    if stat.st_mode & 0o022 != 0 {
        return Err(Refusal::Writable(name.to_owned()));
    }
    // A non-blocking read of a regular file behaves as a blocking one; clear the flag so the
    // descriptor handed to the child is an ordinary one.
    rustix::fs::fcntl_setfl(&fd, OFlags::empty()).map_err(|error| io_refusal(name, error))?;
    Ok((fd, stat))
}

/// SHA-256 of a descriptor's content, read by offset from 0 (the descriptor's own offset is
/// left alone).
pub(crate) fn hash(
    fd: BorrowedFd<'_>,
    name: &str,
    stop: &dyn Fn() -> bool,
) -> Result<[u8; 32], Refusal> {
    let file = File::from(
        fd.try_clone_to_owned()
            .map_err(|_| Refusal::Io(name.to_owned()))?,
    );
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    let mut offset = 0_u64;
    loop {
        // Every MiB: a cancelled start stops hashing a 2 GiB model within moments.
        if stop() {
            return Err(Refusal::Cancelled);
        }
        let read = file
            .read_at(&mut buffer, offset)
            .map_err(|_| Refusal::Io(name.to_owned()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        offset += read as u64;
    }
    Ok(hasher.finalize().into())
}

/// Hashes of files by their identity, so a retry does not re-read a 2 GiB model.
#[derive(Default)]
pub(crate) struct HashCache {
    known: Mutex<BTreeMap<String, (Identity, [u8; 32])>>,
}

impl HashCache {
    /// The descriptor's SHA-256: the remembered one while its identity is unchanged.
    pub(crate) fn hash(
        &self,
        key: &str,
        fd: BorrowedFd<'_>,
        identity: Identity,
        stop: &dyn Fn() -> bool,
    ) -> Result<[u8; 32], Refusal> {
        let mut known = self
            .known
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((seen, sha)) = known.get(key)
            && *seen == identity
        {
            return Ok(*sha);
        }
        let sha = hash(fd, key, stop)?;
        known.insert(key.to_owned(), (identity, sha));
        Ok(sha)
    }
}

fn verified(
    dir: BorrowedFd<'_>,
    name: &str,
    owners: Owners,
    cache: &HashCache,
    key: &str,
    stop: &dyn Fn() -> bool,
) -> Result<Verified, Refusal> {
    let (fd, stat) = open_file(dir, name, owners)?;
    let identity = Identity::of(&stat);
    let sha256 = cache.hash(key, fd.as_fd(), identity, stop)?;
    Ok(Verified {
        name: name.to_owned(),
        fd,
        identity,
        sha256,
    })
}

/// The SHA-256 and size of the model `name` in the models folder, opened with the same
/// checks as a load (the hash is remembered by the file's identity, so the load that
/// follows does not read it again), stopping once `stop` says so.
pub(crate) fn model_digest(
    models: BorrowedFd<'_>,
    name: &str,
    cache: &HashCache,
    stop: &dyn Fn() -> bool,
) -> Result<([u8; 32], u64), Refusal> {
    let file = verified(
        models,
        name,
        Owners::Mine,
        cache,
        &format!("model/{name}"),
        stop,
    )?;
    Ok((file.sha256, file.identity.size))
}

/// Opens and verifies the model `name` in the models folder against `sha256` (64 lowercase
/// hex digits).
#[cfg(test)]
pub(crate) fn model(
    models: BorrowedFd<'_>,
    name: &str,
    sha256: &str,
    cache: &HashCache,
) -> Result<Verified, Refusal> {
    model_until(models, name, sha256, cache, &|| false)
}

/// [`model`], stopping once `stop` says so.
pub(crate) fn model_until(
    models: BorrowedFd<'_>,
    name: &str,
    sha256: &str,
    cache: &HashCache,
    stop: &dyn Fn() -> bool,
) -> Result<Verified, Refusal> {
    let file = verified(
        models,
        name,
        Owners::Mine,
        cache,
        &format!("model/{name}"),
        stop,
    )?;
    if hex::encode(file.sha256) != sha256 {
        return Err(Refusal::Changed(name.to_owned()));
    }
    Ok(file)
}

/// Opens and verifies a build's directory beneath the runtime directory: it holds exactly the
/// manifest's files, each regular, not writable by group or others, of its pinned size and
/// SHA-256. The returned files are in manifest order.
#[cfg(test)]
pub(crate) fn runtime(
    runtime: BorrowedFd<'_>,
    build: &Build,
    cache: &HashCache,
) -> Result<Vec<Verified>, Refusal> {
    let dir = open_dir_beneath(runtime, &build.directory, Owners::MineOrRoot, false)?;
    runtime_until(dir.as_fd(), build, cache, &|| false)
}

/// [`runtime`], stopping once `stop` says so.
pub(crate) fn runtime_until(
    dir: BorrowedFd<'_>,
    build: &Build,
    cache: &HashCache,
    stop: &dyn Fn() -> bool,
) -> Result<Vec<Verified>, Refusal> {
    let mut present = std::collections::BTreeSet::new();
    let listing =
        rustix::fs::Dir::read_from(dir).map_err(|error| io_refusal(&build.directory, error))?;
    for entry in listing {
        let entry = entry.map_err(|error| io_refusal(&build.directory, error))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name != "." && name != ".." {
            present.insert(name);
        }
    }
    for name in &present {
        if !build.files.iter().any(|file| &file.name == name) {
            return Err(Refusal::Extra(name.clone()));
        }
    }
    let mut files = Vec::with_capacity(build.files.len());
    for file in &build.files {
        if !present.contains(&file.name) {
            return Err(Refusal::Missing(file.name.clone()));
        }
        let key = format!("runtime/{}/{}", build.directory, file.name);
        let opened = verified(
            dir.as_fd(),
            &file.name,
            Owners::MineOrRoot,
            cache,
            &key,
            stop,
        )?;
        if opened.identity.size != file.size || hex::encode(opened.sha256) != file.sha256 {
            return Err(Refusal::Changed(file.name.clone()));
        }
        files.push(opened);
    }
    Ok(files)
}

/// Whether `path` and every ancestor directory are safe for loading libraries by path (levels
/// 2 and 3): owned by root or Mimik's user and not writable by group or others (links
/// resolved first).
#[cfg(test)]
pub(crate) fn ancestors_safe(path: &Path) -> Result<(), Refusal> {
    ancestors_safe_for(path, |uid| Owners::MineOrRoot.accepts(uid))
}

/// [`ancestors_safe`] with the accepted owners given.
#[cfg(test)]
pub(crate) fn ancestors_safe_for(path: &Path, owner: impl Fn(u32) -> bool) -> Result<(), Refusal> {
    let real = std::fs::canonicalize(path).map_err(|_| Refusal::Io(path.display().to_string()))?;
    walk_safe(&real, &owner).map(drop)
}

/// The runtime folder, resolved once: opened at its real path (an earlier link resolved,
/// the last component not a link), its real path read back from that descriptor, and, for
/// the levels that load libraries by path (`by_path`), that exact path walked from `/` by
/// descriptor without following links, every folder root's or Mimik's user's and writable
/// by no one else, ending at the very folder opened. `between` runs after the open (tests).
/// Everything later uses this descriptor and this path, never the configured one again.
pub(crate) fn runtime_folder(
    path: &Path,
    by_path: bool,
    between: impl FnOnce(),
) -> Result<(OwnedFd, std::path::PathBuf), Refusal> {
    let fd = open_dir(path, Owners::MineOrRoot, false)?;
    let shown = path.display().to_string();
    let real = std::fs::read_link(format!("/proc/self/fd/{}", fd.as_raw_fd()))
        .map_err(|_| Refusal::Io(shown.clone()))?;
    between();
    if by_path {
        let walked = walk_safe(&real, &|uid| Owners::MineOrRoot.accepts(uid))?;
        let (opened, end) = (
            rustix::fs::fstat(&fd).map_err(|error| io_refusal(&shown, error))?,
            rustix::fs::fstat(&walked).map_err(|error| io_refusal(&shown, error))?,
        );
        if (opened.st_dev, opened.st_ino) != (end.st_dev, end.st_ino) {
            return Err(Refusal::Changed(real.display().to_string()));
        }
    }
    Ok((fd, real))
}

/// Walks the real path `real` from `/` by descriptor without following any link, checking
/// each folder; returns the last one's descriptor.
fn walk_safe(real: &Path, owner: &dyn Fn(u32) -> bool) -> Result<OwnedFd, Refusal> {
    let mut walked = std::path::PathBuf::from("/");
    let mut dir = rustix::fs::open(
        "/",
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| io_refusal("/", error))?;
    safe_ancestor(&dir, &walked, owner)?;
    for component in real.components().skip(1) {
        let std::path::Component::Normal(name) = component else {
            return Err(Refusal::Io(real.display().to_string()));
        };
        walked.push(name);
        let shown = walked.display().to_string();
        dir = rustix::fs::openat2(
            &dir,
            name,
            OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        )
        .map_err(|error| io_refusal(&shown, error))?;
        safe_ancestor(&dir, &walked, owner)?;
    }
    Ok(dir)
}

/// One folder of [`ancestors_safe`]'s walk: root's or Mimik's user's, writable by no one else.
fn safe_ancestor(dir: &OwnedFd, path: &Path, owner: &dyn Fn(u32) -> bool) -> Result<(), Refusal> {
    let name = path.display().to_string();
    let stat = rustix::fs::fstat(dir).map_err(|error| io_refusal(&name, error))?;
    if !owner(stat.st_uid) {
        return Err(Refusal::Owner(name));
    }
    if stat.st_mode & 0o022 != 0 {
        return Err(Refusal::Writable(name));
    }
    Ok(())
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
