//! Explicit snapshots of the pre-refactor allowlist, with host discovery injected so that
//! absent library/driver folders and distribution symlinks cannot change the expectations.
use super::*;
use crate::sandbox::{landlock_rules, options, tmpfs_options};
use hakoniwa::MountOptions as Flags;

fn inputs(gpu: Option<&GpuBinds>) -> Inputs<'_> {
    Inputs {
        root: "/data/run/3.root".into(),
        mountinfo: String::new(),
        runtime: vec![
            File {
                name: "llama-server".into(),
                source: "/data/llama/cpu/llama-server".into(),
                identity: (7, 100),
            },
            File {
                name: "libllama.so.0".into(),
                source: "/data/llama/cpu/libllama.so.0".into(),
                identity: (7, 101),
            },
        ],
        model: ("/data/models/m.gguf".into(), (7, 200)),
        generation: ("/data/run/3".into(), (7, 300)),
        token: Secret::new("secret-token\n".into()),
        extra: vec![("/data/mimik".into(), "/opt/probe/mimik".into(), (7, 400))],
        gpu,
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        argv: vec!["/opt/llama/llama-server".into(), "--help".into()],
    }
}

fn system() -> Vec<Item> {
    vec![
        Item::RoBind("/usr/lib".into()),
        Item::RoBind("/usr/lib64".into()),
        Item::Symlink {
            target: "usr/lib".into(),
            at: "/lib".into(),
        },
        Item::Symlink {
            target: "usr/lib64".into(),
            at: "/lib64".into(),
        },
        Item::RoBind("/usr/libexec/ld".into()),
        Item::RoBind("/etc/ld.so.cache".into()),
        Item::RoBind("/sys/devices/system/cpu".into()),
    ]
}

fn pin_profile(gpu: Option<&GpuBinds>) {
    let plan = profile(&inputs(gpu), system());
    assert_eq!(plan.validate(), Ok(()));
    pin_binds(&plan, gpu);
    pin_bind_flags(&plan);
    pin_tmpfs_and_files(&plan);
    pin_rules(&plan, gpu);
    pin_links(&plan, gpu);
    let dirs = if gpu.is_some() {
        vec![
            "/dev",
            "/sys",
            "/sys/bus/pci",
            "/sys/class",
            "/sys/class/drm",
            "/sys/dev",
            "/sys/dev/char",
        ]
    } else {
        vec!["/dev"]
    };
    assert_eq!(plan.dirs, dirs);
    assert_eq!(plan.root, "/data/run/3.root");
    assert_eq!(plan.workdir, "/opt/llama");
    assert_eq!(plan.argv, ["/opt/llama/llama-server", "--help"]);
    assert_eq!(plan.env, [("PATH".to_owned(), "/usr/bin:/bin".to_owned())]);
}

fn pin_binds(plan: &Plan, gpu: Option<&GpuBinds>) {
    let mut expected = vec![
        ("/usr/lib", "/usr/lib", Access::ReadOnly, None),
        ("/usr/lib64", "/usr/lib64", Access::ReadOnly, None),
        ("/usr/libexec/ld", "/usr/libexec/ld", Access::ReadOnly, None),
        (
            "/etc/ld.so.cache",
            "/etc/ld.so.cache",
            Access::ReadOnly,
            None,
        ),
        (
            "/sys/devices/system/cpu",
            "/sys/devices/system/cpu",
            Access::ReadOnly,
            None,
        ),
    ];
    if gpu.is_some() {
        expected.extend([
            (
                "/sys/devices/pci0000:00/0000:0c:00.0",
                "/sys/devices/pci0000:00/0000:0c:00.0",
                Access::ReadOnly,
                None,
            ),
            (
                "/usr/share/vulkan",
                "/usr/share/vulkan",
                Access::ReadOnly,
                None,
            ),
            ("/etc/vulkan", "/etc/vulkan", Access::ReadOnly, None),
            (
                "/usr/share/libdrm",
                "/usr/share/libdrm",
                Access::ReadOnly,
                None,
            ),
            (
                "/usr/share/drirc.d",
                "/usr/share/drirc.d",
                Access::ReadOnly,
                None,
            ),
            (
                "/dev/dri/renderD128",
                "/dev/dri/renderD128",
                Access::Device,
                None,
            ),
        ]);
    }
    expected.extend([
        ("/dev/null", "/dev/null", Access::Device, None),
        ("/dev/zero", "/dev/zero", Access::Device, None),
        ("/dev/full", "/dev/full", Access::Device, None),
        ("/dev/random", "/dev/random", Access::Device, None),
        ("/dev/urandom", "/dev/urandom", Access::Device, None),
        (
            "/data/llama/cpu/llama-server",
            "/opt/llama/llama-server",
            Access::ReadOnly,
            Some((7, 100)),
        ),
        (
            "/data/llama/cpu/libllama.so.0",
            "/opt/llama/libllama.so.0",
            Access::ReadOnly,
            Some((7, 101)),
        ),
        (
            "/data/models/m.gguf",
            "/models/model.gguf",
            Access::ReadOnly,
            Some((7, 200)),
        ),
        (
            "/data/mimik",
            "/opt/probe/mimik",
            Access::ReadOnly,
            Some((7, 400)),
        ),
        (
            "/data/run/3",
            "/run/llama",
            Access::ReadWrite,
            Some((7, 300)),
        ),
    ]);
    let actual: Vec<_> = plan
        .binds
        .iter()
        .map(|b| (b.source.as_str(), b.target.as_str(), b.access, b.identity))
        .collect();
    assert_eq!(actual, expected);
}

fn pin_bind_flags(plan: &Plan) {
    for bind in &plan.binds {
        let mut flags = match bind.access {
            Access::ReadOnlyDevice => Flags::BIND | Flags::NOSUID | Flags::RDONLY,
            Access::ReadOnly => Flags::BIND | Flags::NOSUID | Flags::NODEV | Flags::RDONLY,
            Access::ReadWrite => Flags::BIND | Flags::NOSUID | Flags::NODEV,
            Access::Device => Flags::BIND | Flags::NOSUID,
        };
        // Exactly master's source-mount inheritance, the only host-dependent flag.
        if rustix::fs::statvfs(&bind.source)
            .is_ok_and(|s| s.f_flag.contains(rustix::fs::StatVfsMountFlags::NOEXEC))
        {
            flags |= Flags::NOEXEC;
        }
        assert_eq!(options(bind), flags, "{}", bind.target);
    }
}

fn pin_tmpfs_and_files(plan: &Plan) {
    assert!(plan.proc);
    let tmpfs: Vec<_> = plan
        .tmpfs
        .iter()
        .map(|t| {
            (
                t.target.as_str(),
                t.bytes,
                t.mode,
                t.read_only,
                t.flags.noexec,
                t.flags.nosuid,
                t.flags.nodev,
            )
        })
        .collect();
    assert_eq!(
        tmpfs,
        [
            ("/run", 65_536, 0o755, false, true, true, true),
            ("/dev", 1_048_576, 0o755, false, true, true, true),
            ("/tmp", 67_108_864, 0o1777, false, true, true, true),
            ("/dev/shm", 268_435_456, 0o1777, false, true, true, true),
        ]
    );
    for tmpfs in &plan.tmpfs {
        assert_eq!(
            tmpfs_options(tmpfs),
            Flags::NOSUID | Flags::NODEV | Flags::NOEXEC
        );
    }
    assert_eq!(
        plan.files,
        [RunFile {
            path: "/run/token".into(),
            content: Secret::new("secret-token\n".into())
        }]
    );
}

fn pin_rules(plan: &Plan, gpu: Option<&GpuBinds>) {
    let mut rules = vec![
        ("/proc", Grant::Read),
        ("/dev", Grant::List),
        ("/run/token", Grant::ReadFile),
        ("/opt/llama", Grant::List),
    ];
    if gpu.is_some() {
        rules.extend([
            ("/sys/bus/pci", Grant::List),
            ("/sys/class", Grant::List),
            ("/sys/class/drm", Grant::List),
            ("/sys/dev", Grant::List),
            ("/sys/dev/char", Grant::List),
        ]);
    }
    rules.extend([
        ("/tmp", Grant::ReadWrite),
        ("/dev/shm", Grant::ReadWrite),
        ("/usr/lib", Grant::ReadExec),
        ("/usr/lib64", Grant::ReadExec),
        ("/usr/libexec/ld", Grant::ReadExec),
        ("/etc/ld.so.cache", Grant::Read),
        ("/sys/devices/system/cpu", Grant::Read),
    ]);
    if gpu.is_some() {
        rules.extend([
            ("/sys/devices/pci0000:00/0000:0c:00.0", Grant::Read),
            ("/usr/share/vulkan", Grant::Read),
            ("/etc/vulkan", Grant::Read),
            ("/usr/share/libdrm", Grant::Read),
            ("/usr/share/drirc.d", Grant::Read),
            ("/dev/dri", Grant::List),
            ("/dev/dri/renderD128", Grant::Device),
        ]);
    }
    rules.extend([
        ("/dev/null", Grant::Device),
        ("/dev/zero", Grant::Device),
        ("/dev/full", Grant::Device),
        ("/dev/random", Grant::Device),
        ("/dev/urandom", Grant::Device),
        ("/opt/llama/llama-server", Grant::ReadExec),
        ("/opt/llama/libllama.so.0", Grant::Read),
        ("/models/model.gguf", Grant::ReadFile),
        ("/opt/probe/mimik", Grant::Read),
        ("/run/llama", Grant::ReadWrite),
    ]);
    let actual_rules = landlock_rules(plan);
    assert_eq!(
        actual_rules
            .iter()
            .map(|r| (r.path.as_str(), r.grant))
            .collect::<Vec<_>>(),
        rules
    );
}

fn pin_links(plan: &Plan, gpu: Option<&GpuBinds>) {
    let mut links = vec![("usr/lib", "/lib"), ("usr/lib64", "/lib64")];
    if gpu.is_some() {
        links.extend([
            (
                "/sys/devices/pci0000:00/0000:0c:00.0/drm/renderD128",
                "/sys/dev/char/226:128",
            ),
            (
                "/sys/devices/pci0000:00/0000:0c:00.0/drm/renderD128",
                "/sys/class/drm/renderD128",
            ),
        ]);
    }
    links.extend([
        ("/proc/self/fd", "/dev/fd"),
        ("/proc/self/fd/0", "/dev/stdin"),
        ("/proc/self/fd/1", "/dev/stdout"),
        ("/proc/self/fd/2", "/dev/stderr"),
    ]);
    assert_eq!(
        plan.symlinks
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect::<Vec<_>>(),
        links
    );
}

#[test]
fn cpu_profile_matches_the_original_allowlist() {
    pin_profile(None);
}

#[test]
fn gpu_profile_matches_the_original_allowlist() {
    let gpu = GpuBinds {
        dev: "/dev/dri/renderD128".into(),
        name: "renderD128".into(),
        sysfs: "/sys/devices/pci0000:00/0000:0c:00.0".into(),
        rdev: (226, 128),
        shared: vec![
            "/usr/share/vulkan".into(),
            "/etc/vulkan".into(),
            "/usr/share/libdrm".into(),
            "/usr/share/drirc.d".into(),
        ],
    };
    pin_profile(Some(&gpu));
}

#[test]
fn nested_mounts_under_a_real_read_only_directory_have_exact_masks() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("read only");
    std::fs::create_dir(&source).unwrap();
    let mut given = inputs(None);
    given
        .extra
        .push((source.clone(), "/opt/data".into(), (7, 500)));
    let escaped = text(&source).replace(' ', "\\040");
    given.mountinfo = format!(
        "1 0 0:1 / {escaped}/z rw - tmpfs tmpfs rw\n\
         2 0 0:2 / {escaped}/a rw - tmpfs tmpfs rw\n\
         3 2 0:3 / {escaped}/a/child rw - tmpfs tmpfs rw\n\
         4 0 0:4 / {escaped}-sibling/no rw - tmpfs tmpfs rw\n"
    );
    let plan = profile(&given, system());
    assert_eq!(plan.validate(), Ok(()));
    assert_eq!(
        plan.tmpfs[4..],
        [
            Tmpfs {
                target: "/opt/data/a".into(),
                bytes: 4096,
                mode: 0o555,
                read_only: true,
                flags: TmpfsFlags::PRIVATE_DATA
            },
            Tmpfs {
                target: "/opt/data/z".into(),
                bytes: 4096,
                mode: 0o555,
                read_only: true,
                flags: TmpfsFlags::PRIVATE_DATA
            },
        ]
    );
}
