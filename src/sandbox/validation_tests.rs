use super::*;

fn profile() -> Profile {
    Profile {
        root: "/sandbox".into(),
        files: vec![],
        proc: false,
        rules: vec![],
        binds: vec![],
        tmpfs: vec![Tmpfs {
            target: "/run".into(),
            bytes: 4096,
            mode: 0o700,
            read_only: false,
            flags: TmpfsFlags::PRIVATE_DATA,
        }],
        symlinks: vec![],
        dirs: vec![],
        workdir: "/".into(),
        argv: vec!["/worker".into()],
        env: vec![],
    }
}

#[test]
fn refuses_non_absolute_or_unnormalised_paths() {
    for path in ["", "relative", "/a/./b", "/a/../b", "/a//b", "//a", "/a/"] {
        for field in 0..5 {
            let mut p = profile();
            match field {
                0 => p.rules.push(Rule {
                    path: path.into(),
                    grant: Grant::Read,
                }),
                1 | 2 => p.binds.push(Bind {
                    source: if field == 1 { path } else { "/source" }.into(),
                    target: if field == 2 { path } else { "/target" }.into(),
                    access: Access::ReadOnly,
                    identity: None,
                }),
                3 => p.tmpfs[0].target = path.into(),
                _ => p.files.push(RunFile {
                    path: path.into(),
                    content: Secret::new(String::new()),
                }),
            }
            assert!(p.validate().is_err(), "field {field}: {path:?}");
        }
    }
}

#[test]
fn refuses_explicit_proc_rules_even_when_proc_is_enabled() {
    for proc in [false, true] {
        for path in ["/proc", "/proc/self/fd"] {
            let mut p = profile();
            p.proc = proc;
            p.rules.push(Rule {
                path: path.into(),
                grant: Grant::Read,
            });
            assert!(p.validate().is_err(), "{proc}: {path}");
        }
    }
}

#[test]
fn refuses_writable_tmpfs_without_each_private_data_flag() {
    for missing in 0..3 {
        let mut p = profile();
        match missing {
            0 => p.tmpfs[0].flags.noexec = false,
            1 => p.tmpfs[0].flags.nosuid = false,
            _ => p.tmpfs[0].flags.nodev = false,
        }
        assert!(p.validate().is_err(), "flag {missing}");
    }
}

#[test]
fn refuses_execute_except_on_a_read_only_bind() {
    for access in [None, Some(Access::ReadWrite), Some(Access::Device)] {
        let mut p = profile();
        p.rules.push(Rule {
            path: "/worker".into(),
            grant: Grant::ReadExec,
        });
        if let Some(access) = access {
            p.binds.push(Bind {
                source: "/source".into(),
                target: "/worker".into(),
                access,
                identity: None,
            });
        }
        assert!(p.validate().is_err(), "{access:?}");
    }
}

#[test]
fn refuses_run_files_outside_writable_tmpfs() {
    for (path, read_only) in [
        ("/token", false),
        ("/runtime/token", false),
        ("/run", false),
        ("/run/token", true),
    ] {
        let mut p = profile();
        p.tmpfs[0].read_only = read_only;
        p.files.push(RunFile {
            path: path.into(),
            content: Secret::new(String::new()),
        });
        assert!(p.validate().is_err(), "{path}, read_only={read_only}");
    }
}
