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
    "--dev-bind",
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
    /// It carries file capabilities (`security.capability`): not the unprivileged setup this
    /// design assumes.
    Capabilities(String),
    /// The installed bwrap lacks this option.
    Option(&'static str),
    /// Its help text could not be read.
    Help,
    /// Unprivileged user namespaces are disabled (the setting named).
    UserNamespaces(&'static str),
    /// Ubuntu's `AppArmor` restriction on unprivileged user namespaces.
    AppArmor,
}

impl std::fmt::Display for Unusable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => f.write_str("bubblewrap is not installed (/usr/bin/bwrap or /bin/bwrap)"),
            Self::SetId(path) => write!(f, "{path} is set-user-ID or set-group-ID"),
            Self::Capabilities(path) => write!(f, "{path} has file capabilities"),
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

/// Whether a file carries file capabilities.
fn has_capabilities(path: &Path) -> bool {
    let mut value = [0_u8; 64];
    // No such attribute, or no attribute support: none. Anything else, including an error
    // that cannot tell, is refused as if it had them.
    !matches!(
        rustix::fs::getxattr(path, "security.capability", &mut value),
        Err(rustix::io::Errno::NODATA | rustix::io::Errno::NOTSUP)
    )
}

/// The first of `candidates` that exists.
fn locate(candidates: &[&Path]) -> Result<PathBuf, Unusable> {
    locate_with(candidates, has_capabilities)
}

fn locate_with(
    candidates: &[&Path],
    capabilities: impl Fn(&Path) -> bool,
) -> Result<PathBuf, Unusable> {
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
        if capabilities(candidate) {
            return Err(Unusable::Capabilities(candidate.display().to_string()));
        }
        return Ok(candidate.to_path_buf());
    }
    Err(Unusable::Missing)
}

/// The help text of `path` (5 s at most).
fn help(path: &Path) -> Result<String, Unusable> {
    let spawn = || {
        std::process::Command::new(path)
            .arg("--help")
            .env_clear()
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
    };
    // ETXTBSY: a file just written may still be open for writing in a child another thread
    // forked meanwhile (until that child's exec); it passes in moments.
    let mut tries = 0;
    let mut child = loop {
        match spawn() {
            Ok(child) => break child,
            Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) && tries < 50 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return Err(Unusable::Help),
        }
    };
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

fn sysctl(root: &Path, name: &str) -> Option<u64> {
    std::fs::read_to_string(root.join(name))
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
}

/// The one setting that rules user namespaces out for certain, read from `root`.
pub(crate) fn namespaces(root: &Path) -> Result<(), Unusable> {
    if sysctl(root, "sys/user/max_user_namespaces") == Some(0) {
        return Err(Unusable::UserNamespaces("user.max_user_namespaces = 0"));
    }
    Ok(())
}

/// A setting that may explain a failed level-1 probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cause {
    /// `kernel.unprivileged_userns_clone = 0` (Debian's and some other kernels' switch).
    UsernsClone,
    /// Ubuntu's `AppArmor` restriction (`kernel.apparmor_restrict_unprivileged_userns = 1`).
    AppArmor,
}

/// The setting under `root` that may explain a failed probe; it decides nothing (a
/// distribution's `AppArmor` profile for `bwrap`, for example, may still allow it).
pub(crate) fn namespace_cause(root: &Path) -> Option<Cause> {
    if sysctl(root, "sys/kernel/unprivileged_userns_clone") == Some(0) {
        return Some(Cause::UsernsClone);
    }
    if sysctl(root, "sys/kernel/apparmor_restrict_unprivileged_userns") == Some(1) {
        return Some(Cause::AppArmor);
    }
    None
}

/// [`namespace_cause`] as the text bubblewrap's reason carries.
pub(crate) fn namespace_hint(root: &Path) -> Option<String> {
    namespace_cause(root).map(|cause| match cause {
        Cause::UsernsClone => "kernel.unprivileged_userns_clone = 0".to_owned(),
        Cause::AppArmor => Unusable::AppArmor.to_string(),
    })
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
    /// The GPU path: the selected render node and what the driver reads.
    pub gpu: Option<GpuBinds>,
}

/// What a GPU sandbox binds besides the CPU profile: one render node read-write, its PCI
/// device's sysfs folder read-only with the links libdrm resolves to it, and the Vulkan
/// loader's and driver's data folders that exist. The exact list is fixed by the live GPU
/// check (plan §1.4a: "the exact path list is fixed by the 2c-2 tests on both paths").
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GpuBinds {
    /// The host's device file.
    pub dev: PathBuf,
    /// `renderD128`: bound at `/dev/dri/<name>`.
    pub name: String,
    /// The PCI device's sysfs folder (its real path), bound at the same path.
    pub sysfs: PathBuf,
    pub rdev: (u32, u32),
    /// Vulkan ICD manifests and driver data, bound read-only at their own paths.
    pub shared: Vec<PathBuf>,
}

/// The Vulkan loader's ICD manifests and the Mesa driver's data (`amdgpu.ids`, driconf).
pub(crate) const GPU_DATA: &[&str] = &[
    "/usr/share/vulkan",
    "/etc/vulkan",
    "/usr/share/libdrm",
    "/usr/share/drirc.d",
];

/// The folders of `candidates` that exist.
pub(crate) fn shared_data(candidates: &[&Path]) -> Vec<PathBuf> {
    candidates
        .iter()
        .filter(|path| path.is_dir())
        .map(|path| path.to_path_buf())
        .collect()
}

impl GpuBinds {
    /// The binds of `node` with the data folders this host has.
    pub(crate) fn of(node: &super::device::Node) -> Self {
        let data: Vec<&Path> = GPU_DATA.iter().map(Path::new).collect();
        Self {
            dev: node.dev.clone(),
            name: node.name.clone(),
            sysfs: node.sysfs.clone(),
            rdev: node.rdev,
            shared: shared_data(&data),
        }
    }

    /// The node's path inside the sandbox.
    pub(crate) fn inside(&self) -> String {
        format!("/dev/dri/{}", self.name)
    }

    /// What the GPU path binds besides the render node itself, as typed items.
    pub(crate) fn items(&self) -> Vec<Item> {
        let node = self.sysfs.join("drm").join(&self.name);
        let mut items = vec![
            Item::RoBind(self.sysfs.clone()),
            Item::Symlink {
                target: node.clone(),
                at: format!("/sys/dev/char/{}:{}", self.rdev.0, self.rdev.1).into(),
            },
            Item::Symlink {
                target: node,
                at: format!("/sys/class/drm/{}", self.name).into(),
            },
            Item::Dir("/sys/bus/pci".into()),
        ];
        items.extend(self.shared.iter().cloned().map(Item::RoBind));
        items
    }

    fn arguments(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec![
            "--dev-bind".into(),
            self.dev.clone().into_os_string(),
            self.inside().into(),
        ];
        args.extend(self.items().iter().flat_map(Item::arguments));
        args
    }
}

/// One entry of the sandbox's allowlist, independent of the sandbox that builds it (bwrap's
/// arguments and the Hakoniwa plan are both made from these).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Item {
    /// A read-only bind of a host path at the same path inside.
    RoBind(PathBuf),
    /// A symbolic link `at` pointing to `target`.
    Symlink { target: PathBuf, at: PathBuf },
    /// An empty directory.
    Dir(PathBuf),
}

impl Item {
    fn arguments(&self) -> Vec<OsString> {
        match self {
            Self::RoBind(path) => vec![
                "--ro-bind".into(),
                path.clone().into_os_string(),
                path.clone().into_os_string(),
            ],
            Self::Symlink { target, at } => vec![
                "--symlink".into(),
                target.clone().into_os_string(),
                at.clone().into_os_string(),
            ],
            Self::Dir(path) => vec!["--dir".into(), path.clone().into_os_string()],
        }
    }
}

/// The system library directories, the dynamic loader's cache and the CPU topology, as
/// binds or links.
pub(crate) fn system_items() -> Vec<Item> {
    let mut items = Vec::new();
    for dir in [
        "/usr/lib",
        "/usr/lib64",
        "/lib",
        "/lib64",
        "/usr/libexec/ld",
    ] {
        let path = Path::new(dir);
        let Ok(metadata) = std::fs::symlink_metadata(path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            if let Ok(target) = std::fs::read_link(path) {
                items.push(Item::Symlink {
                    target,
                    at: path.to_path_buf(),
                });
            }
        } else if metadata.is_dir() {
            items.push(Item::RoBind(path.to_path_buf()));
        }
    }
    if Path::new("/etc/ld.so.cache").is_file() {
        items.push(Item::RoBind("/etc/ld.so.cache".into()));
    }
    if Path::new("/sys/devices/system/cpu").is_dir() {
        items.push(Item::RoBind("/sys/devices/system/cpu".into()));
    }
    items
}

/// [`system_items`] as bwrap arguments.
pub(crate) fn system_libraries() -> Vec<OsString> {
    system_items().iter().flat_map(Item::arguments).collect()
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
        if let Some(gpu) = &self.gpu {
            args.extend(gpu.arguments());
        }
        for (bytes, dest) in [(TMP_BYTES, "/tmp"), (SHM_BYTES, "/dev/shm")] {
            if self.size {
                args.extend([SIZE.into(), bytes.to_string().into()]);
            }
            args.extend(["--tmpfs".into(), dest.into()]);
        }
        let fd = |n: RawFd| OsString::from(n.to_string());
        args.extend([
            "--bind-fd".into(),
            fd(self.socket_dir_fd),
            SOCKET_DIR.into(),
        ]);
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
        args.extend(["--chdir".into(), RUNTIME.into()]);
        args.extend(self.command.iter().map(OsString::from));
        args
    }
}

/// What bwrap's status descriptor reports first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Started {
    pub child_pid: i32,
    pub pid_namespace: u64,
    /// The sandbox's PID-1 process as seen from outside (Hakoniwa only: its reaper is not
    /// inside the namespace, so the process that holds the memory must be watched itself).
    pub inner_pid: Option<i32>,
    /// Its start time (`/proc/<pid>/stat` field 22): with the pid it is the process's identity,
    /// which a reused pid does not share.
    pub inner_start: Option<u64>,
    /// Hakoniwa: the sandbox had already ended when it was reported, its reaper reaped
    /// after the server's own exit status: the server is proven gone.
    pub ended: bool,
}

/// Reads one status line; `None` when it is not the start report.
pub(crate) fn started(line: &str) -> Option<Started> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    Some(Started {
        child_pid: i32::try_from(value.get("child-pid")?.as_i64()?).ok()?,
        pid_namespace: value.get("pid-namespace")?.as_u64()?,
        inner_pid: None,
        inner_start: None,
        ended: false,
    })
}

#[cfg(test)]
#[path = "bwrap_tests.rs"]
mod tests;
