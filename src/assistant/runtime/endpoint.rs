//! The endpoint check (assisted sheet plan §1.4a, "Endpoint check"): every connection to the
//! model server is checked to reach the current generation's child before its first byte,
//! and the token goes only there.
//!
//! - Level 1 (the Hakoniwa sandbox, a Unix socket in the generation's directory): `SO_PEERCRED`
//!   must give Mimik's UID and a PID that is the reported `child-pid` (the PID-1 reaper of
//!   the sandbox) or a process whose `PPid` is it; the peer's PID namespace must be the one
//!   reported (`/proc/<pid>/ns/pid`), cross-checked by `NSpid` (two entries, the inner
//!   one small). A UID-only match is never accepted. Mimik connects through
//!   `/proc/self/fd/<directory descriptor>/llama.sock`, so the host path's length never meets
//!   the 108-byte `sun_path` limit, and the reaper's pidfd tells when it has gone.
//! - Level 3 (TCP on `127.0.0.1`): before and after `connect()`, every listening entry
//!   on the port (`127.0.0.1`, the IPv4 wildcard, and the IPv6 wildcard or mapped loopback)
//!   must be a socket open in the child's `/proc/<pid>/fd`; no entry yet means "not ready";
//!   one foreign entry fails the attempt.
use crate::assistant::transport::{Connect, Connecting, Stream, UNIX_AUTHORITY};
use std::collections::BTreeSet;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::Arc;

/// The socket's name in the generation's directory.
pub(crate) const SOCKET: &str = "llama.sock";
/// The largest PID a process of a fresh sandbox has inside it (the reaper is 1 and the
/// server 2; a margin for whatever starts first).
const INNER_PID_MAX: i32 = 64;

/// A process of this generation, held by a pidfd so its exit is seen.
#[derive(Debug)]
pub(crate) struct Watched {
    pub pid: i32,
    pub pidfd: OwnedFd,
}

impl Watched {
    /// Opens a pidfd on `pid`.
    pub(crate) fn open(pid: i32) -> io::Result<Self> {
        let raw = rustix::process::Pid::from_raw(pid).ok_or_else(|| io::Error::other("PID 0"))?;
        let pidfd = rustix::process::pidfd_open(raw, rustix::process::PidfdFlags::empty())?;
        Ok(Self { pid, pidfd })
    }

    /// Whether the process is still running (its pidfd not yet readable).
    pub(crate) fn alive(&self) -> bool {
        !exited(&self.pidfd)
    }
}

/// Whether a pidfd reports its process gone (it polls readable then).
pub(crate) use crate::sandbox::process::exited;

fn refused(what: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, what)
}

const GONE: &str = "the sandbox's reaper has gone";

/// `pid_namespace` when the sandbox's launcher could not read the namespace number (Hakoniwa's
/// reaper is not dumpable): the namespace comparison is then left to the parent and `NSpid`
/// checks. No real namespace has inode 0.
pub(crate) const NAMESPACE_UNREPORTED: u64 = 0;

/// What the runner reported for this generation's sandbox.
#[derive(Debug)]
pub(crate) struct Sandbox {
    /// `child-pid`: the sandbox's PID-1 reaper, in Mimik's PID namespace.
    pub reaper: Watched,
    /// `pid-namespace`: the sandbox's PID namespace inode.
    pub pid_namespace: u64,
    /// Hakoniwa: the sandbox's PID 1 (the server itself), held by its own pidfd. Its reaper
    /// is outside the namespace and can end before the server has finished dying (its memory
    /// is released only then), so the sandbox is gone when this process is.
    pub inner: std::sync::OnceLock<InnerPin>,
}

/// The sandbox's PID 1, pinned. `verified` says its start time matched what the runner
/// reported; one that could not be read keeps the pidfd (waited for until it exits) but is
/// never signalled: an unverified process may be another's.
#[derive(Debug)]
pub(crate) struct InnerPin {
    pub watched: Watched,
    pub verified: bool,
}

/// Level 1's connector: the generation's socket, checked by its peer.
#[derive(Debug)]
pub(crate) struct SandboxSocket {
    pub directory: Arc<OwnedFd>,
    pub sandbox: Arc<Sandbox>,
}

/// The status fields the check reads.
struct Status {
    ppid: i32,
    nspid: Vec<i32>,
}

fn status(pid: i32) -> io::Result<Status> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status"))?;
    let field = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name))
            .map(str::trim)
            .ok_or_else(|| refused("no such status field"))
    };
    let parent = field("PPid:")?.parse().map_err(|_| refused("bad PPid"))?;
    let namespace_pids = field("NSpid:")?
        .split_whitespace()
        .map(|value| value.parse().map_err(|_| refused("bad NSpid")))
        .collect::<io::Result<Vec<i32>>>()?;
    Ok(Status {
        ppid: parent,
        nspid: namespace_pids,
    })
}

/// Opens a pidfd on the reaper the launcher reported and checks the process it pins: a child
/// of the runner (`PPid`), in the reported PID namespace where that is readable. A reaper that
/// has already exited (and is still the runner's unreaped child) pins fine: the caller decides
/// what an ended sandbox means.
pub(crate) fn pin_reaper(pid: i32, runner: i32, namespace: u64) -> io::Result<Watched> {
    let watched = Watched::open(pid)?;
    if status(pid)?.ppid != runner {
        return Err(refused("the reported reaper is not the runner's child"));
    }
    match rustix::fs::stat(format!("/proc/{pid}/ns/pid")) {
        _ if namespace == NAMESPACE_UNREPORTED => {}
        Ok(stat) if stat.st_ino == namespace => {}
        Ok(_) => return Err(refused("the reported reaper is in another PID namespace")),
        Err(rustix::io::Errno::ACCESS | rustix::io::Errno::PERM) => {}
        Err(error) => return Err(error.into()),
    }
    Ok(watched)
}

#[cfg(test)]
/// [`pin_reaper`] and still alive after both reads (so they described this process, not a
/// later one with its PID).
pub(crate) fn open_reaper(pid: i32, runner: i32, namespace: u64) -> io::Result<Watched> {
    let watched = pin_reaper(pid, runner, namespace)?;
    if !watched.alive() {
        return Err(refused(GONE));
    }
    Ok(watched)
}

/// What pinning the server found.
#[derive(Debug)]
pub(crate) enum Inner {
    /// The process the runner reported: same pid and start time.
    Ours(Watched),
    /// Proof it is gone: no process has the pid, or one has it that started at another time
    /// (the pid was reused after the server ended).
    Gone,
    /// The pid is held by a process whose start time could not be read (`EMFILE`, `EACCES`, an
    /// unreadable line): it may be the server. Its pidfd is kept to be waited on, never used to
    /// signal.
    Unverified(Watched),
}

/// Pins the sandbox's PID 1 as the runner reported it: its pid and start time are its
/// identity, whatever its parent is now (a killed reaper leaves it reparented). A process that
/// has exited but is not reaped pins fine (its pidfd reads as ended).
///
/// # Errors
///
/// A failure of the pidfd that does not say the process is gone.
pub(crate) fn open_inner(pid: i32, start: u64) -> io::Result<Inner> {
    open_inner_with(pid, start, |pid| {
        super::rundir::stat_of(pid).map(|(_, start)| start)
    })
}

/// [`open_inner`] with the start-time read given (the tests inject its failures).
pub(crate) fn open_inner_with(
    pid: i32,
    start: u64,
    start_of: impl Fn(i32) -> io::Result<u64>,
) -> io::Result<Inner> {
    let watched = match Watched::open(pid) {
        Ok(watched) => watched,
        Err(error) if is_gone(&error) => return Ok(Inner::Gone),
        Err(error) => return Err(error),
    };
    // Read after the pidfd is open: a pid reused since then is the other process's, not the
    // one pinned (which, if it ended, reads as another start time or none).
    match start_of(pid) {
        Ok(found) if found == start => Ok(Inner::Ours(watched)),
        Ok(_) => Ok(Inner::Gone),
        Err(error) if is_gone(&error) => Ok(Inner::Gone),
        Err(_) => Ok(Inner::Unverified(watched)),
    }
}

/// Whether `open_reaper` failed because the reaper had already exited.
pub(crate) fn is_gone(error: &io::Error) -> bool {
    [rustix::io::Errno::SRCH, rustix::io::Errno::NOENT]
        .iter()
        .any(|gone| error.raw_os_error() == Some(gone.raw_os_error()))
        || (error.kind() == io::ErrorKind::PermissionDenied && error.to_string() == GONE)
}

/// The peer check of level 1 (see the module comment).
pub(crate) fn check_peer(stream: &impl std::os::fd::AsFd, sandbox: &Sandbox) -> io::Result<()> {
    let peer = rustix::net::sockopt::socket_peercred(stream)?;
    if peer.uid != rustix::process::getuid() {
        return Err(refused("the socket's peer is another user"));
    }
    let pid = peer.pid.as_raw_nonzero().get();
    if !sandbox.reaper.alive() {
        return Err(refused("the sandbox has gone"));
    }
    let status = status(pid)?;
    if pid != sandbox.reaper.pid && status.ppid != sandbox.reaper.pid {
        return Err(refused(
            "the socket's peer is not in this generation's sandbox",
        ));
    }
    match rustix::fs::stat(format!("/proc/{pid}/ns/pid")) {
        _ if sandbox.pid_namespace == NAMESPACE_UNREPORTED => {}
        Ok(stat) if stat.st_ino == sandbox.pid_namespace => {}
        Ok(_) => return Err(refused("the socket's peer is in another PID namespace")),
        // A non-dumpable peer denies it; NSpid and PPid decide.
        Err(rustix::io::Errno::ACCESS | rustix::io::Errno::PERM) => {}
        Err(error) => return Err(error.into()),
    }
    let inner = match status.nspid.as_slice() {
        [host, inner] if *host == pid => *inner,
        _ => return Err(refused("the socket's peer is not one namespace deep")),
    };
    if !(1..=INNER_PID_MAX).contains(&inner) {
        return Err(refused("the socket's peer has an unexpected inner PID"));
    }
    if !sandbox.reaper.alive() {
        return Err(refused("the sandbox has gone"));
    }
    Ok(())
}

/// Connects to the socket in `directory` through the directory's descriptor, once the entry
/// is a socket (not a link or anything else; the peer check decides whose it is).
pub(crate) async fn connect_in(directory: &OwnedFd) -> io::Result<tokio::net::UnixStream> {
    let entry = rustix::fs::statat(directory, SOCKET, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)?;
    if rustix::fs::FileType::from_raw_mode(entry.st_mode) != rustix::fs::FileType::Socket {
        return Err(refused("the socket entry is not a socket"));
    }
    tokio::net::UnixStream::connect(format!("/proc/self/fd/{}/{SOCKET}", directory.as_raw_fd()))
        .await
}

/// Runs a `/proc` scan off the async runtime.
async fn blocking<T: Send + 'static>(
    scan: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    tokio::task::spawn_blocking(scan)
        .await
        .map_err(|_| io::Error::other("the endpoint check did not finish"))?
}

impl Connect for SandboxSocket {
    fn connect(&self) -> Connecting {
        let directory = Arc::clone(&self.directory);
        let sandbox = Arc::clone(&self.sandbox);
        Box::pin(async move {
            let stream = connect_in(&directory).await?;
            let fd = std::os::fd::AsFd::as_fd(&stream).try_clone_to_owned()?;
            blocking(move || check_peer(&fd, &sandbox)).await?;
            Ok(Stream::Unix(stream))
        })
    }

    fn authority(&self) -> String {
        UNIX_AUTHORITY.to_owned()
    }
}

/// Level 3's connector: `127.0.0.1:<port>`, checked by the port's listeners.
#[derive(Debug)]
pub(crate) struct Loopback {
    pub port: u16,
    pub child: Arc<Watched>,
}

/// Why the listener check failed.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Listeners {
    /// Nothing listens on the port yet.
    NotReady,
    /// A listener on the port is not the child's.
    Foreign,
}

fn covers(address: &SocketAddr, port: u16) -> bool {
    address.port() == port
        && match address.ip() {
            IpAddr::V4(ip) => ip == Ipv4Addr::LOCALHOST || ip.is_unspecified(),
            IpAddr::V6(ip) => {
                ip.is_unspecified()
                    || ip == Ipv6Addr::LOCALHOST
                    || ip.to_ipv4_mapped() == Some(Ipv4Addr::LOCALHOST)
            }
        }
}

/// Every listening entry that could take a connection to `127.0.0.1:<port>` belongs to
/// `pid`.
pub(crate) fn listeners(port: u16, pid: i32) -> io::Result<Result<(), Listeners>> {
    let map = proc_error;
    let mut inodes: Vec<u64> = Vec::new();
    for entry in procfs::net::tcp()
        .map_err(map)?
        .into_iter()
        .chain(procfs::net::tcp6().map_err(map)?)
    {
        if entry.state == procfs::net::TcpState::Listen && covers(&entry.local_address, port) {
            inodes.push(entry.inode);
        }
    }
    if inodes.is_empty() {
        return Ok(Err(Listeners::NotReady));
    }
    let process = procfs::process::Process::new(pid).map_err(map)?;
    let owned = socket_inodes(process.fd().map_err(map)?.map(|fd| {
        fd.map(|fd| match fd.target {
            procfs::process::FDTarget::Socket(inode) => Some(inode),
            _ => None,
        })
    }))?;
    if inodes.iter().all(|inode| owned.contains(inode)) {
        Ok(Ok(()))
    } else {
        Ok(Err(Listeners::Foreign))
    }
}

/// A `/proc` error as the `io::Error` underneath (its errno kept, so a transient one such as
/// `EMFILE` stays recognisable).
pub(crate) fn proc_error(error: procfs::ProcError) -> io::Error {
    match error {
        procfs::ProcError::Io(error, _) => error,
        procfs::ProcError::PermissionDenied(_) => io::Error::from_raw_os_error(libc::EACCES),
        procfs::ProcError::NotFound(_) => io::Error::from_raw_os_error(libc::ENOENT),
        procfs::ProcError::Incomplete(_) => io::Error::from_raw_os_error(libc::EAGAIN),
        other => io::Error::other(other.to_string()),
    }
}

/// The socket inodes among a process's descriptors (`None` for the others). A descriptor
/// closed while it is listed is skipped; any other error fails the whole scan.
pub(crate) fn socket_inodes(
    descriptors: impl Iterator<Item = procfs::ProcResult<Option<u64>>>,
) -> io::Result<BTreeSet<u64>> {
    let mut inodes = BTreeSet::new();
    for descriptor in descriptors {
        match descriptor {
            Ok(Some(inode)) => {
                inodes.insert(inode);
            }
            Ok(None) | Err(procfs::ProcError::NotFound(_)) => {}
            Err(error) => return Err(proc_error(error)),
        }
    }
    Ok(inodes)
}

fn listening(port: u16, child: &Watched) -> io::Result<()> {
    if !child.alive() {
        return Err(refused("the model server has gone"));
    }
    match listeners(port, child.pid)? {
        Ok(()) => Ok(()),
        Err(Listeners::NotReady) => Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "the model server is not listening yet",
        )),
        Err(Listeners::Foreign) => Err(refused(
            "another process listens on the model server's port",
        )),
    }
}

impl Connect for Loopback {
    fn connect(&self) -> Connecting {
        let port = self.port;
        let child = Arc::clone(&self.child);
        Box::pin(async move {
            let before = Arc::clone(&child);
            blocking(move || listening(port, &before)).await?;
            let stream = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await?;
            blocking(move || listening(port, &child)).await?;
            Ok(Stream::Tcp(stream))
        })
    }

    fn authority(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }
}

#[cfg(test)]
#[path = "endpoint_tests.rs"]
mod tests;
