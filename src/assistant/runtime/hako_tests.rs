use super::*;
use crate::assistant::runtime::Backend;

fn inputs<'a>(gpu: Option<&'a GpuBinds>, files: &[&str]) -> Inputs<'a> {
    Inputs {
        root: PathBuf::from("/data/run/3.root"),
        runtime: files
            .iter()
            .enumerate()
            .map(|(n, name)| File {
                name: (*name).to_owned(),
                source: PathBuf::from("/data/llama/cpu").join(name),
                identity: (7, 100 + n as u64),
            })
            .collect(),
        model: (PathBuf::from("/data/models/m.gguf"), (7, 200)),
        generation: (PathBuf::from("/data/run/3"), (7, 300)),
        token: Secret::new("secret-token\n".into()),
        extra: Vec::new(),
        gpu,
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        argv: vec!["/opt/llama/llama-server".into(), "--help".into()],
    }
}

fn gpu() -> GpuBinds {
    GpuBinds {
        dev: PathBuf::from("/dev/dri/renderD128"),
        name: "renderD128".into(),
        sysfs: PathBuf::from("/sys/devices/pci0000:00/0000:0c:00.0"),
        rdev: (226, 128),
        shared: vec![PathBuf::from("/usr/share/vulkan")],
    }
}

fn targets(plan: &Plan) -> Vec<&str> {
    plan.binds.iter().map(|bind| bind.target.as_str()).collect()
}

#[test]
fn the_plan_binds_the_runtime_model_and_one_writable_directory_by_identity() {
    let plan = plan(&inputs(None, &["llama-server", "libllama.so.0"]));
    let bind = |target: &str| {
        plan.binds
            .iter()
            .find(|bind| bind.target == target)
            .unwrap_or_else(|| panic!("no bind at {target}"))
    };
    assert_eq!(bind("/opt/llama/llama-server").identity, Some((7, 100)));
    assert_eq!(bind("/opt/llama/libllama.so.0").identity, Some((7, 101)));
    assert_eq!(bind(bwrap::MODEL).identity, Some((7, 200)));
    assert_eq!(bind(bwrap::SOCKET_DIR).identity, Some((7, 300)));
    let writable: Vec<_> = plan
        .binds
        .iter()
        .filter(|bind| bind.access == Access::ReadWrite)
        .map(|bind| bind.target.as_str())
        .collect();
    assert_eq!(writable, [bwrap::SOCKET_DIR]);
    for bind in &plan.binds {
        if bind.access == Access::ReadOnly {
            continue;
        }
        assert!(
            bind.target == bwrap::SOCKET_DIR || bind.target.starts_with("/dev/"),
            "{bind:?}"
        );
    }
}

#[test]
fn the_minimal_dev_has_no_terminal_pts_or_ptmx() {
    let plan = plan(&inputs(Some(&gpu()), &[]));
    let mut dev: Vec<&str> = targets(&plan)
        .into_iter()
        .filter(|target| target.starts_with("/dev/"))
        .collect();
    dev.sort_unstable();
    assert_eq!(
        dev,
        [
            "/dev/dri/renderD128",
            "/dev/full",
            "/dev/null",
            "/dev/random",
            "/dev/urandom",
            "/dev/zero"
        ]
    );
    let everything: Vec<&str> = plan
        .symlinks
        .iter()
        .flat_map(|(original, link)| [original.as_str(), link.as_str()])
        .chain(plan.dirs.iter().map(String::as_str))
        .chain(targets(&plan))
        .collect();
    for forbidden in ["tty", "pts", "ptmx", "console", "kcore"] {
        assert!(
            !everything.iter().any(|path| path.contains(forbidden)),
            "{forbidden} is in the plan"
        );
    }
}

#[test]
fn only_the_render_node_is_a_device_beside_the_five_safe_ones() {
    let plan = plan(&inputs(Some(&gpu()), &[]));
    let devices: Vec<&str> = plan
        .binds
        .iter()
        .filter(|bind| bind.access == Access::Device)
        .map(|bind| bind.target.as_str())
        .collect();
    assert_eq!(devices.len(), DEVICES.len() + 1);
    assert!(devices.contains(&"/dev/dri/renderD128"));
    let cpu = super::plan(&inputs(None, &[]));
    assert_eq!(
        cpu.binds
            .iter()
            .filter(|bind| bind.access == Access::Device)
            .count(),
        DEVICES.len()
    );
}

#[test]
fn the_plan_has_the_same_system_allowlist_as_the_bwrap_profile() {
    let plan = plan(&inputs(Some(&gpu()), &[]));
    let mut items = bwrap::system_items();
    items.extend(gpu().items());
    for item in items {
        match item {
            Item::RoBind(path) => {
                let found = plan.binds.iter().any(|bind| {
                    bind.source == path.to_string_lossy()
                        && bind.target == path.to_string_lossy()
                        && bind.access == Access::ReadOnly
                });
                assert!(found, "{path:?}");
            }
            Item::Symlink { target, at } => assert!(
                plan.symlinks
                    .contains(&(target.to_string_lossy().into(), at.to_string_lossy().into())),
                "{at:?}"
            ),
            Item::Dir(path) => assert!(plan.dirs.contains(&path.to_string_lossy().into_owned())),
        }
    }
}

#[test]
fn every_symlink_has_its_parent_directories() {
    let plan = plan(&inputs(Some(&gpu()), &[]));
    for (_, link) in &plan.symlinks {
        for parent in parents(link) {
            assert!(plan.dirs.contains(&parent), "{link}: no {parent}");
        }
    }
    assert!(plan.dirs.contains(&"/sys/dev/char".to_owned()));
    assert!(plan.dirs.contains(&"/sys/class/drm".to_owned()));
}

#[test]
fn temporary_space_is_bounded_and_dev_is_a_small_tmpfs() {
    let plan = plan(&inputs(None, &[]));
    let tmpfs = |target: &str| plan.tmpfs.iter().find(|t| t.target == target).unwrap();
    assert_eq!(tmpfs("/tmp").bytes, bwrap::TMP_BYTES);
    assert_eq!(tmpfs("/dev/shm").bytes, bwrap::SHM_BYTES);
    assert!(tmpfs("/dev").bytes <= 1 << 20);
    assert_eq!(plan.tmpfs.len(), 4);
    assert_eq!(plan.workdir, bwrap::RUNTIME);
}

#[test]
fn the_probe_executable_is_an_extra_read_only_bind_with_its_identity() {
    let mut given = inputs(None, &[]);
    given.extra.push((
        PathBuf::from("/usr/bin/mimik"),
        "/opt/mimik/mimik".into(),
        (7, 9),
    ));
    let plan = plan(&given);
    let bind = plan
        .binds
        .iter()
        .find(|bind| bind.target == "/opt/mimik/mimik")
        .unwrap();
    assert_eq!(bind.access, Access::ReadOnly);
    assert_eq!(bind.identity, Some((7, 9)));
}

#[test]
fn a_bound_object_that_is_not_the_verified_one_is_refused() {
    let binds = plan(&inputs(None, &["llama-server"])).binds;
    let right = |target: &str| match target {
        "/opt/llama/llama-server" => Some((7, 100)),
        "/models/model.gguf" => Some((7, 200)),
        "/run/llama" => Some((7, 300)),
        _ => None,
    };
    assert!(identity_mismatch(&binds, right).is_none());
    // Another inode (a swapped file), another device, an unreadable target, each refuses.
    let swapped = |target: &str| {
        if target == "/models/model.gguf" {
            Some((7, 999))
        } else {
            right(target)
        }
    };
    assert_eq!(
        identity_mismatch(&binds, swapped).map(|bind| bind.target.as_str()),
        Some("/models/model.gguf")
    );
    let other_device = |target: &str| right(target).map(|(_, inode)| (8, inode));
    assert!(identity_mismatch(&binds, other_device).is_some());
    let unreadable = |target: &str| {
        if target == "/run/llama" {
            None
        } else {
            right(target)
        }
    };
    assert_eq!(
        identity_mismatch(&binds, unreadable).map(|bind| bind.target.as_str()),
        Some("/run/llama")
    );
}

#[test]
fn the_start_report_is_the_reapers_pid_without_a_namespace_number() {
    let line = started_line(4242, None, false);
    assert_eq!(line, "{\"child-pid\":4242}\n");
    let started = started(&line).unwrap();
    assert_eq!(started.child_pid, 4242);
    assert_eq!(
        started.pid_namespace,
        crate::assistant::runtime::endpoint::NAMESPACE_UNREPORTED
    );
    for bad in ["", "{}", "{\"child-pid\":\"x\"}", "not json"] {
        assert!(super::started(bad).is_none(), "{bad}");
    }
}

#[test]
fn closure_exit_codes_are_distinct_and_only_the_exec_is_after_the_marker() {
    let before = [
        exit::SESSION,
        exit::IDENTITY,
        exit::CAPABILITIES,
        exit::SECCOMP,
        exit::MARKER,
        exit::DESCRIPTORS,
        exit::SPEC,
    ];
    let mut all = before.to_vec();
    all.push(exit::EXEC);
    all.sort_unstable();
    all.dedup();
    assert_eq!(all.len(), before.len() + 1);
}

#[test]
fn the_hakoniwa_exec_travels_in_the_runner_spec() {
    use crate::assistant::runtime::runner::{Exec, Spec};
    let spec = Spec {
        parent: 1,
        keep: vec![4, 12],
        env: Vec::new(),
        chdir: None,
        exec: Exec::Hakoniwa {
            plan: Box::new(plan(&inputs(None, &["llama-server"]))),
            status: 4,
            marker: 12,
            ack: 13,
        },
    };
    let text = serde_json::to_string(&spec).unwrap();
    assert_eq!(serde_json::from_str::<Spec>(&text).unwrap(), spec);
}

#[test]
fn the_backend_names_parse_and_unknown_ones_are_refused() {
    assert_eq!(Backend::parse("bwrap"), Ok(Backend::Bwrap));
    assert_eq!(Backend::parse("bubblewrap"), Ok(Backend::Bwrap));
    assert!(Backend::parse("docker").unwrap_err().contains("docker"));
    assert_eq!(Backend::default(), Backend::Bwrap);
    #[cfg(feature = "sandbox-hakoniwa")]
    assert_eq!(Backend::parse("hakoniwa"), Ok(Backend::Hakoniwa));
    #[cfg(not(feature = "sandbox-hakoniwa"))]
    assert!(Backend::parse("hakoniwa").is_err());
}

#[cfg(feature = "sandbox-hakoniwa")]
#[test]
fn a_process_with_another_thread_is_not_single_threaded() {
    // The runner forks and then allocates: that is only sound alone, and it checks.
    let (stop, hold) = std::sync::mpsc::channel::<()>();
    let (ready_sender, ready) = std::sync::mpsc::channel::<()>();
    let other = std::thread::spawn(move || {
        ready_sender.send(()).unwrap();
        let _ = hold.recv();
    });
    ready.recv().unwrap();
    assert!(!single_threaded());
    stop.send(()).unwrap();
    other.join().unwrap();
}

#[cfg(feature = "sandbox-hakoniwa")]
#[test]
fn only_a_failure_of_hakoniwa_itself_is_a_setup_failure() {
    let status = |code, exit_code| hakoniwa::ExitStatus {
        code,
        reason: String::new(),
        exit_code,
        rusage: None,
        proc_pid_smaps_rollup: None,
        proc_pid_status: None,
    };
    // Hakoniwa's own failure: code 125 and no exit status of the program.
    assert!(setup_failed(&status(125, None)));
    // The program's exit, whatever its code (125 too), is its own; so is a death by signal.
    assert!(!setup_failed(&status(125, Some(125))));
    for code in [0, 1, 110, 116, 126, 127, 128, 137, 139] {
        assert!(!setup_failed(&status(code, Some(code))), "{code}");
        assert!(!setup_failed(&status(code, None)), "{code}");
    }
}

#[test]
fn the_root_is_the_generations_sibling_so_its_removal_is_the_generations() {
    let plan = plan(&inputs(None, &[]));
    assert_eq!(plan.root, "/data/run/3.root");
    // Not the temporary directory Hakoniwa would make (which a killed runner leaves behind).
    assert!(!plan.root.starts_with("/tmp"));
}

#[test]
fn the_token_is_in_the_plan_only_as_content_for_the_sandbox_own_tmpfs() {
    let plan = plan(&inputs(None, &[]));
    assert_eq!(plan.token, Secret::new("secret-token\n".into()));
    // Not a bind of a host file, and the /run it is written into is a bounded tmpfs.
    assert!(!targets(&plan).contains(&bwrap::TOKEN));
    let run = plan.tmpfs.iter().find(|t| t.target == "/run").unwrap();
    assert!(run.bytes <= 1 << 20);
    // It never prints, in debug output or anywhere a plan is formatted.
    assert!(!format!("{plan:?}").contains("secret-token"));
}

#[test]
fn the_apparmor_profile_is_made_for_the_actual_path() {
    let profile = apparmor_profile(Path::new("/opt/my apps/mimik")).unwrap();
    assert!(
        profile.contains("profile mimik \"/opt/my apps/mimik\" flags=(unconfined) {"),
        "{profile}"
    );
    assert!(profile.contains("\n  userns,\n"));
    assert!(
        !profile.contains(APPARMOR_PLACEHOLDER) || profile.contains("# This file is a template")
    );
    // The profile is the template, with the path the only thing put in.
    assert_eq!(
        profile.replace("/opt/my apps/mimik", APPARMOR_PLACEHOLDER),
        APPARMOR_TEMPLATE
    );
}

#[test]
fn a_path_apparmor_would_interpret_gets_no_profile() {
    for bad in [
        "mimik",
        "relative/mimik",
        "/opt/{a,b}/mimik",
        "/opt/*/mimik",
        "/opt/mi\"mik",
        "/opt/mi\nmik",
        "/opt/mi\\mik",
        "/opt/mi@{x}mik",
        "/opt/[a]/mimik",
    ] {
        assert!(apparmor_profile(Path::new(bad)).is_err(), "{bad}");
    }
}

#[test]
fn the_page_message_carries_the_generating_command_and_the_install_steps() {
    let message = apparmor_message(Path::new("/opt/mimik/bin/mimik"));
    // Generated to a file first and chained with `&&`: a failing generator installs nothing.
    assert!(message.contains(
        "/opt/mimik/bin/mimik assistant-apparmor-profile > mimik.apparmor && sudo install -m 0644 mimik.apparmor /etc/apparmor.d/mimik && sudo apparmor_parser -r /etc/apparmor.d/mimik"
    ));
    assert!(!message.contains("tee"));
    assert!(message.contains("may be why"));
    assert!(message.contains("Restart"));
    let spaced = apparmor_message(Path::new("/opt/it's here/mimik"));
    assert!(
        spaced.contains("'/opt/it'\\''s here/mimik' assistant-apparmor-profile >"),
        "{spaced}"
    );
}

#[test]
fn the_start_report_may_name_the_inner_process() {
    let line = started_line(4242, Some((4243, 99)), false);
    let started = started(&line).unwrap();
    assert_eq!((started.child_pid, started.inner_pid), (4242, Some(4243)));
    assert_eq!(started.inner_start, Some(99));
    assert!(super::started("{\"child-pid\":1,\"inner-pid\":\"x\"}").is_none());
}

/// Fake steps in the closure's order: each records that it ran; `fail` fails the one at that
/// index. Returns what ran and the outcome.
fn sequence(fail: Option<usize>) -> (Vec<&'static str>, Result<(), (i32, String)>) {
    let ran = std::cell::RefCell::new(Vec::new());
    let steps: Vec<Step<'_>> = STEP_ORDER
        .iter()
        .enumerate()
        .map(|(at, name)| {
            let ran = &ran;
            let step: Step<'_> = (
                100 + i32::try_from(at).unwrap(),
                Box::new(move || {
                    ran.borrow_mut().push(*name);
                    if fail == Some(at) {
                        Err(format!("{name} failed"))
                    } else {
                        Ok(())
                    }
                }),
            );
            step
        })
        .collect();
    let outcome = run_steps(steps);
    (ran.into_inner(), outcome)
}

#[test]
fn the_marker_is_the_last_step_and_a_failure_of_any_step_gives_none() {
    assert_eq!(STEP_ORDER.last(), Some(&"marker"));
    let (ran, outcome) = sequence(None);
    assert_eq!(outcome, Ok(()));
    assert_eq!(ran, STEP_ORDER);
    for at in 0..STEP_ORDER.len() {
        let (ran, outcome) = sequence(Some(at));
        let (code, _) = outcome.unwrap_err();
        assert_eq!(code, 100 + i32::try_from(at).unwrap());
        // Nothing after the failing step ran, and the marker only ever when it was reached
        // and succeeded (so, for a failure of the marker step itself, none was written).
        assert_eq!(ran.len(), at + 1, "{ran:?}");
        if at + 1 < STEP_ORDER.len() {
            assert!(!ran.contains(&"marker"), "step {at}: {ran:?}");
        }
    }
}

#[cfg(feature = "sandbox-hakoniwa")]
#[test]
fn the_real_closure_has_the_documented_steps_with_the_marker_last() {
    let plan = plan(&inputs(None, &[]));
    let steps = steps(&plan, rustix::fs::Mode::from_raw_mode(0o022));
    assert_eq!(steps.len(), STEP_ORDER.len());
    // The codes: every step before the marker is a sandbox failure; the marker's is its own.
    let codes: Vec<i32> = steps.iter().map(|(code, _)| *code).collect();
    assert_eq!(codes.last(), Some(&exit::MARKER));
    assert!(!codes.contains(&exit::EXEC));
}

#[cfg(feature = "sandbox-hakoniwa")]
#[test]
fn binds_are_not_recursive_so_no_submount_can_stay_writable_under_a_read_only_bind() {
    use hakoniwa::MountOptions;
    let bind = |access| Bind {
        source: "/usr/lib".into(),
        target: "/usr/lib".into(),
        access,
        identity: None,
    };
    for access in [Access::ReadOnly, Access::ReadWrite, Access::Device] {
        let options = options(&bind(access));
        assert!(!options.contains(MountOptions::REC), "{access:?}");
        assert!(options.contains(MountOptions::NOSUID), "{access:?}");
    }
    assert!(options(&bind(Access::ReadOnly)).contains(MountOptions::RDONLY));
    assert!(options(&bind(Access::ReadOnly)).contains(MountOptions::NODEV));
}

#[cfg(feature = "sandbox-hakoniwa")]
#[test]
fn a_runner_with_another_thread_refuses_before_it_takes_or_spawns_anything() {
    let (read, write) = rustix::pipe::pipe().unwrap();
    let (status_read, status_write) = rustix::pipe::pipe().unwrap();
    let (stop, hold) = std::sync::mpsc::channel::<()>();
    let (ready_sender, ready) = std::sync::mpsc::channel::<()>();
    let other = std::thread::spawn(move || {
        ready_sender.send(()).unwrap();
        let _ = hold.recv();
    });
    ready.recv().unwrap();
    let plan = plan(&inputs(None, &[]));
    let status = rustix::io::dup(&status_write).unwrap();
    let marker = rustix::io::dup(&write).unwrap();
    let (status_fd, marker_fd) = (
        std::os::fd::AsRawFd::as_raw_fd(&status),
        std::os::fd::AsRawFd::as_raw_fd(&marker),
    );
    let code = run_guarded(plan, status_fd, marker_fd, status_fd, true);
    assert_eq!(code, exit::SPEC);
    // The descriptors were never taken: they are still open here (a taken one would have been
    // closed with its owner), and nothing was written to either pipe or spawned.
    assert!(rustix::io::fcntl_getfd(&status).is_ok());
    assert!(rustix::io::fcntl_getfd(&marker).is_ok());
    drop((status, marker, write, status_write));
    let mut seen = Vec::new();
    std::io::Read::read_to_end(&mut std::fs::File::from(read), &mut seen).unwrap();
    std::io::Read::read_to_end(&mut std::fs::File::from(status_read), &mut seen).unwrap();
    assert_eq!(seen, Vec::<u8>::new());
    stop.send(()).unwrap();
    other.join().unwrap();
}

#[test]
fn a_replaced_executable_gets_no_profile() {
    let error = apparmor_profile(Path::new("/opt/mimik/mimik (deleted)")).unwrap_err();
    assert!(error.contains("replaced or removed"), "{error}");
}

#[test]
fn each_cause_gets_its_own_worded_as_possible_remedy() {
    let exe = Path::new("/opt/mimik/mimik");
    let apparmor = remedy(bwrap::Cause::AppArmor, exe);
    assert!(apparmor.contains("AppArmor") && apparmor.contains("may be why"));
    assert!(apparmor.contains("apparmor_parser"));
    let clone = remedy(bwrap::Cause::UsernsClone, exe);
    assert!(clone.contains("unprivileged_userns_clone") && clone.contains("may be why"));
    // The AppArmor steps are never offered for the other setting, and the reverse.
    assert!(!clone.contains("apparmor_parser"));
    assert!(!apparmor.contains("sysctl"));
}

#[cfg(feature = "sandbox-hakoniwa")]
#[test]
fn a_sandbox_that_ended_is_reported_only_when_the_program_ran() {
    let status = |code, exit_code| hakoniwa::ExitStatus {
        code,
        reason: "r".into(),
        exit_code,
        rusage: None,
        proc_pid_smaps_rollup: None,
        proc_pid_status: None,
    };
    let reported = std::cell::Cell::new(0);
    let report = || {
        reported.set(reported.get() + 1);
        Ok(())
    };
    // The program ran and ended in the window between the early wait and the child lookup:
    // reported (the sandbox was up), its own code returned.
    assert_eq!(after_end(Ok(status(3, Some(3))), report), 3);
    assert_eq!(reported.get(), 1);
    // Hakoniwa's own failure: not reported, its 125 returned.
    assert_eq!(after_end(Ok(status(125, None)), report), 125);
    assert_eq!(reported.get(), 1);
    // A program that exits 125 itself is its own exit, reported.
    assert_eq!(after_end(Ok(status(125, Some(125))), report), 125);
    assert_eq!(reported.get(), 2);
    // A report that cannot be written is a failure of the runner.
    let failing = || Err(std::io::Error::other("closed"));
    assert_eq!(after_end(Ok(status(0, Some(0))), failing), exit::SPEC);
}

#[test]
fn the_start_report_says_when_the_sandbox_had_ended_and_names_the_server_by_identity() {
    let line = started_line(10, Some((11, 12345)), false);
    let started = started(&line).unwrap();
    assert_eq!(
        (started.inner_pid, started.inner_start, started.ended),
        (Some(11), Some(12345), false)
    );
    let ended = super::started(&started_line(10, None, true)).unwrap();
    assert!(ended.ended && ended.inner_pid.is_none());
}
