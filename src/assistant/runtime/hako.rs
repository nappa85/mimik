//! Level 1 through the Hakoniwa library instead of bubblewrap (comparison spike; ASSIST-48,
//! DEP-24). Same sandbox properties, built in the runner process: new user, mount, PID, IPC,
//! UTS, cgroup and network namespaces (no network at all: the namespace stays empty), a root
//! of read-only binds and bounded tmpfs mounts, the same allowlist as `bwrap.rs`, our own
//! seccomp program and the same marker.
//!
//! What differs from bubblewrap, and why it is safe:
//!
//! - Hakoniwa binds host *paths* and closes every descriptor above 2 in its child, so the
//!   verified descriptors cannot be bound. Each bind carries the `(device, inode)` the
//!   launcher verified; the program's closure compares it with what is mounted, after the
//!   mounts and just before the exec, and refuses to go on when any differs. The descriptors
//!   themselves never enter the sandbox.
//! - The program is PID 1 of its namespace and its reaper is the runner's child outside it
//!   (the process the endpoint check pins). The sandbox has no further process: the seccomp
//!   program already refuses `fork`.
//! - The token is the content of a file the runner has Hakoniwa write into the sandbox's own
//!   `/run` tmpfs (memory): never on the host's disk, a command line or an environment.
//! - The shim is gone. The marker pipe's write end is the program's standard input until the
//!   closure writes the marker on it, replaces descriptor 0 with `/dev/null` and closes every
//!   other descriptor, just before the exec: nothing the program does can write the marker.
//!   Failures before the marker (a mount, the identity check, capabilities, seccomp) are
//!   sandbox failures; failures after it (the exec itself) are the server's.
use super::bwrap::{self, GpuBinds, Item};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// `device` and `inode` of a verified object.
pub(crate) type Identity = (u64, u64);

/// A secret that never prints: `Debug` shows nothing of it, `Serialize` is transparent (the
/// runner's spec is a memory file, never a disk file).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct Secret(String);

impl Secret {
    pub(crate) fn new(text: String) -> Self {
        Self(text)
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

/// How a bind is mounted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Access {
    /// Read-only, `nosuid`, `nodev`.
    ReadOnly,
    /// Read-write, `nosuid`, `nodev` (the generation's socket directory).
    ReadWrite,
    /// A device node, read-write, `nosuid` only.
    Device,
}

/// One bind mount.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Bind {
    pub source: String,
    pub target: String,
    pub access: Access,
    /// What the mounted object must be (device and inode); `None` for system paths, which
    /// bubblewrap binds by path too.
    pub identity: Option<Identity>,
}

/// A bounded tmpfs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Tmpfs {
    pub target: String,
    pub bytes: u64,
    pub mode: u32,
}

/// The whole sandbox: what the runner builds the container from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Plan {
    /// The root directory the sandbox is built in (Mimik's, a sibling of the generation's
    /// directory in `run/`, so its removal is the generation's).
    pub root: String,
    /// The token file's content, written at [`bwrap::TOKEN`] into the sandbox's own `/run`
    /// tmpfs (memory) by the runner: it is never in a file on the host, a command line or an
    /// environment variable.
    pub token: Secret,
    pub binds: Vec<Bind>,
    pub tmpfs: Vec<Tmpfs>,
    /// `(target, link)` pairs.
    pub symlinks: Vec<(String, String)>,
    pub dirs: Vec<String>,
    pub workdir: String,
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// A runtime file to bind: its name under [`bwrap::RUNTIME`], the host path it was verified
/// at and its identity.
#[derive(Clone, Debug)]
pub(crate) struct File {
    pub name: String,
    pub source: PathBuf,
    pub identity: Identity,
}

/// What the launcher gives the plan.
#[derive(Debug)]
pub(crate) struct Inputs<'a> {
    pub root: PathBuf,
    pub runtime: Vec<File>,
    pub model: (PathBuf, Identity),
    pub generation: (PathBuf, Identity),
    pub token: Secret,
    /// Further read-only files `(source, target, identity)` (the probe's own executable).
    pub extra: Vec<(PathBuf, String, Identity)>,
    pub gpu: Option<&'a GpuBinds>,
    pub env: Vec<(String, String)>,
    pub argv: Vec<String>,
}

/// The device nodes of the minimal `/dev`: no terminal, no `pts`, no `ptmx`.
pub(crate) const DEVICES: &[&str] = &["null", "zero", "full", "random", "urandom"];

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn ro(source: &Path, target: &str, identity: Option<Identity>) -> Bind {
    Bind {
        source: text(source),
        target: target.to_owned(),
        access: Access::ReadOnly,
        identity,
    }
}

/// Parents of `path` that must exist as directories (every ancestor but `/`).
fn parents(path: &str) -> Vec<String> {
    let mut found: Vec<String> = Path::new(path)
        .ancestors()
        .skip(1)
        .map(text)
        .filter(|ancestor| ancestor != "/" && !ancestor.is_empty())
        .collect();
    found.reverse();
    found
}

/// The plan for `inputs`: the same allowlist as bubblewrap's profile.
pub(crate) fn plan(inputs: &Inputs<'_>) -> Plan {
    let mut binds = Vec::new();
    let mut symlinks: Vec<(String, String)> = Vec::new();
    let mut dirs: Vec<String> = Vec::new();
    let mut items = bwrap::system_items();
    if let Some(gpu) = inputs.gpu {
        items.extend(gpu.items());
    }
    for item in items {
        match item {
            Item::RoBind(path) => binds.push(ro(&path, &text(&path), None)),
            Item::Symlink { target, at } => symlinks.push((text(&target), text(&at))),
            Item::Dir(path) => dirs.push(text(&path)),
        }
    }
    if let Some(gpu) = inputs.gpu {
        binds.push(Bind {
            source: text(&gpu.dev),
            target: gpu.inside(),
            access: Access::Device,
            identity: None,
        });
    }
    for device in DEVICES {
        let path = format!("/dev/{device}");
        binds.push(Bind {
            source: path.clone(),
            target: path,
            access: Access::Device,
            identity: None,
        });
    }
    for file in &inputs.runtime {
        binds.push(ro(
            &file.source,
            &format!("{}/{}", bwrap::RUNTIME, file.name),
            Some(file.identity),
        ));
    }
    binds.push(ro(&inputs.model.0, bwrap::MODEL, Some(inputs.model.1)));
    for (source, target, identity) in &inputs.extra {
        binds.push(ro(source, target, Some(*identity)));
    }
    binds.push(Bind {
        source: text(&inputs.generation.0),
        target: bwrap::SOCKET_DIR.to_owned(),
        access: Access::ReadWrite,
        identity: Some(inputs.generation.1),
    });
    for (original, link) in [
        ("/proc/self/fd", "/dev/fd"),
        ("/proc/self/fd/0", "/dev/stdin"),
        ("/proc/self/fd/1", "/dev/stdout"),
        ("/proc/self/fd/2", "/dev/stderr"),
    ] {
        symlinks.push((original.to_owned(), link.to_owned()));
    }
    for (_, link) in &symlinks {
        dirs.extend(parents(link));
    }
    dirs.sort();
    dirs.dedup();
    Plan {
        root: text(&inputs.root),
        token: inputs.token.clone(),
        binds,
        tmpfs: vec![
            // The token's file lives here, in memory; the socket directory is bound under it.
            Tmpfs {
                target: "/run".to_owned(),
                bytes: 64 << 10,
                mode: 0o755,
            },
            Tmpfs {
                target: "/dev".to_owned(),
                bytes: 1 << 20,
                mode: 0o755,
            },
            Tmpfs {
                target: "/tmp".to_owned(),
                bytes: bwrap::TMP_BYTES,
                mode: 0o1777,
            },
            Tmpfs {
                target: "/dev/shm".to_owned(),
                bytes: bwrap::SHM_BYTES,
                mode: 0o1777,
            },
        ],
        symlinks,
        dirs,
        workdir: bwrap::RUNTIME.to_owned(),
        argv: inputs.argv.clone(),
        env: inputs.env.clone(),
    }
}

/// The AppArmor profile template (`apparmor/mimik`, also for distribution packages): the
/// placeholder is the executable's path.
const APPARMOR_TEMPLATE: &str = include_str!("../../../apparmor/mimik");
const APPARMOR_PLACEHOLDER: &str = "@MIMIK_EXE@";

/// The profile for the binary at `exe` (an absolute path): the template with that one path
/// in it. A path AppArmor would read as more than a file name (globs, braces, quotes,
/// control characters) is refused rather than escaped.
///
/// # Errors
///
/// The path is relative or has a character a profile attachment would interpret.
pub(crate) fn apparmor_profile(exe: &Path) -> Result<String, String> {
    let name = exe
        .to_str()
        .ok_or_else(|| "the executable's path is not text".to_owned())?;
    if !exe.is_absolute() {
        return Err("the executable's path is not absolute".into());
    }
    if let Some(bad) = name
        .chars()
        .find(|c| c.is_control() || "\"\\{}[]*?^,@".contains(*c))
    {
        return Err(format!(
            "the executable's path has a character AppArmor would interpret ({bad:?})"
        ));
    }
    Ok(APPARMOR_TEMPLATE.replace(APPARMOR_PLACEHOLDER, name))
}

/// `text` as one shell word.
fn shell_word(text: &str) -> String {
    if !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+=:,@%".contains(c))
    {
        text.to_owned()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

/// The assistant page's text when the sandbox cannot start and Ubuntu's `AppArmor`
/// restriction of unprivileged user namespaces is on: the profile for this very binary,
/// generated by it, and the steps to install it (the page's draft; untested on Ubuntu 24.04).
pub(crate) fn apparmor_message(exe: &Path) -> String {
    let command = shell_word(&exe.to_string_lossy());
    format!(
        "This system's AppArmor does not let Mimik create the private sandbox of the model server. An administrator can allow it once for this program: run `{command} assistant-apparmor-profile | sudo tee /etc/apparmor.d/mimik`, then `sudo apparmor_parser -r /etc/apparmor.d/mimik`, then press Restart. The profile only gives this one program that permission; it does not confine or unconfine anything else."
    )
}

/// What the Hakoniwa sandbox needs of the host before its probe: unprivileged user
/// namespaces (the one setting that rules them out for certain; the probe decides otherwise).
/// There is no helper program to find.
pub(crate) fn preflight() -> Result<bwrap::Bwrap, bwrap::Unusable> {
    bwrap::namespaces(Path::new("/proc"))?;
    Ok(bwrap::Bwrap {
        path: PathBuf::new(),
        size: true,
    })
}

#[cfg(any(test, feature = "sandbox-hakoniwa"))]
/// What the runner reports first on its status descriptor, the shape of bubblewrap's line:
/// the reaper outside the PID namespace. The namespace number is not reported (the reaper is
/// not dumpable, so `/proc/<pid>/ns/pid_for_children` is unreadable): the endpoint check
/// then leaves that comparison to the parent and `NSpid` checks.
pub(crate) fn started_line(reaper: i32) -> String {
    format!("{{\"child-pid\":{reaper}}}\n")
}

/// Reads the runner's start report: only `child-pid`.
pub(crate) fn started(line: &str) -> Option<bwrap::Started> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    Some(bwrap::Started {
        child_pid: i32::try_from(value.get("child-pid")?.as_i64()?).ok()?,
        pid_namespace: super::endpoint::NAMESPACE_UNREPORTED,
    })
}

#[cfg(any(test, feature = "sandbox-hakoniwa"))]
/// The exit codes of the closure when it does not reach its exec. All but [`exit::EXEC`]
/// come before the marker, so the launcher counts them as sandbox failures.
pub(crate) mod exit {
    pub const SESSION: i32 = 110;
    pub const IDENTITY: i32 = 111;
    pub const CAPABILITIES: i32 = 112;
    pub const SECCOMP: i32 = 113;
    pub const MARKER: i32 = 114;
    pub const DESCRIPTORS: i32 = 115;
    pub const SPEC: i32 = 116;
    /// After the marker: the exec failed (counted as the server's failure).
    pub const EXEC: i32 = 127;
}

#[cfg(any(test, feature = "sandbox-hakoniwa"))]
/// The first bind whose mounted object is not the verified one: `stat` gives the mounted
/// target's identity (`None` when it cannot be read, which fails closed).
pub(crate) fn identity_mismatch(
    binds: &[Bind],
    stat: impl Fn(&str) -> Option<Identity>,
) -> Option<&Bind> {
    binds.iter().find(|bind| {
        bind.identity
            .is_some_and(|want| stat(&bind.target) != Some(want))
    })
}

#[cfg(feature = "sandbox-hakoniwa")]
pub(crate) use run::run;
#[cfg(all(test, feature = "sandbox-hakoniwa"))]
pub(crate) use run::{setup_failed, single_threaded};

#[cfg(feature = "sandbox-hakoniwa")]
mod run {
    use super::{Access, Bind, Plan, bwrap, exit, identity_mismatch, started_line};
    use crate::assistant::runtime::{probe, seccomp};
    use hakoniwa::{Container, MountOptions, Namespace, Stdio};
    use std::io::Write;
    use std::os::fd::{FromRawFd, OwnedFd, RawFd};
    use std::os::unix::process::CommandExt;
    use std::sync::Arc;

    const MARKER_LINE: &[u8] = b"exec\n";
    /// How long the runner waits, after the sandbox reports its setup, for an early end
    /// (a failed setup ends at once; a program that runs is still there).
    const EARLY_END: std::time::Duration = std::time::Duration::from_millis(50);

    /// The mount options of a bind: the plan's, plus every restriction the source's own mount
    /// carries. The kernel refuses an unprivileged remount that drops a locked flag (`nosuid`,
    /// `nodev`, `noexec`), and the library's fallback for that refusal drops read-only
    /// instead, so the flags are given up front and no fallback is used: a mount that cannot be
    /// made as planned fails the start.
    fn options(bind: &Bind) -> MountOptions {
        let base = MountOptions::BIND | MountOptions::NOSUID;
        let mut options = match bind.access {
            Access::ReadOnly => {
                base | MountOptions::REC | MountOptions::NODEV | MountOptions::RDONLY
            }
            Access::ReadWrite => base | MountOptions::REC | MountOptions::NODEV,
            Access::Device => base,
        };
        if rustix::fs::statvfs(&bind.source)
            .is_ok_and(|stat| stat.f_flag.contains(rustix::fs::StatVfsMountFlags::NOEXEC))
        {
            options |= MountOptions::NOEXEC;
        }
        options
    }

    fn container(plan: &Plan) -> Container {
        let mut container = Container::empty();
        container.rootdir(&plan.root);
        for namespace in [
            Namespace::Mount,
            Namespace::User,
            Namespace::Pid,
            Namespace::Ipc,
            Namespace::Uts,
            Namespace::Cgroup,
            // Left empty: no interface is configured, so the sandbox has no network at all.
            Namespace::Network,
        ] {
            container.unshare(namespace);
        }
        container
            .uidmap(rustix::process::getuid().as_raw())
            .gidmap(rustix::process::getgid().as_raw())
            .procfsmount("/proc");
        for bind in &plan.binds {
            container.mount(&bind.source, &bind.target, "", options(bind), None);
        }
        for tmpfs in &plan.tmpfs {
            container.mount(
                "tmpfs",
                &tmpfs.target,
                "tmpfs",
                MountOptions::NOSUID | MountOptions::NODEV,
                Some(format!("size={},mode={:o}", tmpfs.bytes, tmpfs.mode)),
            );
        }
        for dir in &plan.dirs {
            container.dir(dir, 0o755);
        }
        container.file(bwrap::TOKEN, &plan.token.0);
        for (original, link) in &plan.symlinks {
            container.symlink(original, link);
        }
        container
    }

    fn mounted(target: &str) -> Option<super::Identity> {
        rustix::fs::stat(target)
            .ok()
            .map(|stat| (stat.st_dev, stat.st_ino))
    }

    fn refuse(code: i32, why: &str) -> i32 {
        eprintln!("mimik sandbox: {why}");
        code
    }

    /// Inside the sandbox, as the program's process just before it becomes the server: a new
    /// session, the verified objects checked, every capability dropped, seccomp installed,
    /// the marker written, descriptor 0 replaced and the rest closed, then the exec.
    fn inside(plan: &Plan) -> i32 {
        if rustix::process::setsid().is_err() {
            return refuse(exit::SESSION, "no new session");
        }
        if let Some(bind) = identity_mismatch(&plan.binds, mounted) {
            return refuse(
                exit::IDENTITY,
                &format!("{} is not the verified object", bind.target),
            );
        }
        for set in [
            caps::CapSet::Ambient,
            caps::CapSet::Bounding,
            caps::CapSet::Inheritable,
            caps::CapSet::Permitted,
            caps::CapSet::Effective,
        ] {
            if caps::clear(None, set).is_err() {
                return refuse(exit::CAPABILITIES, "capabilities cannot be dropped");
            }
        }
        let program = seccomp::program(seccomp::Profile::Sandbox);
        if seccomp::install(&program).is_err() {
            return refuse(exit::SECCOMP, "the seccomp program cannot be installed");
        }
        if plan.argv.is_empty() {
            return refuse(exit::SPEC, "nothing to execute");
        }
        // SAFETY: descriptor 0 is the marker pipe's write end, handed over by the runner as
        // the program's standard input and used only here, by number.
        let marker = unsafe { std::os::fd::BorrowedFd::borrow_raw(0) };
        if rustix::io::write(marker, MARKER_LINE).ok() != Some(MARKER_LINE.len()) {
            return refuse(exit::MARKER, "the marker cannot be written");
        }
        let replaced = std::fs::File::open("/dev/null")
            .is_ok_and(|null| rustix::stdio::dup2_stdin(&null).is_ok());
        if !replaced || !probe::close_inherited(probe::close_range, probe::fd_listing) {
            return refuse(
                exit::DESCRIPTORS,
                "the inherited descriptors cannot be closed",
            );
        }
        let error = std::process::Command::new(&plan.argv[0])
            .args(&plan.argv[1..])
            .env_clear()
            .envs(plan.env.iter().map(|(name, value)| (name, value)))
            .exec();
        refuse(
            exit::EXEC,
            &format!("cannot execute {}: {error}", plan.argv[0]),
        )
    }

    /// Whether this process has no other thread: Hakoniwa forks and then allocates, which is
    /// only sound without other threads holding locks.
    pub(crate) fn single_threaded() -> bool {
        std::fs::read_dir("/proc/self/task").is_ok_and(|tasks| tasks.count() == 1)
    }

    /// The runner's level-1 body: builds the container, starts the program, reports the
    /// reaper, waits and returns the program's exit code. `status` is the status
    /// descriptor and `marker` the marker pipe's write end, both handed over by the launcher.
    pub(crate) fn run(plan: Plan, status: RawFd, marker: RawFd) -> i32 {
        // The test binary plays the runner (as a libtest helper, which has a second thread
        // beside a main thread that only waits); the shipped runner is `main` and alone.
        // Files the sandbox makes (the token's) are private to Mimik's user.
        rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077));
        if !cfg!(test) && !single_threaded() {
            return refuse(exit::SPEC, "the runner is not single-threaded");
        }
        // SAFETY: both descriptors are handed over by the launcher and owned by this
        // process from here on; nothing else uses these numbers.
        let (status, marker) =
            unsafe { (OwnedFd::from_raw_fd(status), OwnedFd::from_raw_fd(marker)) };
        let mut status_file = std::fs::File::from(status);
        let workdir = plan.workdir.clone();
        let container = container(&plan);
        let plan = Arc::new(plan);
        let inner = Arc::clone(&plan);
        // SAFETY: the closure runs after fork in a process with no other thread (checked
        // above), so allocating and locking in it is sound.
        let mut command = unsafe { container.command_from_closure(move || inside(&inner)) };
        command.current_dir(&workdir).stdin(Stdio::from(marker));
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => return refuse(exit::SPEC, &format!("hakoniwa: {error}")),
        };
        // A failed setup ends the reaper at once; one that runs is still there after a
        // moment. Only a sandbox that is set up is reported (as bubblewrap does).
        let pidfd = i32::try_from(child.id())
            .ok()
            .and_then(rustix::process::Pid::from_raw)
            .and_then(|pid| {
                rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty()).ok()
            });
        let early = pidfd.as_ref().is_some_and(|pidfd| {
            let mut fds = [rustix::event::PollFd::new(
                pidfd,
                rustix::event::PollFlags::IN,
            )];
            let wait = rustix::time::Timespec {
                tv_sec: 0,
                tv_nsec: i64::try_from(EARLY_END.as_nanos()).unwrap_or(50_000_000),
            };
            rustix::event::poll(&mut fds, Some(&wait)).is_ok_and(|ready| ready > 0)
        });
        let reaper = i32::try_from(child.id()).unwrap_or(0);
        let report = |status: &mut std::fs::File| status.write_all(started_line(reaper).as_bytes());
        if early {
            return match child.wait() {
                // The sandbox could not be set up: nothing is reported (as bubblewrap
                // reports nothing when it fails first), and the marker never came.
                Ok(status) if setup_failed(&status) => said(&status),
                // The program ran and ended within moments (a device listing, a probe that
                // reports and exits): the sandbox was up, so it is reported.
                Ok(status) => {
                    if report(&mut status_file).is_err() {
                        return refuse(exit::SPEC, "the status descriptor cannot be written");
                    }
                    status.code
                }
                Err(error) => refuse(exit::SPEC, &format!("hakoniwa: {error}")),
            };
        }
        if report(&mut status_file).is_err() {
            return refuse(exit::SPEC, "the status descriptor cannot be written");
        }
        finish(&mut child)
    }

    /// Whether Hakoniwa itself failed (code 125 without an exit status of the program: a
    /// program's own exit is always an exit status, a signal is above 128).
    pub(crate) fn setup_failed(status: &hakoniwa::ExitStatus) -> bool {
        status.exit_code.is_none() && status.code == 125
    }

    /// Says a setup failure on stderr and returns its code.
    fn said(status: &hakoniwa::ExitStatus) -> i32 {
        eprintln!("mimik sandbox: {}", status.reason);
        status.code
    }

    /// Waits for the program and returns its exit code; a setup failure Hakoniwa reports
    /// (code 125 without an exit status) is said on stderr.
    fn finish(child: &mut hakoniwa::Child) -> i32 {
        match child.wait() {
            Ok(status) if setup_failed(&status) => said(&status),
            Ok(status) => status.code,
            Err(error) => refuse(exit::SPEC, &format!("hakoniwa: {error}")),
        }
    }
}

#[cfg(test)]
#[path = "hako_tests.rs"]
mod tests;
