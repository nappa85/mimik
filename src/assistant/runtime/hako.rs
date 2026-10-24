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

impl Drop for Secret {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.0);
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
    /// Mounted read-only (an empty mask over a mount under a read-only bind).
    pub read_only: bool,
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
    /// The host's `/proc/self/mountinfo`: what is mounted under a bound directory.
    pub mountinfo: String,
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

/// What the Landlock layer inside the namespaces did, as the closure reports it on the marker
/// pipe (the only channel that survives Hakoniwa closing every descriptor above 2, and one the
/// server cannot write to: it never holds the descriptor). Best effort (DEP-25): a kernel
/// without Landlock, or with an older ABI, leaves the namespace sandbox alone in force, as
/// with bubblewrap; the page says which.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Landlock {
    /// Everything asked for is enforced (filesystem, TCP bind and connect, scoping); `abi` is
    /// the kernel's Landlock ABI.
    Full { abi: u8 },
    /// Landlock is there but the kernel's ABI lacks part of what was asked for.
    Partial { abi: u8 },
    /// No Landlock in this kernel (or it is not enabled): nothing was enforced.
    Unavailable,
}

impl Landlock {
    /// The line the closure appends to the marker.
    pub(crate) fn line(self) -> String {
        match self {
            Self::Full { abi } => format!("{{\"landlock\":\"full\",\"abi\":{abi}}}\n"),
            Self::Partial { abi } => format!("{{\"landlock\":\"partial\",\"abi\":{abi}}}\n"),
            Self::Unavailable => "{\"landlock\":\"none\"}\n".to_owned(),
        }
    }

    /// The report in what followed the marker: exactly one JSON line with the known keys and
    /// nothing after its newline (`None`: absent, not understood, or anything more or less).
    pub(crate) fn parse(after_marker: &[u8]) -> Option<Self> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Line {
            landlock: String,
            abi: Option<u8>,
        }
        let line: Line = serde_json::from_slice(after_marker.strip_suffix(b"\n")?).ok()?;
        // ABI 0 is "no Landlock", which the `none` report says without a number.
        let parsed = match (line.landlock.as_str(), line.abi) {
            ("full", Some(abi @ 1..)) => Self::Full { abi },
            ("partial", Some(abi @ 1..)) => Self::Partial { abi },
            ("none", None) => Self::Unavailable,
            _ => return None,
        };
        // Only the exact wire form the closure writes (no other whitespace, key order or
        // `null`): what it would write for this value is compared byte for byte.
        (parsed.line().as_bytes() == after_marker).then_some(parsed)
    }

    /// The assistant page's words.
    pub(crate) fn describe(self) -> String {
        match self {
            Self::Full { abi } => format!("full for the ABI 6 rules (kernel ABI {abi})"),
            Self::Partial { abi } => format!("partial (kernel ABI {abi})"),
            Self::Unavailable => "not available".to_owned(),
        }
    }
}

/// What a path of the Landlock ruleset may be used for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(any(test, feature = "sandbox-hakoniwa"))]
pub(crate) enum Grant {
    /// Read, list and execute: the program the closure execs and the system library
    /// directories (which hold the ELF interpreter).
    ReadExec,
    /// Read one file, never execute it: the model.
    ReadFile,
    /// Read and list, never execute: `/proc`, the runtime's libraries and the GPU's system
    /// data.
    Read,
    /// List only: `/dev` itself (its nodes have their own rules).
    List,
    /// Read, write and create (regular files, directories, sockets, fifos, links) and
    /// remove, never execute: the socket directory, `/tmp`, `/dev/shm`.
    ReadWrite,
    /// A device node: read, write and ioctl.
    Device,
}

/// One path of the Landlock ruleset.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg(any(test, feature = "sandbox-hakoniwa"))]
pub(crate) struct Rule {
    pub path: String,
    pub grant: Grant,
}

/// The tmpfs mounts the server writes in (the plan's own): the sandbox's `/tmp` and
/// `/dev/shm`. `/run` holds the token file and the socket directory's bind, each with a rule
/// of its own.
#[cfg(any(test, feature = "sandbox-hakoniwa"))]
const WRITABLE_TMPFS: [&str; 2] = ["/tmp", "/dev/shm"];

/// The Landlock ruleset of `plan`: what the namespace mounts give the server, narrowed to
/// the paths and uses it needs. Execute only on the program the closure execs (its first
/// argument) and the system library directories (the kernel opens the ELF interpreter with the
/// execute right; the loader maps libraries with `mmap`, which that right does not govern, so
/// the runtime's libraries need read only); read and list on every other read-only bind, the
/// model read only; read and write on the read-write binds and the two writable tmpfs mounts
/// `/tmp` and `/dev/shm`; of `/run` only the token file (read) and the socket directory (its
/// bind); listing on `/dev` and `/dev/dri` (libdrm and Mesa enumerate it), the runtime's
/// directory and the GPU path's `/sys` scaffolding, `/proc` read; each device node as a device.
/// Everything else the mounts make visible or writable (the other `/dev` entries, `/etc` and
/// `/sys` outside their binds, a writable mount not listed) is denied.
#[cfg(any(test, feature = "sandbox-hakoniwa"))]
pub(crate) fn landlock_rules(plan: &Plan) -> Vec<Rule> {
    let mut rules: Vec<Rule> = Vec::new();
    let mut add = |path: &str, grant: Grant| {
        let rule = Rule {
            path: path.to_owned(),
            grant,
        };
        if !rules.contains(&rule) {
            rules.push(rule);
        }
    };
    add("/proc", Grant::Read);
    add("/dev", Grant::List);
    // The token is read, never listed or written (the rest of `/run` has no rule).
    add(bwrap::TOKEN, Grant::ReadFile);
    // The runtime's directory holds only its bound files, but ggml lists it to find its
    // backends.
    add(bwrap::RUNTIME, Grant::List);
    // The scaffolding directories of the GPU path's `/sys` links (not the top-level ones).
    for dir in &plan.dirs {
        if Path::new(dir).components().count() > 2 {
            add(dir, Grant::List);
        }
    }
    for path in WRITABLE_TMPFS {
        add(path, Grant::ReadWrite);
    }
    for bind in &plan.binds {
        let program = plan.argv.first().is_some_and(|argv0| *argv0 == bind.target);
        let grant = match bind.access {
            Access::ReadOnly if bind.target == bwrap::MODEL => Grant::ReadFile,
            Access::ReadOnly if program || bwrap::LIBRARY_DIRS.contains(&bind.target.as_str()) => {
                Grant::ReadExec
            }
            Access::ReadOnly => Grant::Read,
            Access::ReadWrite => Grant::ReadWrite,
            Access::Device => {
                // The device's directory is listed by a rule of its own: libdrm and Mesa
                // enumerate `/dev/dri` (the rule on `/dev` also reaches it, but the table
                // does not lean on that).
                if let Some(parent) = Path::new(&bind.target).parent().and_then(Path::to_str)
                    && parent != "/"
                {
                    add(parent, Grant::List);
                }
                Grant::Device
            }
        };
        add(&bind.target, grant);
    }
    rules
}

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

/// The mount point of one `mountinfo` line, its octal escapes (`\\040` for a space) decoded.
fn mount_point(line: &str) -> Option<String> {
    let field = line.split(' ').nth(4)?;
    let mut out = Vec::new();
    let bytes = field.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'\\'
            && at + 3 < bytes.len()
            && bytes[at + 1..at + 4].iter().all(u8::is_ascii_digit)
        {
            let value =
                u32::from_str_radix(std::str::from_utf8(&bytes[at + 1..at + 4]).ok()?, 8).ok()?;
            out.push(u8::try_from(value).ok()?);
            at += 4;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The topmost mount points strictly under `dir` in `mountinfo`. A non-recursive bind of
/// `dir` shows the directory underneath such a mount (possibly stale files), so each is
/// masked.
pub(crate) fn nested_mounts(mountinfo: &str, dir: &str) -> Vec<String> {
    let prefix = format!("{}/", dir.trim_end_matches('/'));
    let mut found: Vec<String> = mountinfo
        .lines()
        .filter_map(mount_point)
        .filter(|point| point.starts_with(&prefix))
        .collect();
    found.sort();
    found.dedup();
    let mut topmost: Vec<String> = Vec::new();
    for point in found {
        if !topmost
            .iter()
            .any(|above| point.starts_with(&format!("{above}/")))
        {
            topmost.push(point);
        }
    }
    topmost
}

/// The sandbox's own tmpfs mounts: `/run` (the token's file and the socket directory's bind),
/// `/dev`, `/tmp` and `/dev/shm`, each bounded.
fn fixed_tmpfs() -> Vec<Tmpfs> {
    vec![
        // The token's file lives here, in memory; the socket directory is bound under it.
        Tmpfs {
            target: "/run".to_owned(),
            bytes: 64 << 10,
            mode: 0o755,
            read_only: false,
        },
        Tmpfs {
            target: "/dev".to_owned(),
            bytes: 1 << 20,
            mode: 0o755,
            read_only: false,
        },
        Tmpfs {
            target: "/tmp".to_owned(),
            bytes: bwrap::TMP_BYTES,
            mode: 0o1777,
            read_only: false,
        },
        Tmpfs {
            target: "/dev/shm".to_owned(),
            bytes: bwrap::SHM_BYTES,
            mode: 0o1777,
            read_only: false,
        },
    ]
}

/// A mount under a read-only directory bind is not part of the (non-recursive) bind: an
/// empty read-only tmpfs masks what the directory itself holds underneath it.
fn masks(binds: &[Bind], mountinfo: &str) -> Vec<Tmpfs> {
    binds
        .iter()
        .filter(|bind| bind.access == Access::ReadOnly && Path::new(&bind.source).is_dir())
        .flat_map(|bind| {
            nested_mounts(mountinfo, &bind.source)
                .into_iter()
                .map(|point| Tmpfs {
                    target: format!("{}{}", bind.target, &point[bind.source.len()..]),
                    bytes: 4096,
                    mode: 0o555,
                    read_only: true,
                })
                .collect::<Vec<_>>()
        })
        .collect()
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
    let masks = masks(&binds, &inputs.mountinfo);
    Plan {
        root: text(&inputs.root),
        token: inputs.token.clone(),
        binds,
        tmpfs: fixed_tmpfs().into_iter().chain(masks).collect(),
        symlinks,
        dirs,
        workdir: bwrap::RUNTIME.to_owned(),
        argv: inputs.argv.clone(),
        env: inputs.env.clone(),
    }
}

/// The `AppArmor` profile template (`apparmor/mimik`, also for distribution packages): the
/// placeholder is the executable's path.
const APPARMOR_TEMPLATE: &str = include_str!("../../../apparmor/mimik");
const APPARMOR_PLACEHOLDER: &str = "@MIMIK_EXE@";

/// The profile for the binary at `exe` (an absolute path): the template with that one path
/// in it. A path `AppArmor` would read as more than a file name (globs, braces, quotes,
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
    // `/proc/self/exe` of a binary replaced or removed while it runs ends in this: no profile
    // can attach to it.
    if name.ends_with(" (deleted)") {
        return Err("the executable has been replaced or removed since it started".into());
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

/// The profile `mimik assistant-apparmor-profile [--for <path>]` prints: for `--for`'s path (a
/// packager's or a test setup's, which needs no `sed`), otherwise for the running binary.
///
/// # Errors
///
/// An argument it does not know, or a path the profile cannot take.
pub(crate) fn apparmor_profile_for(args: &[String]) -> Result<String, String> {
    match args {
        [] => std::env::current_exe()
            .map_err(|_| "Mimik's own path is unknown".to_owned())
            .and_then(|exe| apparmor_profile(&exe)),
        [flag, path] if flag == "--for" => apparmor_profile(Path::new(path)),
        _ => Err("usage: assistant-apparmor-profile [--for <absolute path of the binary>]".into()),
    }
}

/// `text` as one shell word.
pub(crate) fn shell_word(text: &str) -> String {
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

/// The page's remedy for a failed Hakoniwa start, by the one setting that may explain it
/// (worded as a possibility: the probe's failure itself decides nothing).
pub(crate) fn remedy(cause: bwrap::Cause, exe: &Path) -> String {
    match cause {
        bwrap::Cause::AppArmor => apparmor_message(exe),
        bwrap::Cause::UsernsClone => "kernel.unprivileged_userns_clone = 0 may be why: it forbids the private sandbox of the model server. An administrator can allow it with `sudo sysctl kernel.unprivileged_userns_clone=1` (and keep it in a file under /etc/sysctl.d), then press Restart.".to_owned(),
    }
}

/// The assistant page's text for Ubuntu's `AppArmor` restriction of unprivileged user
/// namespaces: the profile for this very binary, generated by it, and the steps to install
/// it (the page's text, checked on Ubuntu 24.04). The profile is written to a file first,
/// so a failure of the generating command installs nothing.
pub(crate) fn apparmor_message(exe: &Path) -> String {
    let command = shell_word(&exe.to_string_lossy());
    format!(
        "Ubuntu's AppArmor restriction of unprivileged user namespaces may be why Mimik cannot create the private sandbox of the model server. An administrator can allow it once for this program: run `{command} assistant-apparmor-profile > mimik.apparmor && sudo install -m 0644 mimik.apparmor /etc/apparmor.d/mimik && sudo apparmor_parser -r /etc/apparmor.d/mimik`, then press Restart. The profile only gives this one program that permission; it does not confine or unconfine anything else."
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
pub(crate) fn started_line(reaper: i32, inner: Option<(i32, u64)>, ended: bool) -> String {
    use std::fmt::Write as _;
    let mut line = format!("{{\"child-pid\":{reaper}");
    if let Some((inner, start)) = inner {
        let _ = write!(line, ",\"inner-pid\":{inner},\"inner-start\":{start}");
    }
    if ended {
        line.push_str(",\"ended\":true");
    }
    line.push_str("}\n");
    line
}

/// Reads the runner's start report: only `child-pid`.
pub(crate) fn started(line: &str) -> Option<bwrap::Started> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    Some(bwrap::Started {
        child_pid: i32::try_from(value.get("child-pid")?.as_i64()?).ok()?,
        pid_namespace: super::endpoint::NAMESPACE_UNREPORTED,
        inner_pid: match value.get("inner-pid") {
            Some(inner) => Some(i32::try_from(inner.as_i64()?).ok()?),
            None => None,
        },
        inner_start: match value.get("inner-start") {
            Some(start) => Some(start.as_u64()?),
            None => None,
        },
        ended: match value.get("ended") {
            Some(ended) => ended.as_bool()?,
            None => false,
        },
    })
}

/// The closure's steps in the order it runs them (the names of `steps`); the marker is the last, right before the
/// exec, so nothing that fails after it can be a sandbox failure and nothing that fails
/// before it can come with a marker.
#[cfg(test)]
pub(crate) const STEP_ORDER: [&str; 10] = [
    "session",
    "identity",
    "capabilities",
    "landlock",
    "seccomp",
    "marker-descriptor",
    "stdin",
    "descriptors",
    "umask",
    "marker",
];

/// One step of the closure: its name, the exit code of its failure and what it does.
#[cfg(any(test, feature = "sandbox-hakoniwa"))]
pub(crate) type Step<'a> = (&'static str, i32, Box<dyn Fn() -> Result<(), String> + 'a>);

/// Runs `steps` in order and stops at the first failure with its code and reason: later
/// steps (the marker among them) do not run.
///
/// # Errors
///
/// The failing step's code and reason.
#[cfg(any(test, feature = "sandbox-hakoniwa"))]
pub(crate) fn run_steps(steps: Vec<Step<'_>>) -> Result<(), (i32, String)> {
    for (name, code, step) in steps {
        step().map_err(|why| (code, format!("{name}: {why}")))?;
    }
    Ok(())
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
    /// A Landlock ruleset that could not be built or applied on a kernel that has Landlock
    /// (a kernel without it is not a failure).
    pub const LANDLOCK: i32 = 117;
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

#[cfg(all(test, feature = "sandbox-hakoniwa"))]
pub(crate) use run::apply_landlock;
#[cfg(feature = "sandbox-hakoniwa")]
pub(crate) use run::run;
#[cfg(all(test, feature = "sandbox-hakoniwa"))]
pub(crate) use run::{
    Procs, Reaper, access, after_end, apply_landlock_with, find_child, landlock_step, options,
    report_for, run_guarded, setup_failed, single_threaded, steps, supervise, tmpfs_options,
};

#[cfg(feature = "sandbox-hakoniwa")]
mod run {
    use super::{
        Access, Bind, Grant, Landlock, Plan, Rule, bwrap, exit, identity_mismatch, landlock_rules,
        run_steps, started_line,
    };
    use crate::assistant::runtime::{probe, seccomp};
    use hakoniwa::{Container, MountOptions, Namespace, Stdio};
    use landlock::{
        ABI, Access as _, AccessFs, AccessNet, BitFlags, CompatLevel, Compatible, PathBeneath,
        PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus, Scope, make_bitflags,
    };
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
    /// made as planned fails the start. The binds are not recursive: a mount under a bound
    /// directory is left out (an empty directory), so no submount can stay writable under a
    /// read-only bind.
    pub(crate) fn options(bind: &Bind) -> MountOptions {
        let base = MountOptions::BIND | MountOptions::NOSUID;
        let mut options = match bind.access {
            Access::ReadOnly => base | MountOptions::NODEV | MountOptions::RDONLY,
            Access::ReadWrite => base | MountOptions::NODEV,
            Access::Device => base,
        };
        if rustix::fs::statvfs(&bind.source)
            .is_ok_and(|stat| stat.f_flag.contains(rustix::fs::StatVfsMountFlags::NOEXEC))
        {
            options |= MountOptions::NOEXEC;
        }
        options
    }

    /// The ABI whose access sets Mimik asks for (DEP-25): ABI 6 has every filesystem right the
    /// ruleset names (device `ioctl`, truncation, reparenting), TCP bind and connect (ABI 4) and
    /// the abstract-socket and signal scopes. Newer rights are not asked for, so a kernel from
    /// ABI 6 up reports "full".
    const TARGET: ABI = ABI::V6;

    /// The rights `grant` gives on a directory (a file gets the file ones among them, which
    /// the caller masks).
    pub(crate) fn access(grant: Grant) -> BitFlags<AccessFs> {
        match grant {
            Grant::ReadExec => make_bitflags!(AccessFs::{Execute | ReadFile | ReadDir}),
            Grant::ReadFile => AccessFs::ReadFile.into(),
            Grant::Read => make_bitflags!(AccessFs::{ReadFile | ReadDir}),
            Grant::List => AccessFs::ReadDir.into(),
            // No `Execute`, `MakeChar`, `MakeBlock` or `Refer`: no node can be made and
            // nothing is moved between directories, and the right to `execve` a file written
            // here is withheld (the tmpfs mounts are `noexec` as well, which also stops
            // executable mappings of what is written).
            Grant::ReadWrite => make_bitflags!(AccessFs::{
                ReadFile | ReadDir | WriteFile | Truncate | RemoveFile | RemoveDir | MakeReg
                    | MakeDir | MakeSock | MakeFifo | MakeSym
            }),
            Grant::Device => make_bitflags!(AccessFs::{ReadFile | WriteFile | Truncate | IoctlDev}),
        }
    }

    /// The mount options of a tmpfs of the plan: never `exec`, `suid` or `dev` (the server
    /// writes data there and nothing in it is to run; the CPU and Vulkan paths map no
    /// executable memory from files there), read-only for a mask.
    pub(crate) fn tmpfs_options(tmpfs: &super::Tmpfs) -> MountOptions {
        let base = MountOptions::NOSUID | MountOptions::NODEV | MountOptions::NOEXEC;
        if tmpfs.read_only {
            base | MountOptions::RDONLY
        } else {
            base
        }
    }

    /// The kernel's Landlock ABI, by the version probe of `landlock_create_ruleset` (the
    /// crate's own probe lumps every error as "not implemented", which would hide a real
    /// failure).
    fn probe_abi() -> Result<i32, i32> {
        // SAFETY: the version flag takes a null attribute of size 0 and touches no memory.
        let abi = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                std::ptr::null::<libc::c_void>(),
                0_usize,
                1_u32, // LANDLOCK_CREATE_RULESET_VERSION
            )
        };
        if abi < 0 {
            Err(std::io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or(libc::EIO))
        } else {
            i32::try_from(abi).map_err(|_| libc::EIO)
        }
    }

    /// Restricts the calling thread (the closure is single-threaded) to `rules`, denies TCP
    /// bind and connect and scopes abstract Unix sockets and signals to the new domain, best
    /// effort: a kernel without Landlock, or with an older ABI, enforces what it has and the
    /// answer says how much.
    ///
    /// "Not available" (the start goes on with the namespaces alone) is exactly a version probe
    /// that fails with `ENOSYS` (no Landlock in the kernel), `EOPNOTSUPP` (disabled at boot) or
    /// `EPERM` (a seccomp or service-manager filter around Mimik refuses the call: the same
    /// effect as no Landlock, and the layer is defence in depth). Any other probe error, and
    /// every failure after a successful probe (a path that cannot be opened, a rule or the
    /// restriction refused), fails the start with exit 117: a bug or a path the mounts did not
    /// make, never silently weaker.
    ///
    /// # Errors
    ///
    /// The path or call that failed.
    #[cfg(test)]
    pub(crate) fn apply_landlock(rules: &[Rule]) -> Result<Landlock, String> {
        apply_landlock_with(rules, probe_abi)
    }

    /// The closure's Landlock step over `rules` with the version probe given: the report goes
    /// to `found` for the marker step, a failure is [`exit::LANDLOCK`].
    pub(crate) fn landlock_step<'a>(
        rules: Vec<Rule>,
        found: std::rc::Rc<std::cell::Cell<Option<Landlock>>>,
        probe: impl Fn() -> Result<i32, i32> + 'a,
    ) -> super::Step<'a> {
        (
            "landlock",
            exit::LANDLOCK,
            Box::new(move || {
                found.set(Some(apply_landlock_with(&rules, &probe)?));
                Ok(())
            }),
        )
    }

    /// [`apply_landlock`] with the version probe given (tests simulate its errors).
    ///
    /// # Errors
    ///
    /// As [`apply_landlock`].
    pub(crate) fn apply_landlock_with(
        rules: &[Rule],
        probe: impl FnOnce() -> Result<i32, i32>,
    ) -> Result<Landlock, String> {
        fn failed<E: std::fmt::Display>(what: &str) -> impl Fn(E) -> String + '_ {
            move |error| format!("{what}: {error}")
        }
        // The kernel's ABI from Mimik's own probe. From here on the kernel is known to have
        // Landlock: the ruleset is built for exactly this ABI with every right required, and
        // anything that then fails or is not fully enforced (the crate probing again and
        // failing, a refused rule) is an error, never "not available".
        let kernel = match probe() {
            Ok(abi) if abi >= 1 => abi,
            Ok(abi) => return Err(format!("probe: the kernel reports ABI {abi}")),
            Err(libc::ENOSYS | libc::EOPNOTSUPP | libc::EPERM) => return Ok(Landlock::Unavailable),
            Err(errno) => return Err(failed("probe")(std::io::Error::from_raw_os_error(errno))),
        };
        let used = ABI::from(kernel).min(TARGET);
        // The paths first: opening them asks nothing of Landlock.
        let mut opened = Vec::with_capacity(rules.len());
        for rule in rules {
            let fd = PathFd::new(&rule.path).map_err(failed(&rule.path))?;
            let mut rights = access(rule.grant) & AccessFs::from_all(used);
            // A file takes no directory right (the crate's helper does the same).
            if !std::fs::metadata(&rule.path)
                .map_err(failed(&rule.path))?
                .is_dir()
            {
                rights &= AccessFs::from_file(used);
            }
            opened.push((fd, rights));
        }
        let mut ruleset = Ruleset::default()
            .set_compatibility(CompatLevel::HardRequirement)
            .handle_access(AccessFs::from_all(used))
            .map_err(failed("filesystem rights"))?;
        if used >= ABI::V4 {
            ruleset = ruleset
                .handle_access(AccessNet::from_all(used))
                .map_err(failed("network rights"))?;
        }
        if used >= ABI::V6 {
            ruleset = ruleset
                .scope(Scope::from_all(used))
                .map_err(failed("scopes"))?;
        }
        let mut ruleset = ruleset.create().map_err(failed("ruleset"))?;
        for ((fd, rights), rule) in opened.into_iter().zip(rules) {
            ruleset = ruleset
                .add_rule(PathBeneath::new(fd, rights))
                .map_err(failed(&rule.path))?;
        }
        let status = ruleset.restrict_self().map_err(failed("restrict_self"))?;
        if status.ruleset != RulesetStatus::FullyEnforced {
            return Err(format!("restrict_self: not fully enforced ({status:?})"));
        }
        // The kernel's own ABI (above the crate's newest when the kernel is newer).
        let abi = u8::try_from(kernel).unwrap_or(u8::MAX);
        Ok(report_for(abi))
    }

    /// What Mimik reports for a kernel of ABI `abi` whose ruleset was fully enforced: the
    /// rights asked for (ABI 6) are all there from ABI 6 up; below, only a subset was asked.
    pub(crate) fn report_for(abi: u8) -> Landlock {
        if abi >= 6 {
            Landlock::Full { abi }
        } else {
            Landlock::Partial { abi }
        }
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
                tmpfs_options(tmpfs),
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

    /// The descriptor the marker is written on: the standard input's pipe end, moved here
    /// close-on-exec before descriptor 0 is replaced, kept through the cleanup, closed by the
    /// exec itself.
    const MARKER_FD: RawFd = 3;

    fn write_marker(landlock: Option<Landlock>) -> Result<(), String> {
        // SAFETY: the cleanup keeps this close-on-exec descriptor for this write only.
        let marker = unsafe { std::os::fd::BorrowedFd::borrow_raw(MARKER_FD) };
        let mut line = MARKER_LINE.to_vec();
        if let Some(landlock) = landlock {
            line.extend_from_slice(landlock.line().as_bytes());
        }
        match rustix::io::write(marker, &line) {
            Ok(written) if written == line.len() => Ok(()),
            Ok(_) => Err("the marker was written short".to_owned()),
            Err(error) => Err(error.to_string()),
        }
    }

    fn drop_bounding_set() -> Result<(), String> {
        for capability in 0..64 {
            // SAFETY: PR_CAPBSET_DROP takes a capability number and touches no memory; a
            // number above the kernel's last one answers EINVAL, which ends the loop.
            let result = unsafe { libc::prctl(libc::PR_CAPBSET_DROP, capability, 0, 0, 0) };
            if result < 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::EINVAL) {
                    return Ok(());
                }
                return Err(format!("capability {capability}: {error}"));
            }
        }
        Ok(())
    }

    fn clear_capabilities() -> Result<(), String> {
        drop_bounding_set()?;
        for set in [
            caps::CapSet::Ambient,
            caps::CapSet::Inheritable,
            caps::CapSet::Permitted,
            caps::CapSet::Effective,
        ] {
            caps::clear(None, set).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    /// Closes every descriptor above `keep`: `close_range`, else each one `/proc/self/fd`
    /// lists (a listing that fails fails the step).
    fn close_above(keep: RawFd) -> Result<(), String> {
        let first = u32::try_from(keep + 1).map_err(|_| "bad descriptor".to_owned())?;
        // SAFETY: a plain system call closing descriptors this process no longer uses.
        if unsafe { libc::syscall(libc::SYS_close_range, first, u32::MAX, 0_u32) } == 0 {
            return Ok(());
        }
        let listing = probe::fd_listing().map_err(|error| error.to_string())?;
        for fd in listing {
            if fd > keep {
                // SAFETY: closing an inherited descriptor that is not used again.
                unsafe { libc::close(fd) };
            }
        }
        Ok(())
    }

    /// The closure's steps over `plan`, in [`STEP_ORDER`]; `umask` is the runner's own,
    /// restored before the exec.
    pub(crate) fn steps(plan: &Plan, umask: rustix::fs::Mode) -> Vec<super::Step<'_>> {
        fn step<'a>(
            name: &'static str,
            code: i32,
            run: Box<dyn Fn() -> Result<(), String> + 'a>,
        ) -> super::Step<'a> {
            (name, code, run)
        }
        // What the Landlock step found, for the marker step to report.
        let landlock: std::rc::Rc<std::cell::Cell<Option<Landlock>>> =
            std::rc::Rc::new(std::cell::Cell::new(None));
        let (landlock_set, landlock_read) = (landlock.clone(), landlock);
        vec![
            step(
                "session",
                exit::SESSION,
                Box::new(|| {
                    rustix::process::setsid()
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                }),
            ),
            step(
                "identity",
                exit::IDENTITY,
                Box::new(|| match identity_mismatch(&plan.binds, mounted) {
                    Some(bind) => Err(format!("{} is not the verified object", bind.target)),
                    None => Ok(()),
                }),
            ),
            step(
                "capabilities",
                exit::CAPABILITIES,
                Box::new(clear_capabilities),
            ),
            // After the mounts and the identity check, before seccomp: best effort. A kernel
            // without Landlock, or with an older ABI, leaves the namespaces alone in force and
            // the report says so; an error on a kernel that has it fails the start.
            landlock_step(landlock_rules(plan), landlock_set, probe_abi),
            step(
                "seccomp",
                exit::SECCOMP,
                Box::new(|| {
                    seccomp::install(&seccomp::program(seccomp::Profile::Sandbox))
                        .map_err(|error| error.to_string())
                }),
            ),
            step(
                "marker-descriptor",
                exit::DESCRIPTORS,
                Box::new(|| {
                    // SAFETY: descriptor 0 is the marker pipe's write end, handed over by the
                    // runner as the program's standard input and used only here, by number.
                    let stdin = unsafe { std::os::fd::BorrowedFd::borrow_raw(0) };
                    let moved = rustix::io::fcntl_dupfd_cloexec(stdin, MARKER_FD)
                        .map_err(|error| error.to_string())?;
                    // Every other descriptor was closed by Hakoniwa, so the lowest free one
                    // from 3 is 3: the cleanup below keeps exactly this number.
                    if std::os::fd::AsRawFd::as_raw_fd(&moved) == MARKER_FD {
                        std::mem::forget(moved);
                        Ok(())
                    } else {
                        Err("the marker descriptor is not where it must be".to_owned())
                    }
                }),
            ),
            step(
                "stdin",
                exit::DESCRIPTORS,
                Box::new(|| {
                    let null = std::fs::File::open("/dev/null").map_err(|e| e.to_string())?;
                    rustix::stdio::dup2_stdin(&null).map_err(|error| error.to_string())
                }),
            ),
            step(
                "descriptors",
                exit::DESCRIPTORS,
                Box::new(|| close_above(MARKER_FD)),
            ),
            step(
                "umask",
                exit::SPEC,
                Box::new(move || {
                    rustix::process::umask(umask);
                    Ok(())
                }),
            ),
            step(
                "marker",
                exit::MARKER,
                Box::new(move || write_marker(landlock_read.get())),
            ),
        ]
    }

    /// Inside the sandbox, as the program's process just before it becomes the server: the
    /// steps (the marker the last), then the exec.
    fn inside(plan: &Plan, umask: rustix::fs::Mode) -> i32 {
        if plan.argv.is_empty() {
            return refuse(exit::SPEC, "nothing to execute");
        }
        if let Err((code, why)) = run_steps(steps(plan, umask)) {
            return refuse(code, &why);
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
    pub(crate) fn run(plan: Plan, status: RawFd, marker: RawFd, ack: RawFd) -> i32 {
        // The test binary plays the runner (as a libtest helper, which has a second thread
        // beside a main thread that only waits); the shipped runner is `main` and alone, and
        // the live suite runs it as such (`MIMIK_LIVE_RUNNER`).
        run_guarded(plan, status, marker, ack, !cfg!(test))
    }

    /// [`run`], with the single-thread guard as asked: refused before anything is taken or
    /// spawned.
    pub(crate) fn run_guarded(
        plan: Plan,
        status: RawFd,
        marker: RawFd,
        ack: RawFd,
        guard: bool,
    ) -> i32 {
        if guard && !single_threaded() {
            return refuse(exit::SPEC, "the runner is not single-threaded");
        }
        // Files the sandbox makes (the token's) are private to Mimik's user; the closure
        // restores the runner's own mask before the exec.
        let umask = rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077));
        // SAFETY: both descriptors are handed over by the launcher and owned by this
        // process from here on; nothing else uses these numbers.
        let (status, marker, ack) = unsafe {
            (
                OwnedFd::from_raw_fd(status),
                OwnedFd::from_raw_fd(marker),
                OwnedFd::from_raw_fd(ack),
            )
        };
        let mut status_file = std::fs::File::from(status);
        let workdir = plan.workdir.clone();
        let container = container(&plan);
        let plan = Arc::new(plan);
        let inner = Arc::clone(&plan);
        // SAFETY: the closure runs after fork in a process with no other thread (checked
        // above), so allocating and locking in it is sound.
        let mut command = unsafe { container.command_from_closure(move || inside(&inner, umask)) };
        command.current_dir(&workdir).stdin(Stdio::from(marker));
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => return refuse(exit::SPEC, &format!("hakoniwa: {error}")),
        };
        // A setup that Hakoniwa itself fails ends the reaper at once; a program that runs is
        // still there after a moment. A setup failure of Hakoniwa's own is not reported (as
        // bubblewrap reports nothing when it fails first); everything else is, with the
        // reaper's pid, and the marker tells whether the program got to its exec.
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
        supervise(
            &mut child,
            reaper,
            early,
            &Host,
            &mut status_file,
            &ack,
            ACK_WITHIN,
        )
    }

    /// How long the runner waits for the launcher's acknowledgement.
    const ACK_WITHIN: std::time::Duration = std::time::Duration::from_secs(30);

    /// The sandbox's reaper process as the runner sees it (Hakoniwa's `Child`, or a stand-in
    /// in tests).
    pub(crate) trait Reaper {
        fn wait(&mut self) -> Result<hakoniwa::ExitStatus, hakoniwa::Error>;
        fn try_wait(&mut self) -> Result<Option<hakoniwa::ExitStatus>, hakoniwa::Error>;
        fn kill(&mut self);
    }

    impl Reaper for hakoniwa::Child {
        fn wait(&mut self) -> Result<hakoniwa::ExitStatus, hakoniwa::Error> {
            hakoniwa::Child::wait(self)
        }
        fn try_wait(&mut self) -> Result<Option<hakoniwa::ExitStatus>, hakoniwa::Error> {
            hakoniwa::Child::try_wait(self)
        }
        fn kill(&mut self) {
            let _ = hakoniwa::Child::kill(self);
        }
    }

    /// What the runner reads of `/proc` to find the server (injectable: the tests substitute
    /// processes between the steps).
    pub(crate) trait Procs {
        fn pids(&self) -> Vec<i32>;
        /// A pidfd on `pid`, which pins the process it opened.
        fn open(&self, pid: i32) -> std::io::Result<OwnedFd>;
        /// `(parent, start time)` of `pid`, from one read of its `stat`.
        fn stat(&self, pid: i32) -> std::io::Result<(i32, u64)>;
        fn exited(&self, pidfd: &OwnedFd) -> bool;
    }

    /// The host's `/proc`.
    pub(crate) struct Host;

    impl Procs for Host {
        fn pids(&self) -> Vec<i32> {
            std::fs::read_dir("/proc")
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|entry| entry.file_name().to_string_lossy().parse().ok())
                .collect()
        }
        fn open(&self, pid: i32) -> std::io::Result<OwnedFd> {
            let pid = rustix::process::Pid::from_raw(pid)
                .ok_or_else(|| std::io::Error::other("pid 0"))?;
            Ok(rustix::process::pidfd_open(
                pid,
                rustix::process::PidfdFlags::empty(),
            )?)
        }
        fn stat(&self, pid: i32) -> std::io::Result<(i32, u64)> {
            crate::assistant::runtime::rundir::stat_of(pid)
        }
        fn exited(&self, pidfd: &OwnedFd) -> bool {
            crate::assistant::runtime::endpoint::exited(pidfd)
        }
    }

    /// The one live child of `parent`, as `(pid, start time)`. Each candidate is pinned by a
    /// pidfd first; its parent and start time come from one read of its `stat`; and it must
    /// still be running after that read (one that has ended may be a reused pid by then, and
    /// is not the server). `Ok(None)`: no live child. `Err`: more than one.
    pub(crate) fn find_child(procs: &impl Procs, parent: i32) -> Result<Option<(i32, u64)>, ()> {
        let mut found = None;
        for pid in procs.pids() {
            let Ok(pidfd) = procs.open(pid) else {
                continue;
            };
            let Ok((owner, start)) = procs.stat(pid) else {
                continue;
            };
            if procs.exited(&pidfd) || owner != parent {
                continue;
            }
            if found.replace((pid, start)).is_some() {
                return Err(());
            }
        }
        Ok(found)
    }

    /// After the sandbox is spawned: reports it, holds it for the launcher's pinning and
    /// waits for the program. `early` says the reaper had already ended when it was looked at.
    pub(crate) fn supervise<R: Reaper>(
        child: &mut R,
        reaper: i32,
        early: bool,
        procs: &impl Procs,
        status: &mut impl Write,
        ack: &OwnedFd,
        ack_within: std::time::Duration,
    ) -> i32 {
        let mut report = |inner: Option<(i32, u64)>, ended: bool| {
            status.write_all(started_line(reaper, inner, ended).as_bytes())
        };
        if early {
            // Reaped here: its status came from the server's own exit, so the server is gone
            // (`ended`).
            return after_end(child.wait(), || report(None, true));
        }
        // The program is PID 1 of the namespace, the reaper's one child: its host PID and
        // start time are reported, so Mimik can watch the process that holds the memory.
        match find_child(procs, reaper) {
            Ok(Some(inner)) => {
                if report(Some(inner), false).is_err() {
                    return refuse(exit::SPEC, "the status descriptor cannot be written");
                }
                // Mimik pins the reaper and the server, then says so; the reaper is not reaped
                // before (a zombie can still be pinned), and a launcher that fails closes the
                // pipe instead.
                if !acknowledged(ack, ack_within) {
                    child.kill();
                    let _ = child.wait();
                    return refuse(exit::SPEC, "the launcher did not acknowledge the sandbox");
                }
                finish(child)
            }
            // No live child: the reaper has ended in the moments since the early window (take
            // its status like the early branch), or it is alive with nothing to reap, an error.
            Ok(None) => match child.try_wait() {
                Ok(Some(done)) => after_end(Ok(done), || report(None, true)),
                Ok(None) => {
                    child.kill();
                    let _ = child.wait();
                    refuse(exit::SPEC, "the sandbox's server process cannot be found")
                }
                Err(error) => refuse(exit::SPEC, &format!("hakoniwa: {error}")),
            },
            Err(()) => {
                child.kill();
                let _ = child.wait();
                refuse(exit::SPEC, "the sandbox has more than one process")
            }
        }
    }

    /// Whether one byte arrives on `ack` within `within` (a closed pipe or a timeout is no;
    /// an interrupted wait goes on).
    pub(crate) fn acknowledged(ack: &OwnedFd, within: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let mut fds = [rustix::event::PollFd::new(
                ack,
                rustix::event::PollFlags::IN,
            )];
            let wait = rustix::time::Timespec {
                tv_sec: i64::try_from(left.as_secs()).unwrap_or(i64::MAX),
                tv_nsec: i64::from(left.subsec_nanos()),
            };
            match rustix::event::poll(&mut fds, Some(&wait)) {
                Ok(0) => return false,
                Ok(_) => break,
                Err(rustix::io::Errno::INTR) => {}
                Err(_) => return false,
            }
        }
        let mut byte = [0_u8; 1];
        loop {
            match rustix::io::read(ack, &mut byte) {
                Ok(read) => return read == 1,
                Err(rustix::io::Errno::INTR) => {}
                Err(_) => return false,
            }
        }
    }

    /// The end of a sandbox that ended at once: a setup failure of Hakoniwa's own is said and
    /// not reported (the marker never came); a program that ran and ended is reported by
    /// `report` (the sandbox was up) and its code returned.
    pub(crate) fn after_end(
        result: Result<hakoniwa::ExitStatus, hakoniwa::Error>,
        report: impl FnOnce() -> std::io::Result<()>,
    ) -> i32 {
        match result {
            Ok(status) if setup_failed(&status) => said(&status),
            Ok(status) => {
                if report().is_err() {
                    return refuse(exit::SPEC, "the status descriptor cannot be written");
                }
                status.code
            }
            Err(error) => refuse(exit::SPEC, &format!("hakoniwa: {error}")),
        }
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
    fn finish(child: &mut impl Reaper) -> i32 {
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
