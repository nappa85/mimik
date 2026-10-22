//! The CPU path's thread count (ASSIST-41): the physical cores minus one, at least 1,
//! configurable. Physical cores are the distinct `core_cpus_list` sets (SMT siblings share
//! one) of the CPUs sysfs lists; without sysfs, the available parallelism stands in.
use std::collections::BTreeSet;
use std::path::Path;

/// Distinct physical cores under a sysfs CPU directory.
pub(crate) fn physical_cores_at(cpus: &Path) -> Option<usize> {
    let mut cores = BTreeSet::new();
    for entry in std::fs::read_dir(cpus).ok()?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(number) = name.strip_prefix("cpu") else {
            continue;
        };
        if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        // An offline CPU has no topology directory.
        if let Ok(siblings) = std::fs::read_to_string(entry.path().join("topology/core_cpus_list"))
        {
            cores.insert(siblings.trim().to_owned());
        }
    }
    (!cores.is_empty()).then_some(cores.len())
}

/// The default: physical cores minus one, at least 1.
pub(crate) fn default_threads() -> u32 {
    let cores = physical_cores_at(Path::new("/sys/devices/system/cpu")).unwrap_or_else(|| {
        std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
    });
    u32::try_from(cores.saturating_sub(1).max(1)).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cpu(root: &Path, n: usize, siblings: Option<&str>) {
        let dir = root.join(format!("cpu{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(siblings) = siblings {
            std::fs::create_dir_all(dir.join("topology")).unwrap();
            std::fs::write(dir.join("topology/core_cpus_list"), format!("{siblings}\n")).unwrap();
        }
    }

    #[test]
    fn smt_siblings_count_once_and_offline_cpus_not_at_all() {
        let root = tempfile::tempdir().unwrap();
        // Six cores with two threads each (the dev host's Ryzen 5 1600 shape).
        for core in 0..6 {
            cpu(root.path(), core, Some(&format!("{core},{}", core + 6)));
            cpu(root.path(), core + 6, Some(&format!("{core},{}", core + 6)));
        }
        std::fs::create_dir_all(root.path().join("cpufreq")).unwrap();
        cpu(root.path(), 12, None);
        assert_eq!(physical_cores_at(root.path()), Some(6));
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(physical_cores_at(empty.path()), None);
        assert!(default_threads() >= 1);
    }
}
