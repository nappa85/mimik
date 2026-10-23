//! The CPU path's thread count (ASSIST-41): the physical cores minus one, at least 1,
//! configurable. Physical cores are the distinct `core_cpus_list` sets (SMT siblings share
//! one) of the CPUs this process may run on (`sched_getaffinity`), and no more than the
//! cgroup's CPU quota (`cpu.max`, rounded up); without sysfs, the available parallelism
//! stands in.
use std::collections::BTreeSet;
use std::path::Path;

/// Distinct physical cores under a sysfs CPU directory, among the CPUs `allowed` admits.
pub(crate) fn physical_cores_at(cpus: &Path, allowed: impl Fn(usize) -> bool) -> Option<usize> {
    let mut cores = BTreeSet::new();
    for entry in std::fs::read_dir(cpus).ok()?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(number) = name.strip_prefix("cpu") else {
            continue;
        };
        let Ok(number) = number.parse::<usize>() else {
            continue;
        };
        if !allowed(number) {
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

/// The cgroup v2 CPU quota in CPUs (rounded up) from a `cpu.max` text; `None` without one.
pub(crate) fn quota(cpu_max: &str) -> Option<usize> {
    let mut parts = cpu_max.split_whitespace();
    let quota: u64 = parts.next()?.parse().ok()?;
    let period: u64 = parts.next()?.parse().ok()?;
    (period > 0).then(|| {
        usize::try_from(quota.div_ceil(period))
            .unwrap_or(usize::MAX)
            .max(1)
    })
}

fn own_quota() -> Option<usize> {
    let cgroup = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let path = cgroup.lines().find_map(|line| line.strip_prefix("0::"))?;
    quota_along(Path::new("/sys/fs/cgroup"), path.trim())
}

/// The tightest `cpu.max` quota of the cgroup at `path` (under the v2 hierarchy `root`) and
/// every ancestor: a parent's limit binds an unlimited child.
pub(crate) fn quota_along(root: &Path, path: &str) -> Option<usize> {
    let mut dir = root.to_path_buf();
    let mut tightest = std::fs::read_to_string(dir.join("cpu.max"))
        .ok()
        .and_then(|text| quota(&text));
    for part in path.split('/').filter(|part| !part.is_empty()) {
        dir.push(part);
        let here = std::fs::read_to_string(dir.join("cpu.max"))
            .ok()
            .and_then(|text| quota(&text));
        tightest = match (tightest, here) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
    }
    tightest
}

/// The usable cores: physical cores in the affinity set, at most the quota.
pub(crate) fn usable(physical: Option<usize>, quota: Option<usize>, fallback: usize) -> usize {
    let cores = physical.unwrap_or(fallback);
    quota.map_or(cores, |quota| cores.min(quota))
}

/// The default: usable cores minus one, at least 1.
pub(crate) fn default_threads() -> u32 {
    let affinity = rustix::thread::sched_getaffinity(None).ok();
    let physical = physical_cores_at(Path::new("/sys/devices/system/cpu"), |cpu| {
        affinity.as_ref().is_none_or(|set| set.is_set(cpu))
    });
    let fallback = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let cores = usable(physical, own_quota(), fallback);
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
        assert_eq!(physical_cores_at(root.path(), |_| true), Some(6));
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(physical_cores_at(empty.path(), |_| true), None);
        assert!(default_threads() >= 1);
        // An affinity set of CPUs 0, 6 and 1: two physical cores (0 and 6 are siblings).
        assert_eq!(
            physical_cores_at(root.path(), |cpu| [0, 1, 6].contains(&cpu)),
            Some(2)
        );
    }

    #[test]
    fn a_limited_parent_cgroup_binds_an_unlimited_leaf() {
        let root = tempfile::tempdir().unwrap();
        let leaf = root.path().join("user.slice/mimik.scope");
        std::fs::create_dir_all(&leaf).unwrap();
        std::fs::write(root.path().join("user.slice/cpu.max"), "300000 100000\n").unwrap();
        std::fs::write(leaf.join("cpu.max"), "max 100000\n").unwrap();
        assert_eq!(quota_along(root.path(), "/user.slice/mimik.scope"), Some(3));
        std::fs::write(leaf.join("cpu.max"), "100000 100000\n").unwrap();
        assert_eq!(quota_along(root.path(), "/user.slice/mimik.scope"), Some(1));
        assert_eq!(quota_along(root.path(), "/"), None);
    }

    #[test]
    fn the_cgroup_quota_caps_the_cores() {
        assert_eq!(quota("max 100000\n"), None);
        assert_eq!(quota("200000 100000\n"), Some(2));
        assert_eq!(quota("150000 100000"), Some(2));
        assert_eq!(quota("50000 100000"), Some(1));
        assert_eq!(usable(Some(6), Some(2), 12), 2);
        assert_eq!(usable(Some(6), None, 12), 6);
        assert_eq!(usable(None, Some(4), 12), 4);
    }
}
