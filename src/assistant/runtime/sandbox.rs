//! What the sandbox is made of, independent of the code that builds it (`hako.rs`): the paths
//! inside, the allowlist items, the GPU binds and the host's namespace settings.
use std::path::{Path, PathBuf};

/// `/tmp` and `/dev/shm` sizes inside the sandbox.
pub(crate) const TMP_BYTES: u64 = 64 << 20;
pub(crate) const SHM_BYTES: u64 = 256 << 20;

/// Where things are inside the sandbox.
pub(crate) const RUNTIME: &str = "/opt/llama";
pub(crate) const MODEL: &str = "/models/model.gguf";
pub(crate) const SOCKET_DIR: &str = "/run/llama";
pub(crate) const TOKEN: &str = "/run/token";

/// Why the sandbox cannot be used, shown on the assistant page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Unusable {
    /// Unprivileged user namespaces are disabled (the setting named).
    UserNamespaces(&'static str),
}

impl std::fmt::Display for Unusable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UserNamespaces(setting) => {
                write!(f, "unprivileged user namespaces are disabled ({setting})")
            }
        }
    }
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
/// distribution's `AppArmor` profile may still allow it).
pub(crate) fn namespace_cause(root: &Path) -> Option<Cause> {
    if sysctl(root, "sys/kernel/unprivileged_userns_clone") == Some(0) {
        return Some(Cause::UsernsClone);
    }
    if sysctl(root, "sys/kernel/apparmor_restrict_unprivileged_userns") == Some(1) {
        return Some(Cause::AppArmor);
    }
    None
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
}

/// One entry of the sandbox's allowlist, kept apart from the code that builds the sandbox
/// (the Hakoniwa plan is made from these).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Item {
    /// A read-only bind of a host path at the same path inside.
    RoBind(PathBuf),
    /// A symbolic link `at` pointing to `target`.
    Symlink { target: PathBuf, at: PathBuf },
    /// An empty directory.
    Dir(PathBuf),
}

/// The system library directories (they hold the ELF interpreter, which the kernel opens
/// with the execute right).
pub(crate) const LIBRARY_DIRS: [&str; 5] = [
    "/usr/lib",
    "/usr/lib64",
    "/lib",
    "/lib64",
    "/usr/libexec/ld",
];

/// The system library directories, the dynamic loader's cache and the CPU topology, as
/// binds or links.
pub(crate) fn system_items() -> Vec<Item> {
    let mut items = Vec::new();
    for dir in LIBRARY_DIRS {
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

/// What the runner's status descriptor reports first.
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

#[cfg(test)]
#[path = "sandbox_tests.rs"]
mod tests;
