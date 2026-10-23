//! The GPU device choice (assisted sheet plan §1.4a, "Device"; ASSIST-27, HW-1).
//!
//! Render nodes come from sysfs (`/sys/class/drm/renderD*` with their PCI devices); the PCI
//! address is a node's identity across restarts and tells two identical cards apart. The
//! name passed to `--device` always comes from the GPU build's `--list-devices` run inside the
//! level-1 sandbox with only that node bound (enumeration names such as `Vulkan0` change with
//! the set of visible devices). Software ICDs (`llvmpipe`, `lavapipe`, `SwiftShader`) are
//! excluded by description before any count, and each per-node list must then show exactly
//! one hardware GPU.
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// A PCI address as sysfs names a device: domain, bus, device and function.
static PCI: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^[0-9a-f]{4}:[0-9a-f]{2}:[0-1][0-9a-f]\.[0-7]$").expect("pattern")
});

/// One device line of `--list-devices` (`"  %s: %s (%zu MiB, %zu MiB free)"`, the pinned
/// build's `common_print_available_devices`); the description may hold parentheses itself.
static LINE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"^  ([A-Za-z][A-Za-z0-9_-]{0,31}): (.{1,256}) \((\d{1,12}) MiB, (\d{1,12}) MiB free\)$",
    )
    .expect("pattern")
});

/// The listing's first line.
pub(crate) const HEADER: &str = "Available devices:";
/// An empty listing's only device line.
const NONE: &str = "  (none)";
/// Descriptions of software renderers (Mesa's llvmpipe and lavapipe, Google's `SwiftShader`),
/// lower case.
const SOFTWARE: &[&str] = &["llvmpipe", "lavapipe", "swiftshader"];

/// `--assistant-device`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Choice {
    /// The best hardware GPU, else the CPU.
    Auto,
    /// The CPU path only: no device is ever listed.
    Cpu,
    /// The render node of this PCI address (`0000:0c:00.0`).
    Pci(String),
}

impl Choice {
    /// `auto`, `cpu` or a PCI address (`0000:0c:00.0`, or `0c:00.0` for domain 0).
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        match text {
            "auto" => return Ok(Self::Auto),
            "cpu" => return Ok(Self::Cpu),
            _ => {}
        }
        let lower = text.to_ascii_lowercase();
        let full = if lower.len() == 7 {
            format!("0000:{lower}")
        } else {
            lower
        };
        if PCI.is_match(&full) {
            Ok(Self::Pci(full))
        } else {
            Err(format!(
                "--assistant-device must be auto, cpu or a PCI address such as 0000:0c:00.0, not {}",
                text.escape_debug()
            ))
        }
    }
}

/// Where sysfs and the render nodes are (tests give fake trees).
#[derive(Clone, Debug)]
pub(crate) struct Roots {
    pub sys: PathBuf,
    pub dev: PathBuf,
}

impl Default for Roots {
    fn default() -> Self {
        Self {
            sys: PathBuf::from("/sys"),
            dev: PathBuf::from("/dev/dri"),
        }
    }
}

/// A render node of a PCI device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Node {
    /// `renderD128`.
    pub name: String,
    /// The host's device file, bound into the sandbox at `/dev/dri/<name>`.
    pub dev: PathBuf,
    /// Its major and minor numbers.
    pub rdev: (u32, u32),
    /// The PCI device's sysfs folder (its real path).
    pub sysfs: PathBuf,
    /// The PCI address.
    pub pci: String,
    /// `mem_info_vram_total` where the driver reports it (amdgpu), in bytes.
    pub vram: Option<u64>,
}

/// The render nodes of PCI devices under `roots`, by name.
pub(crate) fn scan(roots: &Roots) -> Vec<Node> {
    let Ok(entries) = std::fs::read_dir(roots.sys.join("class/drm")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| {
            name.strip_prefix("renderD").is_some_and(|n| {
                !n.is_empty() && n.len() <= 4 && n.bytes().all(|b| b.is_ascii_digit())
            })
        })
        .collect();
    names.sort();
    names
        .into_iter()
        .filter_map(|name| node(roots, name))
        .collect()
}

/// A number from a sysfs attribute (decimal, or hex with `0x`).
fn attribute(path: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim();
    match text.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

/// One render node: its PCI device (the `subsystem` is `pci` and the folder is named by a PCI
/// address), its numbers from sysfs, and a host device file of exactly those numbers.
fn node(roots: &Roots, name: String) -> Option<Node> {
    let class = roots.sys.join("class/drm").join(&name);
    let sysfs = std::fs::canonicalize(class.join("device")).ok()?;
    let subsystem = std::fs::canonicalize(sysfs.join("subsystem")).ok()?;
    if subsystem.file_name()? != "pci" {
        return None;
    }
    let pci = sysfs.file_name()?.to_str()?.to_owned();
    if !PCI.is_match(&pci) {
        return None;
    }
    let numbers = std::fs::read_to_string(class.join("dev")).ok()?;
    let (major, minor) = numbers.trim().split_once(':')?;
    let rdev = (major.parse().ok()?, minor.parse().ok()?);
    let dev = roots.dev.join(&name);
    let stat = rustix::fs::stat(&dev).ok()?;
    let is_char =
        rustix::fs::FileType::from_raw_mode(stat.st_mode) == rustix::fs::FileType::CharacterDevice;
    if !is_char
        || (
            rustix::fs::major(stat.st_rdev),
            rustix::fs::minor(stat.st_rdev),
        ) != rdev
    {
        return None;
    }
    let vram = attribute(&sysfs.join("mem_info_vram_total"));
    Some(Node {
        name,
        dev,
        rdev,
        sysfs,
        pci,
        vram,
    })
}

/// One line of `--list-devices`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Listed {
    /// `Vulkan0`: what `--device` takes.
    pub name: String,
    pub description: String,
    pub total_mib: u64,
    pub free_mib: u64,
}

/// The pinned build's `--list-devices` output.
pub(crate) fn parse_listing(stdout: &str) -> Result<Vec<Listed>, String> {
    let changed = || "the device listing is not in the pinned build's format".to_owned();
    let mut lines = stdout.lines();
    if lines.next() != Some(HEADER) {
        return Err(changed());
    }
    let rest: Vec<&str> = lines.collect();
    if rest == [NONE] {
        return Ok(Vec::new());
    }
    rest.iter()
        .map(|line| {
            let captures = LINE.captures(line).ok_or_else(changed)?;
            Ok(Listed {
                name: captures[1].to_owned(),
                description: captures[2].to_owned(),
                total_mib: captures[3].parse().map_err(|_| changed())?,
                free_mib: captures[4].parse().map_err(|_| changed())?,
            })
        })
        .collect::<Result<Vec<_>, String>>()
        .and_then(|listed| {
            if listed.is_empty() {
                Err(changed())
            } else {
                Ok(listed)
            }
        })
}

/// Whether a device description names a software renderer.
pub(crate) fn is_software(description: &str) -> bool {
    let lower = description.to_ascii_lowercase();
    SOFTWARE.iter().any(|name| lower.contains(name))
}

/// The one hardware GPU of a per-node listing, software ICDs excluded first.
pub(crate) fn one_hardware(listed: &[Listed]) -> Result<&Listed, String> {
    let hardware: Vec<&Listed> = listed
        .iter()
        .filter(|device| !is_software(&device.description))
        .collect();
    match hardware.as_slice() {
        [one] => Ok(one),
        [] => Err("no hardware GPU is listed (software renderers are not used)".into()),
        more => Err(format!(
            "the sandboxed list shows {} hardware GPUs with one render node bound",
            more.len()
        )),
    }
}

/// A node with its in-sandbox listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub node: Node,
    pub listed: Listed,
}

/// The device to use: under `auto` the remembered PCI address while it is a candidate, else
/// dedicated devices (sysfs reports VRAM) before the others, then by listed memory, ties by
/// PCI address; a PCI address selects that device only.
pub(crate) fn select<'a>(
    candidates: &'a [Candidate],
    choice: &Choice,
    remembered: Option<&str>,
) -> Result<&'a Candidate, String> {
    match choice {
        Choice::Cpu => Err("the CPU path was chosen (--assistant-device cpu)".into()),
        Choice::Pci(pci) => candidates
            .iter()
            .find(|candidate| &candidate.node.pci == pci)
            .ok_or_else(|| format!("no usable hardware GPU at PCI address {pci}")),
        Choice::Auto => {
            if let Some(kept) = remembered.and_then(|pci| {
                candidates
                    .iter()
                    .find(|candidate| candidate.node.pci == pci)
            }) {
                return Ok(kept);
            }
            candidates
                .iter()
                .min_by(|a, b| {
                    let dedicated = |c: &Candidate| c.node.vram.is_some_and(|vram| vram > 0);
                    dedicated(b)
                        .cmp(&dedicated(a))
                        .then(b.listed.total_mib.cmp(&a.listed.total_mib))
                        .then(a.node.pci.cmp(&b.node.pci))
                })
                .ok_or_else(|| "no usable hardware GPU".to_owned())
        }
    }
}

#[cfg(test)]
#[path = "device_tests.rs"]
pub(crate) mod tests;
