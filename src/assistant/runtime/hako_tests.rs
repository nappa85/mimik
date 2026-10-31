use super::*;
use crate::sandbox::*;
use crate::sandbox::{process, seccomp};
use std::path::PathBuf;

#[test]
fn the_gpu_binds_take_only_the_vulkan_and_driver_data_that_exist() {
    let dir = tempfile::tempdir().unwrap();
    let existing = dir.path().join("vulkan");
    std::fs::create_dir(&existing).unwrap();
    let missing = dir.path().join("absent");
    assert_eq!(sandbox::shared_data(&[&existing, &missing]), [existing]);
}

fn inputs<'a>(gpu: Option<&'a GpuBinds>, files: &[&str]) -> Inputs<'a> {
    Inputs {
        root: PathBuf::from("/data/run/3.root"),
        mountinfo: String::new(),
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
    assert_eq!(bind(sandbox::MODEL).identity, Some((7, 200)));
    assert_eq!(bind(sandbox::SOCKET_DIR).identity, Some((7, 300)));
    let writable: Vec<_> = plan
        .binds
        .iter()
        .filter(|bind| bind.access == Access::ReadWrite)
        .map(|bind| bind.target.as_str())
        .collect();
    assert_eq!(writable, [sandbox::SOCKET_DIR]);
    for bind in &plan.binds {
        if bind.access == Access::ReadOnly {
            continue;
        }
        assert!(
            bind.target == sandbox::SOCKET_DIR || bind.target.starts_with("/dev/"),
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
fn the_plan_has_the_system_allowlist() {
    let plan = plan(&inputs(Some(&gpu()), &[]));
    let mut items = sandbox::system_items();
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
    assert_eq!(tmpfs("/tmp").bytes, sandbox::TMP_BYTES);
    assert_eq!(tmpfs("/dev/shm").bytes, sandbox::SHM_BYTES);
    assert!(tmpfs("/dev").bytes <= 1 << 20);
    assert_eq!(plan.tmpfs.len(), 4);
    assert_eq!(plan.workdir, sandbox::RUNTIME);
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
        assert!(
            crate::assistant::runtime::hako::started(bad).is_none(),
            "{bad}"
        );
    }
}

#[test]
fn closure_exit_codes_are_distinct_and_only_the_exec_is_after_the_marker() {
    let before = [
        exit::SESSION,
        exit::IDENTITY,
        exit::CAPABILITIES,
        exit::LANDLOCK,
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
    assert_eq!(plan.files[0].content, Secret::new("secret-token\n".into()));
    // Not a bind of a host file, and the /run it is written into is a bounded tmpfs.
    assert!(!targets(&plan).contains(&sandbox::TOKEN));
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
    assert!(
        crate::assistant::runtime::hako::started("{\"child-pid\":1,\"inner-pid\":\"x\"}").is_none()
    );
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
                name,
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

#[test]
fn the_real_closure_has_the_documented_steps_with_the_marker_last() {
    let plan = plan(&inputs(None, &[]));
    let steps = steps(&plan, rustix::fs::Mode::from_raw_mode(0o022));
    // The names are the production steps' own, so the order asserted is the order run.
    let names: Vec<&str> = steps.iter().map(|(name, _, _)| *name).collect();
    assert_eq!(names, STEP_ORDER);
    // The codes: every step before the marker is a sandbox failure; the marker's is its own.
    let codes: Vec<i32> = steps.iter().map(|(_, code, _)| *code).collect();
    assert_eq!(codes.last(), Some(&exit::MARKER));
    assert!(!codes.contains(&exit::EXEC));
}

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
    let apparmor = remedy(sandbox::Cause::AppArmor, exe);
    assert!(apparmor.contains("AppArmor") && apparmor.contains("may be why"));
    assert!(apparmor.contains("apparmor_parser"));
    let clone = remedy(sandbox::Cause::UsernsClone, exe);
    assert!(clone.contains("unprivileged_userns_clone") && clone.contains("may be why"));
    // The AppArmor steps are never offered for the other setting, and the reverse.
    assert!(!clone.contains("apparmor_parser"));
    assert!(!apparmor.contains("sysctl"));
}

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
    let ended = crate::assistant::runtime::hako::started(&started_line(10, None, true)).unwrap();
    assert!(ended.ended && ended.inner_pid.is_none());
}

const MOUNTINFO: &str = "\
36 1 8:2 / / rw,noatime - ext4 /dev/sda2 rw\n\
40 36 0:30 / /usr/lib/modules rw - ext4 /dev/sdb1 rw\n\
41 40 0:31 / /usr/lib/modules/inner rw - tmpfs tmpfs rw\n\
42 36 0:32 / /usr/lib/with\\040space rw - tmpfs tmpfs rw\n\
43 36 0:33 / /usr/lib64 rw - ext4 /dev/sdc1 rw\n\
44 36 0:34 / /usr/library rw - ext4 /dev/sdd1 rw\n\
45 36 0:35 / /usr/lib rw - ext4 /dev/sde1 rw\n";

#[test]
fn the_topmost_mounts_under_a_directory_are_found_by_their_decoded_mount_point() {
    assert_eq!(
        nested_mounts(MOUNTINFO, "/usr/lib"),
        ["/usr/lib/modules", "/usr/lib/with space"]
    );
    // A sibling that merely starts with the same letters, and the directory itself, are not
    // under it.
    assert_eq!(nested_mounts(MOUNTINFO, "/usr/lib64"), Vec::<String>::new());
    assert_eq!(
        nested_mounts(MOUNTINFO, "/usr/lib/modules"),
        ["/usr/lib/modules/inner"]
    );
    assert_eq!(nested_mounts("", "/usr/lib"), Vec::<String>::new());
}

/// A stand-in for Hakoniwa's reaper process: scripted answers, and a log of what was done to it.
struct FakeReaper {
    log: std::sync::Arc<std::sync::Mutex<Vec<(&'static str, std::time::Instant)>>>,
    wait: Option<hakoniwa::ExitStatus>,
    try_wait: Option<hakoniwa::ExitStatus>,
}

fn ran(code: i32) -> hakoniwa::ExitStatus {
    hakoniwa::ExitStatus {
        code,
        reason: String::new(),
        exit_code: Some(code),
        rusage: None,
        proc_pid_smaps_rollup: None,
        proc_pid_status: None,
    }
}

impl FakeReaper {
    fn new(wait: i32, try_wait: Option<i32>) -> Self {
        Self {
            log: std::sync::Arc::default(),
            wait: Some(ran(wait)),
            try_wait: try_wait.map(ran),
        }
    }
    fn note(&self, what: &'static str) {
        self.log
            .lock()
            .unwrap()
            .push((what, std::time::Instant::now()));
    }
    fn calls(&self) -> Vec<&'static str> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .map(|(what, _)| *what)
            .collect()
    }
}

impl Reaper for FakeReaper {
    fn wait(&mut self) -> Result<hakoniwa::ExitStatus, hakoniwa::Error> {
        self.note("wait");
        Ok(self.wait.clone().unwrap())
    }
    fn try_wait(&mut self) -> Result<Option<hakoniwa::ExitStatus>, hakoniwa::Error> {
        self.note("try_wait");
        Ok(self.try_wait.clone())
    }
    fn kill(&mut self) {
        self.note("kill");
    }
}

/// Scripted `/proc`: `(pid, parent, start)`, with the pids whose pidfd reads as exited after
/// the read, and the pids whose `stat` cannot be read.
#[derive(Default)]
struct FakeProcs {
    table: Vec<(i32, i32, u64)>,
    exited: Vec<i32>,
    unreadable: Vec<i32>,
    // Each pidfd is a real descriptor (a pipe end) keyed by pid.
    fds: std::sync::Mutex<Vec<(i32, i32)>>,
    /// `(pid, parent, start)` that `stat` answers once the pid has been opened: the process
    /// was replaced between the listing and the pinning.
    replaced_after_open: Vec<(i32, i32, u64)>,
    /// Pids whose pidfd reads as ended only once they have been read (they ended after it).
    ended_after_stat: Vec<i32>,
    /// Every call in order: `("open" | "stat" | "exited", pid)`.
    calls: std::sync::Mutex<Vec<(&'static str, i32)>>,
}

impl FakeProcs {
    fn done(&self, what: &'static str, pid: i32) -> bool {
        self.calls.lock().unwrap().contains(&(what, pid))
    }
    fn calls(&self) -> Vec<(&'static str, i32)> {
        self.calls.lock().unwrap().clone()
    }
}

impl Procs for FakeProcs {
    fn pids(&self) -> Vec<i32> {
        self.table.iter().map(|(pid, _, _)| *pid).collect()
    }
    fn open(&self, pid: i32) -> std::io::Result<std::os::fd::OwnedFd> {
        self.calls.lock().unwrap().push(("open", pid));
        let (read, _write) = rustix::pipe::pipe().unwrap();
        let raw = std::os::fd::AsRawFd::as_raw_fd(&read);
        self.fds.lock().unwrap().push((raw, pid));
        Ok(read)
    }
    fn stat(&self, pid: i32) -> std::io::Result<(i32, u64)> {
        let opened = self.done("open", pid);
        self.calls.lock().unwrap().push(("stat", pid));
        if let Some((_, parent, start)) = self
            .replaced_after_open
            .iter()
            .find(|(p, _, _)| opened && *p == pid)
        {
            return Ok((*parent, *start));
        }
        if self.unreadable.contains(&pid) {
            return Err(std::io::Error::from_raw_os_error(libc::EMFILE));
        }
        self.table
            .iter()
            .find(|(candidate, _, _)| *candidate == pid)
            .map(|(_, parent, start)| (*parent, *start))
            .ok_or_else(|| std::io::Error::from_raw_os_error(libc::ESRCH))
    }
    fn exited(&self, pidfd: &std::os::fd::OwnedFd) -> bool {
        let raw = std::os::fd::AsRawFd::as_raw_fd(pidfd);
        let pid = self
            .fds
            .lock()
            .unwrap()
            .iter()
            // The newest: a closed descriptor's number is handed out again.
            .rfind(|(fd, _)| *fd == raw)
            .map(|(_, pid)| *pid)
            .unwrap();
        let statted = self.done("stat", pid);
        self.calls.lock().unwrap().push(("exited", pid));
        self.exited.contains(&pid) || (statted && self.ended_after_stat.contains(&pid))
    }
}

#[test]
fn the_server_is_the_live_child_identified_from_one_read_of_its_stat() {
    let procs = FakeProcs {
        table: vec![(10, 1, 5), (11, 7, 6), (12, 7, 999)],
        exited: vec![12],
        ..FakeProcs::default()
    };
    // 11 is the parent's live child; 12 is its child too but its pidfd reads ended after the
    // read (it may be a reused pid by then): not the server; 10 is another's.
    assert_eq!(find_child(&procs, 7), Ok(Some((11, 6))));
    assert_eq!(find_child(&procs, 99), Ok(None));
}

#[test]
fn a_bystander_substituted_for_the_discovered_pid_is_never_the_server() {
    // The pid listed for the reaper's child is, by the time it is identified, another
    // process's (its parent is not the reaper): the one read of `stat` says so, and the
    // bystander is not returned, so never pinned or signalled.
    let bystander = FakeProcs {
        table: vec![(11, 4242, 777)],
        ..FakeProcs::default()
    };
    assert_eq!(find_child(&bystander, 7), Ok(None));
    // And one whose parent still reads as the reaper's but has ended since the pidfd was
    // opened (a reaped, reused pid): skipped.
    let reused = FakeProcs {
        table: vec![(11, 7, 777)],
        exited: vec![11],
        ..FakeProcs::default()
    };
    assert_eq!(find_child(&reused, 7), Ok(None));
    // A stat that cannot be read is no identity either.
    let unreadable = FakeProcs {
        table: vec![(11, 7, 777)],
        unreadable: vec![11],
        ..FakeProcs::default()
    };
    assert_eq!(find_child(&unreadable, 7), Ok(None));
    // Two live children: refused.
    let two = FakeProcs {
        table: vec![(11, 7, 1), (12, 7, 2)],
        ..FakeProcs::default()
    };
    assert_eq!(find_child(&two, 7), Err(()));
}

fn ack_pipe() -> (std::os::fd::OwnedFd, std::os::fd::OwnedFd) {
    rustix::pipe::pipe().unwrap()
}

fn procs_with_one_child() -> FakeProcs {
    FakeProcs {
        table: vec![(11, 7, 6)],
        ..FakeProcs::default()
    }
}

const SHORT: std::time::Duration = std::time::Duration::from_millis(200);

#[test]
fn a_reaper_that_ended_in_the_early_window_is_reaped_and_reported_as_ended() {
    let (ack, _keep) = ack_pipe();
    let mut reaper = FakeReaper::new(3, None);
    let mut status = Vec::new();
    let code = supervise(
        &mut reaper,
        7,
        true,
        &FakeProcs::default(),
        &mut status,
        &ack,
        SHORT,
    );
    assert_eq!(code, 3);
    assert_eq!(
        String::from_utf8(status).unwrap(),
        "{\"child-pid\":7,\"ended\":true}\n"
    );
    assert_eq!(reaper.calls(), ["wait"]);
}

#[test]
fn a_reaper_that_ended_between_the_early_poll_and_the_child_lookup_is_taken_as_ended() {
    let (ack, _keep) = ack_pipe();
    // No live child (the server ended), and the reaper has ended too: its status is taken.
    let mut reaper = FakeReaper::new(0, Some(3));
    let mut status = Vec::new();
    let code = supervise(
        &mut reaper,
        7,
        false,
        &FakeProcs::default(),
        &mut status,
        &ack,
        SHORT,
    );
    assert_eq!(code, 3);
    assert_eq!(
        String::from_utf8(status).unwrap(),
        "{\"child-pid\":7,\"ended\":true}\n"
    );
    assert_eq!(reaper.calls(), ["try_wait"]);
}

#[test]
fn a_live_reaper_with_no_child_is_killed_reaped_and_refused() {
    let (ack, _keep) = ack_pipe();
    let mut reaper = FakeReaper::new(0, None);
    let mut status = Vec::new();
    let code = supervise(
        &mut reaper,
        7,
        false,
        &FakeProcs::default(),
        &mut status,
        &ack,
        SHORT,
    );
    assert_eq!(code, exit::SPEC);
    assert!(status.is_empty(), "nothing is reported");
    let calls = reaper.calls();
    assert!(
        calls[..calls.len() - 2]
            .iter()
            .all(|call| *call == "try_wait")
    );
    assert_eq!(&calls[calls.len() - 2..], ["kill", "wait"]);
}

#[test]
fn a_reaper_finishing_after_its_child_disappears_keeps_the_payload_status() {
    struct Finishing(bool);
    impl Reaper for Finishing {
        fn wait(&mut self) -> Result<hakoniwa::ExitStatus, hakoniwa::Error> {
            panic!("must use bounded try_wait during the exit race");
        }
        fn try_wait(&mut self) -> Result<Option<hakoniwa::ExitStatus>, hakoniwa::Error> {
            Ok(std::mem::replace(&mut self.0, true).then(|| ran(20)))
        }
        fn kill(&mut self) {
            panic!("a naturally exiting reaper must not be killed");
        }
    }
    let (ack, _keep) = ack_pipe();
    let mut status = Vec::new();
    assert_eq!(
        supervise(
            &mut Finishing(false),
            7,
            false,
            &FakeProcs::default(),
            &mut status,
            &ack,
            SHORT
        ),
        20
    );
    assert_eq!(status, b"{\"child-pid\":7,\"ended\":true}\n");
}

#[test]
fn the_reaper_is_not_reaped_until_the_launcher_acknowledges() {
    let (ack, send) = ack_pipe();
    let mut reaper = FakeReaper::new(5, None);
    let log = std::sync::Arc::clone(&reaper.log);
    let acknowledged_at = std::sync::Arc::new(std::sync::Mutex::new(None));
    let writer = {
        let acknowledged_at = std::sync::Arc::clone(&acknowledged_at);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            *acknowledged_at.lock().unwrap() = Some(std::time::Instant::now());
            rustix::io::write(&send, b"1").unwrap();
        })
    };
    let mut status = Vec::new();
    let code = supervise(
        &mut reaper,
        7,
        false,
        &procs_with_one_child(),
        &mut status,
        &ack,
        std::time::Duration::from_secs(10),
    );
    writer.join().unwrap();
    assert_eq!(code, 5);
    assert_eq!(
        String::from_utf8(status).unwrap(),
        "{\"child-pid\":7,\"inner-pid\":11,\"inner-start\":6}\n"
    );
    let log = log.lock().unwrap();
    assert_eq!(
        log.iter().map(|(what, _)| *what).collect::<Vec<_>>(),
        ["wait"]
    );
    // The reaper was reaped only after the acknowledgement was sent.
    assert!(log[0].1 >= acknowledged_at.lock().unwrap().unwrap());
}

#[test]
fn a_launcher_that_closes_the_pipe_without_a_byte_gets_the_sandbox_ended() {
    let (ack, send) = ack_pipe();
    drop(send);
    let mut reaper = FakeReaper::new(0, None);
    let mut status = Vec::new();
    let code = supervise(
        &mut reaper,
        7,
        false,
        &procs_with_one_child(),
        &mut status,
        &ack,
        SHORT,
    );
    assert_eq!(code, exit::SPEC);
    assert_eq!(reaper.calls(), ["kill", "wait"]);
}

#[test]
fn no_acknowledgement_within_the_time_ends_the_sandbox_in_bounded_time() {
    let (ack, _send) = ack_pipe();
    let mut reaper = FakeReaper::new(0, None);
    let mut status = Vec::new();
    let started = std::time::Instant::now();
    let code = supervise(
        &mut reaper,
        7,
        false,
        &procs_with_one_child(),
        &mut status,
        &ack,
        SHORT,
    );
    assert_eq!(code, exit::SPEC);
    assert!(started.elapsed() >= SHORT && started.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(reaper.calls(), ["kill", "wait"]);
}

#[test]
fn more_than_one_process_in_the_sandbox_ends_it() {
    let (ack, _keep) = ack_pipe();
    let mut reaper = FakeReaper::new(0, None);
    let procs = FakeProcs {
        table: vec![(11, 7, 1), (12, 7, 2)],
        ..FakeProcs::default()
    };
    let mut status = Vec::new();
    let code = supervise(&mut reaper, 7, false, &procs, &mut status, &ack, SHORT);
    assert_eq!(code, exit::SPEC);
    assert_eq!(reaper.calls(), ["kill", "wait"]);
}

#[test]
fn the_stat_line_gives_the_parent_and_the_start_time_of_the_same_read() {
    let me = i32::try_from(std::process::id()).unwrap();
    let (parent, start) = process::stat_of(me).unwrap();
    assert_eq!(
        Some(start),
        crate::assistant::runtime::rundir::start_time(me)
    );
    assert_eq!(
        parent,
        rustix::process::getppid().unwrap().as_raw_nonzero().get()
    );
    // A name with spaces and parentheses does not shift the fields.
    let line = "42 (a) b (c) S 7 1 1 0 -1 4194560 0 0 0 0 0 0 0 0 20 0 1 0 987654 0 0 0";
    assert_eq!(process::parse_stat(line), Some((7, 987_654)));
    assert_eq!(process::parse_stat("garbage"), None);
    assert_eq!(process::parse_stat("1 (x) S"), None);
}

#[test]
fn a_mount_under_a_read_only_bound_directory_is_masked_whatever_the_host_has() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().to_str().unwrap().to_owned();
    let binds = vec![
        Bind {
            source: source.clone(),
            target: "/opt/lib".into(),
            access: Access::ReadOnly,
            identity: None,
        },
        // A writable bind is not masked: it is the server's own directory.
        Bind {
            source: source.clone(),
            target: "/run/llama".into(),
            access: Access::ReadWrite,
            identity: None,
        },
    ];
    let info = format!(
        "40 36 0:30 / {source}/modules rw - tmpfs t rw\n41 40 0:31 / {source}/modules/deep rw - tmpfs t rw\n"
    );
    let masks = masks(&binds, &info);
    assert_eq!(masks.len(), 1, "{masks:?}");
    assert_eq!(masks[0].target, "/opt/lib/modules");
    assert!(masks[0].read_only && masks[0].bytes <= 4096);
    // A source that is a file or does not exist is not masked.
    let file = Bind {
        source: format!("{source}/nothing"),
        target: "/opt/file".into(),
        access: Access::ReadOnly,
        identity: None,
    };
    assert_eq!(super::masks(&[file], &info), Vec::<Tmpfs>::new());
}

#[test]
fn the_child_is_pinned_then_read_once_then_checked_for_having_ended_in_that_order() {
    let procs = procs_with_one_child();
    assert_eq!(find_child(&procs, 7), Ok(Some((11, 6))));
    assert_eq!(procs.calls(), [("open", 11), ("stat", 11), ("exited", 11)]);
}

#[test]
fn a_process_replaced_after_it_was_pinned_is_not_the_server_and_one_ended_after_the_read_is_skipped()
 {
    // The pid listed as the reaper's child was another process's by the time its pidfd was
    // opened: `stat`, read after the pinning, says so (a `stat` read before it would have
    // seen the original and returned it).
    let replaced = FakeProcs {
        table: vec![(11, 7, 6)],
        replaced_after_open: vec![(11, 4242, 777)],
        ..FakeProcs::default()
    };
    assert_eq!(find_child(&replaced, 7), Ok(None));
    assert_eq!(replaced.calls()[..2], [("open", 11), ("stat", 11)]);
    // One that ended after the read (its pidfd then reads as ended, and the pid may be reused
    // by now) is checked after the read, not before it: skipped.
    let ended = FakeProcs {
        table: vec![(11, 7, 6)],
        ended_after_stat: vec![11],
        ..FakeProcs::default()
    };
    assert_eq!(find_child(&ended, 7), Ok(None));
    assert_eq!(ended.calls(), [("open", 11), ("stat", 11), ("exited", 11)]);
}

fn rule<'a>(rules: &'a [Rule], path: &str) -> Option<&'a Rule> {
    rules.iter().find(|rule| rule.path == path)
}

#[test]
fn the_landlock_rules_grant_what_the_plan_binds_and_nothing_more() {
    let render = gpu();
    let mut inputs = inputs(Some(&render), &["llama-server", "libllama.so.0"]);
    inputs.extra = vec![(
        PathBuf::from("/data/mimik"),
        "/opt/probe/mimik".to_owned(),
        (7, 400),
    )];
    let plan = plan(&inputs);
    let rules = landlock_rules(&plan);
    let grant = |path: &str| {
        rule(&rules, path)
            .unwrap_or_else(|| panic!("no rule for {path}: {rules:?}"))
            .grant
    };
    // Execute: the file the closure execs (the first argument) and the system library
    // directories (the kernel opens the ELF interpreter with the execute right).
    assert_eq!(grant("/opt/llama/llama-server"), Grant::ReadExec);
    for bind in &plan.binds {
        if sandbox::LIBRARY_DIRS.contains(&bind.target.as_str()) {
            assert_eq!(grant(&bind.target), Grant::ReadExec, "{bind:?}");
        }
    }
    // Read, never execute: the runtime's libraries (the loader maps them, which Landlock's
    // execute right does not govern), the probe's executable when it is not the program, the
    // GPU's system data and the model.
    for path in [
        "/opt/llama/libllama.so.0",
        "/opt/probe/mimik",
        "/usr/share/vulkan",
    ] {
        assert_eq!(grant(path), Grant::Read, "{path}");
    }
    assert_eq!(grant(sandbox::MODEL), Grant::ReadFile);
    // Read and write: the socket directory and the two writable tmpfs mounts; of `/run` only
    // the token file (read) and the socket directory, never the directory as a whole.
    for path in [sandbox::SOCKET_DIR, "/tmp", "/dev/shm"] {
        assert_eq!(grant(path), Grant::ReadWrite, "{path}");
    }
    assert_eq!(grant(sandbox::TOKEN), Grant::ReadFile);
    assert!(rule(&rules, "/run").is_none());
    // `/dev` is listed and `/proc` read; the five nodes and the render node are devices.
    assert_eq!(grant("/dev"), Grant::List);
    assert_eq!(grant("/proc"), Grant::Read);
    // The runtime's directory is listed (ggml looks for its backends there), not written.
    assert_eq!(grant(sandbox::RUNTIME), Grant::List);
    for device in DEVICES {
        assert_eq!(grant(&format!("/dev/{device}")), Grant::Device);
    }
    assert_eq!(grant("/dev/dri/renderD128"), Grant::Device);
    // Nothing else: no rule for a path the plan does not bind, and no read-write rule on a
    // read-only bind.
    for rule in &rules {
        let bound = plan.binds.iter().find(|bind| bind.target == rule.path);
        let fixed = [
            "/proc",
            "/dev",
            "/tmp",
            "/dev/shm",
            sandbox::TOKEN,
            sandbox::RUNTIME,
            "/dev/dri",
        ]
        .contains(&rule.path.as_str())
            || (rule.grant == Grant::List && plan.dirs.contains(&rule.path));
        assert!(bound.is_some() || fixed, "{rule:?} is not in the plan");
        if let Some(bind) = bound
            && bind.access == Access::ReadOnly
        {
            assert!(
                matches!(rule.grant, Grant::ReadExec | Grant::Read | Grant::ReadFile),
                "{rule:?}"
            );
        }
    }
    // `/home`, `/etc` and `/sys` have no rule of their own (only their binds do), and `/` has
    // none at all.
    assert!(rule(&rules, "/").is_none());
    assert!(rule(&rules, "/etc").is_none());
    assert!(rule(&rules, "/sys").is_none());
}

#[test]
fn the_landlock_report_travels_after_the_marker_and_only_as_a_report() {
    for landlock in [
        Landlock::Full { abi: 11 },
        Landlock::Partial { abi: 3 },
        Landlock::Unavailable,
    ] {
        let line = landlock.line();
        assert!(line.ends_with('\n'));
        assert!(line.len() < 64, "{line}: the marker pipe reads 128 bytes");
        assert_eq!(Landlock::parse(line.as_bytes()), Some(landlock));
    }
    for bad in [
        &b""[..],
        b"\n",
        b"{}\n",
        b"{\"landlock\":\"full\"}\n",
        b"{\"landlock\":\"strong\",\"abi\":3}\n",
        b"{\"landlock\":\"full\",\"abi\":999}\n",
        b"landlock full\n",
        // ABI 0 is no Landlock at all, never "full" or "partial".
        b"{\"landlock\":\"full\",\"abi\":0}\n",
        b"{\"landlock\":\"partial\",\"abi\":0}\n",
        // Unknown keys, and anything after the report's line.
        b"{\"landlock\":\"full\",\"abi\":6,\"extra\":1}\n",
        b"{\"landlock\":\"none\",\"abi\":6}\n",
        b"{\"landlock\":\"full\",\"abi\":6}\nmore",
        b"{\"landlock\":\"full\",\"abi\":6}\n{\"landlock\":\"none\"}\n",
        b"{\"landlock\":\"full\",\"abi\":6} \n",
        // The exact wire form only: no interior whitespace or newline, no `null` for a key
        // that must be absent (or present).
        b"{\"landlock\": \"full\",\"abi\":6}\n",
        b"{\"landlock\":\"full\",\n\"abi\":6}\n",
        b"{\"landlock\":\"none\",\"abi\":null}\n",
        b"{\"landlock\":\"full\",\"abi\":null}\n",
        b"{\"abi\":6,\"landlock\":\"full\"}\n",
        b"{\"landlock\":\"full\",\"abi\":6}",
    ] {
        assert_eq!(Landlock::parse(bad), None, "{bad:?}");
    }
    assert_eq!(
        Landlock::Full { abi: 11 }.describe(),
        "full for the ABI 6 rules (kernel ABI 11)"
    );
    assert_eq!(
        Landlock::Partial { abi: 3 }.describe(),
        "partial (kernel ABI 3)"
    );
    assert_eq!(Landlock::Unavailable.describe(), "not available");
}

#[test]
fn the_landlock_rights_never_let_written_files_run_or_make_device_nodes() {
    use landlock::AccessFs;
    let write = access(Grant::ReadWrite);
    for denied in [
        AccessFs::Execute,
        AccessFs::MakeChar,
        AccessFs::MakeBlock,
        AccessFs::Refer,
        AccessFs::IoctlDev,
    ] {
        assert!(!write.contains(denied), "{denied:?}");
    }
    assert!(write.contains(AccessFs::WriteFile | AccessFs::MakeSock | AccessFs::Truncate));
    for grant in [Grant::ReadFile, Grant::Read, Grant::List, Grant::ReadExec] {
        let rights = access(grant);
        assert!(!rights.contains(AccessFs::WriteFile), "{grant:?}");
        assert!(!rights.contains(AccessFs::MakeReg), "{grant:?}");
    }
    assert!(!access(Grant::ReadFile).contains(AccessFs::Execute));
    assert!(access(Grant::ReadExec).contains(AccessFs::Execute));
    // A device node: read, write and ioctl, nothing else (no execute, no create).
    let device = access(Grant::Device);
    assert!(device.contains(AccessFs::ReadFile | AccessFs::WriteFile | AccessFs::IoctlDev));
    assert!(!device.contains(AccessFs::Execute));
}

#[test]
fn an_enforced_ruleset_is_full_from_abi_6_and_partial_below() {
    assert_eq!(report_for(11), Landlock::Full { abi: 11 });
    assert_eq!(report_for(6), Landlock::Full { abi: 6 });
    assert_eq!(report_for(5), Landlock::Partial { abi: 5 });
    assert_eq!(report_for(1), Landlock::Partial { abi: 1 });
}

/// Runs `body` on a thread whose Landlock calls fail with `EPERM` (a seccomp filter on that
/// thread only), standing for a kernel whose later detection or enforcement fails after Mimik's
/// own probe has succeeded.
fn with_failing_landlock<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::spawn(move || {
        let errno = u32::try_from(libc::EPERM).unwrap();
        seccomp::install(&seccomp::program_without_landlock(errno)).unwrap();
        body()
    })
    .join()
    .unwrap()
}

#[test]
fn once_the_probe_succeeded_a_later_failure_is_never_taken_for_not_available() {
    let result = with_failing_landlock(|| apply_landlock_with(&[], || Ok(6)));
    let why = result.unwrap_err();
    assert!(why.starts_with("filesystem rights:"), "{why}");
}

fn missing_rule() -> (tempfile::TempDir, Vec<Rule>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("absent").to_str().unwrap().to_owned();
    (
        dir,
        vec![Rule {
            path,
            grant: Grant::Read,
        }],
    )
}

#[test]
fn a_path_error_is_found_before_the_kernel_is_asked_whatever_the_kernel() {
    let (_dir, rules) = missing_rule();
    let expected = rules[0].path.clone();
    // Landlock calls fail on this thread: only the path check can produce the error.
    let why = with_failing_landlock(move || apply_landlock_with(&rules, || Ok(6))).unwrap_err();
    assert!(why.starts_with(&expected), "{why}");
}

#[test]
fn an_unavailable_landlock_skips_the_paths_and_the_step_goes_on_to_the_marker() {
    let (_dir, rules) = missing_rule();
    assert_eq!(
        apply_landlock_with(&rules, || Err(libc::ENOSYS)),
        Ok(Landlock::Unavailable)
    );
    let found = std::rc::Rc::new(std::cell::Cell::new(None));
    let marked = std::cell::Cell::new(false);
    let step = landlock_step(rules.clone(), found.clone(), || Err(libc::ENOSYS));
    run_steps(vec![
        step,
        (
            "marker",
            exit::MARKER,
            Box::new(|| {
                marked.set(true);
                Ok(())
            }),
        ),
    ])
    .unwrap();
    assert!(marked.get());
    assert_eq!(found.get(), Some(Landlock::Unavailable));
}

#[test]
fn a_landlock_path_error_stops_the_real_step_before_the_marker() {
    let (_dir, rules) = missing_rule();
    let found = std::rc::Rc::new(std::cell::Cell::new(None));
    let marked = std::cell::Cell::new(false);
    // The probe is injected as successful, so the result does not depend on the host.
    let step = landlock_step(rules, found.clone(), || Ok(6));
    let result = run_steps(vec![
        step,
        (
            "marker",
            exit::MARKER,
            Box::new(|| {
                marked.set(true);
                Ok(())
            }),
        ),
    ]);
    let (code, why) = result.unwrap_err();
    assert_eq!(code, exit::LANDLOCK);
    assert!(why.starts_with("landlock:"), "{why}");
    assert!(!marked.get());
    assert_eq!(found.get(), None);
}

#[test]
fn the_landlock_probe_failing_as_not_available_continues_and_any_other_failure_stops() {
    for errno in [libc::ENOSYS, libc::EOPNOTSUPP, libc::EPERM] {
        assert_eq!(
            apply_landlock_with(&[], || Err(errno)),
            Ok(Landlock::Unavailable),
            "errno {errno}"
        );
    }
    for errno in [
        libc::EINVAL,
        libc::ENOMEM,
        libc::EACCES,
        libc::E2BIG,
        libc::EFAULT,
    ] {
        let why = apply_landlock_with(&[], || Err(errno)).unwrap_err();
        assert!(why.starts_with("probe:"), "{why}");
    }
}

#[test]
fn every_tmpfs_the_sandbox_mounts_is_noexec_nosuid_and_nodev() {
    use hakoniwa::MountOptions;
    let plan = plan(&inputs(None, &["llama-server"]));
    assert!(plan.tmpfs.len() >= 4);
    for tmpfs in &plan.tmpfs {
        let options = tmpfs_options(tmpfs);
        for flag in [
            MountOptions::NOEXEC,
            MountOptions::NOSUID,
            MountOptions::NODEV,
        ] {
            assert!(options.contains(flag), "{tmpfs:?} lacks {flag:?}");
        }
    }
}

/// What the kernel does with a rule on a directory: it covers the directories and files
/// beneath it and nothing beside it (the tree here is one mount; whether a rule crosses a
/// mount point is not shown). Applied to a thread of this test process (Landlock restricts the
/// calling thread). Rust has no runtime skip: on a kernel without Landlock the test prints
/// "SKIPPED" and proves nothing.
#[test]
fn a_landlock_list_rule_covers_the_directories_beneath_it_and_no_file_in_them() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("dev");
    std::fs::create_dir_all(root.join("dri")).unwrap();
    std::fs::write(root.join("dri/node"), b"x").unwrap();
    std::fs::create_dir_all(dir.path().join("other")).unwrap();
    let rules = vec![Rule {
        path: root.to_str().unwrap().to_owned(),
        grant: Grant::List,
    }];
    let result = std::thread::spawn({
        let (root, other) = (root.clone(), dir.path().join("other"));
        move || {
            if !matches!(
                apply_landlock(&rules),
                Ok(Landlock::Full { .. } | Landlock::Partial { .. })
            ) {
                return None;
            }
            Some((
                std::fs::read_dir(root.join("dri")).is_ok(),
                std::fs::read_dir(other).is_ok(),
                std::fs::read(root.join("dri/node")).is_ok(),
            ))
        }
    })
    .join()
    .unwrap();
    let Some((listed, other, file)) = result else {
        // A run that demands Landlock (`MIMIK_LIVE_LANDLOCK=require`, CI on a capable runner)
        // fails instead of skipping.
        assert!(
            std::env::var("MIMIK_LIVE_LANDLOCK").as_deref() != Ok("require"),
            "this run requires Landlock, and the kernel has none"
        );
        eprintln!("SKIPPED: this kernel has no Landlock; nothing was checked");
        return;
    };
    assert!(listed, "a list rule on /dev must list /dev/dri");
    assert!(!other);
    assert!(!file);
}

#[test]
fn the_apparmor_profile_command_takes_a_path_for_packagers_and_defaults_to_this_binary() {
    let profile = apparmor_profile_for(&["--for".into(), "/opt/mimik/bin/mimik".into()]).unwrap();
    assert!(profile.contains("/opt/mimik/bin/mimik"), "{profile}");
    assert!(!profile.contains(APPARMOR_PLACEHOLDER));
    let own = apparmor_profile_for(&[]).unwrap();
    let exe = std::env::current_exe().unwrap();
    assert!(own.contains(exe.to_str().unwrap()), "{own}");
    // A relative path, a path AppArmor would interpret, a missing value and anything else.
    for bad in [
        vec!["--for".to_owned(), "mimik".to_owned()],
        vec!["--for".to_owned(), "/opt/{a,b}/mimik".to_owned()],
        vec!["--for".to_owned()],
        vec!["--other".to_owned()],
        vec!["--for".to_owned(), "/a".to_owned(), "/b".to_owned()],
    ] {
        assert!(apparmor_profile_for(&bad).is_err(), "{bad:?}");
    }
}
