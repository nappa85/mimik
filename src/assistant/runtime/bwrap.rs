//! Level 1, bubblewrap (assisted sheet plan §1.4a, "Confinement levels", "Preflight"):
//! `bwrap` from a fixed absolute path, never `PATH`, not set-ID, every option the profile
//! uses probed from its help text (no remembered minimum version), and the profile itself.
use std::ffi::OsString;
use std::os::fd::RawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where `bwrap` may be, in order.
pub(crate) const PATHS: &[&str] = &["/usr/bin/bwrap", "/bin/bwrap"];

/// Every option the profile uses (`--size` is optional: without it the temporary mounts are
/// bounded only by `RLIMIT_FSIZE` per file, and the page says so).
pub(crate) const OPTIONS: &[&str] = &[
    "--unshare-all",
    "--die-with-parent",
    "--new-session",
    "--clearenv",
    "--setenv",
    "--cap-drop",
    "--ro-bind",
    "--bind-fd",
    "--ro-bind-fd",
    "--ro-bind-data",
    "--perms",
    "--dev",
    "--proc",
    "--tmpfs",
    "--dir",
    "--symlink",
    "--remount-ro",
    "--seccomp",
    "--json-status-fd",
    "--chdir",
];

/// The size option of `--tmpfs`.
const SIZE: &str = "--size";
/// `/tmp` and `/dev/shm` sizes inside the sandbox.
pub(crate) const TMP_BYTES: u64 = 64 << 20;
pub(crate) const SHM_BYTES: u64 = 256 << 20;

/// Where things are inside the sandbox.
pub(crate) const RUNTIME: &str = "/opt/llama";
pub(crate) const MODEL: &str = "/models/model.gguf";
pub(crate) const SOCKET_DIR: &str = "/run/llama";
pub(crate) const TOKEN: &str = "/run/token";

/// Why bubblewrap cannot be used, shown on the assistant page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Unusable {
    Missing,
    SetId(String),
    /// The installed bwrap lacks this option.
    Option(&'static str),
    /// Its help text could not be read.
    Help,
    /// Unprivileged user namespaces are disabled (the setting named).
    UserNamespaces(&'static str),
    /// Ubuntu's AppArmor restriction on unprivileged user namespaces.
    AppArmor,
}

impl std::fmt::Display for Unusable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => f.write_str("bubblewrap is not installed (/usr/bin/bwrap or /bin/bwrap)"),
            Self::SetId(path) => write!(f, "{path} is set-user-ID or set-group-ID"),
            Self::Option(option) => write!(f, "the installed bubblewrap lacks {option}"),
            Self::Help => f.write_str("the installed bubblewrap's options cannot be read"),
            Self::UserNamespaces(setting) => {
                write!(f, "unprivileged user namespaces are disabled ({setting})")
            }
            Self::AppArmor => f.write_str(
                "AppArmor restricts unprivileged user namespaces (kernel.apparmor_restrict_unprivileged_userns)",
            ),
        }
    }
}

/// A usable bubblewrap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Bwrap {
    pub path: PathBuf,
    /// Whether `--size` is available.
    pub size: bool,
}

/// The first of `candidates` that exists.
fn locate(candidates: &[&Path]) -> Result<PathBuf, Unusable> {
    for candidate in candidates {
        let Ok(metadata) = std::fs::metadata(candidate) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        if metadata.permissions().mode() & 0o6000 != 0 {
            return Err(Unusable::SetId(candidate.display().to_string()));
        }
        return Ok(candidate.to_path_buf());
    }
    Err(Unusable::Missing)
}

/// The help text of `path` (5 s at most).
fn help(path: &Path) -> Result<String, Unusable> {
    let mut child = std::process::Command::new(path)
        .arg("--help")
        .env_clear()
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|_| Unusable::Help)?;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Unusable::Help);
            }
        }
    }
    let output = child.wait_with_output().map_err(|_| Unusable::Help)?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(text)
}

fn lists(help: &str, option: &str) -> bool {
    help.split(|c: char| c.is_whitespace() || c == ',' || c == '=')
        .any(|word| word == option)
}

/// The sysctl settings that disable unprivileged user namespaces, read from `root`.
fn namespaces(root: &Path) -> Result<(), Unusable> {
    let read = |name: &str| {
        std::fs::read_to_string(root.join(name))
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok())
    };
    if read("sys/user/max_user_namespaces") == Some(0) {
        return Err(Unusable::UserNamespaces("user.max_user_namespaces = 0"));
    }
    if read("sys/kernel/unprivileged_userns_clone") == Some(0) {
        return Err(Unusable::UserNamespaces("kernel.unprivileged_userns_clone = 0"));
    }
    if read("sys/kernel/apparmor_restrict_unprivileged_userns") == Some(1) {
        return Err(Unusable::AppArmor);
    }
    Ok(())
}

/// The preflight over `candidates` with sysctls under `proc` (tests inject both).
pub(crate) fn preflight_at(candidates: &[&Path], proc: &Path) -> Result<Bwrap, Unusable> {
    let path = locate(candidates)?;
    let text = help(&path)?;
    for option in OPTIONS {
        if !lists(&text, option) {
            return Err(Unusable::Option(option));
        }
    }
    namespaces(proc)?;
    Ok(Bwrap {
        size: lists(&text, SIZE),
        path,
    })
}

/// The preflight of every start.
pub(crate) fn preflight() -> Result<Bwrap, Unusable> {
    let candidates: Vec<&Path> = PATHS.iter().map(Path::new).collect();
    preflight_at(&candidates, Path::new("/proc"))
}

/// The descriptors and command of one sandbox (numbers as the child sees them).
#[derive(Clone, Debug)]
pub(crate) struct Profile {
    pub size: bool,
    pub status_fd: RawFd,
    pub seccomp_fd: RawFd,
    pub token_fd: RawFd,
    pub socket_dir_fd: RawFd,
    pub model_fd: RawFd,
    /// Each runtime file's descriptor and name, bound under [`RUNTIME`].
    pub runtime: Vec<(RawFd, String)>,
    /// Further read-only binds by descriptor (the probe's own executable).
    pub extra: Vec<(RawFd, String)>,
    /// Variables beside the minimal environment (only the tests' helper variable).
    pub env: Vec<(String, String)>,
    /// The command and its arguments.
    pub command: Vec<String>,
}

/// The system library directories and the dynamic loader's cache, as binds or links.
pub(crate) fn system_libraries() -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    for dir in ["/usr/lib", "/usr/lib64", "/lib", "/lib64", "/usr/libexec/ld"] {
        let path = Path::new(dir);
        let Ok(metadata) = std::fs::symlink_metadata(path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            if let Ok(target) = std::fs::read_link(path) {
                args.extend(["--symlink".into(), target.into_os_string(), dir.into()]);
            }
        } else if metadata.is_dir() {
            args.extend(["--ro-bind".into(), dir.into(), dir.into()]);
        }
    }
    if Path::new("/etc/ld.so.cache").is_file() {
        args.extend([
            "--ro-bind".into(),
            "/etc/ld.so.cache".into(),
            "/etc/ld.so.cache".into(),
        ]);
    }
    if Path::new("/sys/devices/system/cpu").is_dir() {
        args.extend([
            "--ro-bind".into(),
            "/sys/devices/system/cpu".into(),
            "/sys/devices/system/cpu".into(),
        ]);
    }
    args
}

impl Profile {
    /// bwrap's arguments: everything unshared, the minimal environment, the allowlist, the
    /// descriptors, the seccomp program and the status descriptor, then the command.
    pub(crate) fn arguments(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = [
            "--unshare-all",
            "--die-with-parent",
            "--new-session",
            "--clearenv",
            "--setenv",
            "PATH",
            "/usr/bin:/bin",
            "--setenv",
            "LANG",
            "C.UTF-8",
            "--setenv",
            "HOME",
            "/tmp",
            "--cap-drop",
            "ALL",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        for (name, value) in &self.env {
            args.extend(["--setenv".into(), name.into(), value.into()]);
        }
        args.extend(system_libraries());
        args.extend(["--dev", "/dev", "--proc", "/proc"].map(OsString::from));
        for (bytes, dest) in [(TMP_BYTES, "/tmp"), (SHM_BYTES, "/dev/shm")] {
            if self.size {
                args.extend([SIZE.into(), bytes.to_string().into()]);
            }
            args.extend(["--tmpfs".into(), dest.into()]);
        }
        let fd = |n: RawFd| OsString::from(n.to_string());
        args.extend(["--bind-fd".into(), fd(self.socket_dir_fd), SOCKET_DIR.into()]);
        for (n, name) in &self.runtime {
            args.extend([
                "--ro-bind-fd".into(),
                fd(*n),
                format!("{RUNTIME}/{name}").into(),
            ]);
        }
        for (n, dest) in &self.extra {
            args.extend(["--ro-bind-fd".into(), fd(*n), dest.into()]);
        }
        args.extend(["--ro-bind-fd".into(), fd(self.model_fd), MODEL.into()]);
        args.extend([
            "--perms".into(),
            "0400".into(),
            "--ro-bind-data".into(),
            fd(self.token_fd),
            TOKEN.into(),
        ]);
        args.extend(["--remount-ro".into(), "/".into()]);
        args.extend(["--seccomp".into(), fd(self.seccomp_fd)]);
        args.extend(["--json-status-fd".into(), fd(self.status_fd)]);
        args.extend(["--chdir".into(), "/".into()]);
        args.extend(self.command.iter().map(OsString::from));
        args
    }
}

/// What bwrap's status descriptor reports first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Started {
    pub child_pid: i32,
    pub pid_namespace: u64,
}

/// Reads one status line; `None` when it is not the start report.
pub(crate) fn started(line: &str) -> Option<Started> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    Some(Started {
        child_pid: i32::try_from(value.get("child-pid")?.as_i64()?).ok()?,
        pid_namespace: value.get("pid-namespace")?.as_u64()?,
    })
}

/// The `exit-code` of a status line, if it is one.
pub(crate) fn exit_code(line: &str) -> Option<i64> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    value.get("exit-code")?.as_i64()
}

#[cfg(test)]
#[path = "bwrap_tests.rs"]
mod tests;
