//! What the sandbox is made of, independent of the code that builds it (`hako.rs`): the paths
//! inside, the allowlist items, the GPU binds and the host's namespace settings.
pub(crate) use crate::sandbox::host::{Cause, Started, Unusable, namespace_cause, namespaces};
use std::path::{Path, PathBuf};

/// `/tmp` and `/dev/shm` sizes inside the sandbox.
pub(crate) const TMP_BYTES: u64 = 64 << 20;
pub(crate) const SHM_BYTES: u64 = 256 << 20;

/// Where things are inside the sandbox.
pub(crate) const RUNTIME: &str = "/opt/llama";
pub(crate) const MODEL: &str = "/models/model.gguf";
pub(crate) const SOCKET_DIR: &str = "/run/llama";
pub(crate) const TOKEN: &str = "/run/token";

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

pub(crate) use crate::sandbox::system::{Item, LIBRARY_DIRS};

/// The system library directories, the dynamic loader's cache and the CPU topology, as
/// binds or links.
pub(crate) fn system_items() -> Vec<Item> {
    let mut items = crate::sandbox::system::library_items();
    if Path::new("/sys/devices/system/cpu").is_dir() {
        items.push(Item::RoBind("/sys/devices/system/cpu".into()));
    }
    items
}
