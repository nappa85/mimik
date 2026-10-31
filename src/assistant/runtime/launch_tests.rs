//! The real launcher at level 3 against a fake llama-server (`examples/fake_llama_server.rs`,
//! built first with `cargo build --locked --example fake_llama_server`), the test binary as
//! the runner and as the probe.
//! Level 1 needs the sandbox and the pinned build and runs in `live_tests.rs`.
use super::super::drain::Failed;
use super::super::manager::{self, State};
use super::super::manifest::File as Entry;
use super::super::test_helper;
use super::*;
use crate::assistant::jobs::Assistant;
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;

const HELPER: &str = "assistant::runtime::launch::tests::helper";
const RUNNER: &str = "sandbox::runner::tests::helper";

/// The probe inside its level: the test binary, its level arguments after `probe-inner`.
#[test]
fn helper() {
    match test_helper::mode().as_deref() {
        Some("probe-inner") => {}
        _ => return,
    }
    let args: Vec<String> = std::env::args().collect();
    let at = args
        .iter()
        .position(|arg| arg == "probe-inner")
        .expect("marker");
    std::process::exit(super::super::probe::inner(&args[at + 1..]));
}

/// A scratch folder whose ancestors are not writable by others (the level-3 rule).
fn scratch() -> tempfile::TempDir {
    let base = test_helper::scratch_base();
    std::fs::create_dir_all(&base).unwrap();
    std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o755)).unwrap();
    tempfile::Builder::new().tempdir_in(base).unwrap()
}

fn fake_server() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let path = exe
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .join("examples/fake_llama_server");
    assert!(
        path.is_file(),
        "{} is missing: run `cargo build --locked --example fake_llama_server` with the same target directory and profile before the tests",
        path.display()
    );
    path
}

#[test]
fn the_fake_server_is_a_runnable_example_not_a_test_harness() {
    let output = std::process::Command::new(fake_server())
        .arg("--list-devices")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .starts_with("Available devices:\n")
    );
}

fn helper_args(test: &str) -> Vec<String> {
    [
        test,
        "--exact",
        "--nocapture",
        "--test-threads=1",
        "--include-ignored",
    ]
    .map(String::from)
    .to_vec()
}

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

struct Setup {
    dir: tempfile::TempDir,
    settings: Settings,
    build: Build,
}

fn setup(model: &[u8], alias: &str) -> Setup {
    setup_with_server(model, alias, &std::fs::read(fake_server()).unwrap())
}

/// [`setup`] with the bytes of the runtime's `llama-server` given.
fn setup_with_server(model: &[u8], alias: &str, server: &[u8]) -> Setup {
    let dir = scratch();
    let root = dir.path();
    let cpu = root.join("runtime/cpu");
    std::fs::create_dir_all(&cpu).unwrap();
    std::fs::write(cpu.join("llama-server"), server).unwrap();
    std::fs::set_permissions(
        cpu.join("llama-server"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::fs::write(cpu.join("LICENSE"), b"MIT").unwrap();
    std::fs::set_permissions(cpu.join("LICENSE"), std::fs::Permissions::from_mode(0o644)).unwrap();
    let build = Build {
        path: "cpu".into(),
        directory: "cpu".into(),
        server: "llama-server".into(),
        files: vec![
            Entry {
                name: "llama-server".into(),
                sha256: sha(server),
                size: server.len() as u64,
                mode: "0755".into(),
            },
            Entry {
                name: "LICENSE".into(),
                sha256: sha(b"MIT"),
                size: 3,
                mode: "0644".into(),
            },
        ],
    };
    let models = root.join("models");
    std::fs::create_dir(&models).unwrap();
    std::fs::set_permissions(&models, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(models.join("m.gguf"), model).unwrap();
    let exe = std::env::current_exe().unwrap();
    let mut inner_args = helper_args(HELPER);
    inner_args.push("probe-inner".into());
    let settings = Settings {
        data_dir: root.join("data"),
        models_dir: models,
        runtime_dir: root.join("runtime"),
        model: "m.gguf".into(),
        model_sha256: sha(model),
        alias: alias.into(),
        threads: 1,
        ctx_size: 256,
        runner: RunnerCommand {
            program: exe.clone(),
            args: helper_args(RUNNER).into_iter().map(Into::into).collect(),
            env: vec![(test_helper::VARIABLE.into(), "runner".into())],
        },
        inner: InnerCommand {
            executable: exe,
            probe_args: inner_args,
            probe_env: vec![(test_helper::VARIABLE.into(), "probe-inner".into())],
        },
        device: device::Choice::Cpu,
        roots: device::Roots::default(),
    };
    Setup {
        dir,
        settings,
        build,
    }
}

/// The launcher over the fake build, level 3 allowed (its switched-off code is tested here).
fn launch(setup: &Setup) -> Launch {
    Launch(Arc::new(
        Real::with_build(setup.settings.clone(), Some(setup.build.clone())).allowing_unconfined(),
    ))
}

const MODEL: &[u8] = b"GGUF a synthetic model";

#[tokio::test]
async fn the_manager_probe_runs_level_1_only_and_the_cli_still_probes_level_3() {
    let setup = setup(MODEL, "fake");
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    // The manager's probe never runs level 3's code (ASSIST-43).
    let levels = launch.probe(Path::Cpu).await;
    assert_eq!(
        levels.len(),
        2,
        "there is no level 2 (ASSIST-48): {levels:?}"
    );
    assert_eq!(
        levels[1],
        (
            Level::Unconfined,
            Err("not used in this release (ASSIST-43)".into())
        ),
        "{levels:?}"
    );
    // `--probe` (probe_levels) still probes level 3, so its code keeps working.
    let real = Arc::clone(&launch.0);
    let all = tokio::task::spawn_blocking(move || {
        let ticket = real.work().enter();
        real.probe_levels(Path::Cpu, &ticket)
    })
    .await
    .unwrap();
    assert_eq!(all[1], (Level::Unconfined, Probed::Passed), "{all:?}");
    launch.settle().await;
    // The probes' directories are gone.
    let run: Vec<_> = std::fs::read_dir(setup.settings.data_dir.join("run"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(run, [rundir::PIDFILE]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_usable_sandbox_the_real_manager_is_unavailable_and_starts_nothing() {
    let setup = setup(MODEL, "fake");
    // The sandbox is unusable for this launcher, wherever the test runs.
    NO_SANDBOX
        .lock()
        .unwrap()
        .push(setup.settings.runtime_dir.clone());
    let real = Arc::new(Real::with_build(
        setup.settings.clone(),
        Some(setup.build.clone()),
    ));
    let assistant = Assistant::new();
    let handle = manager::spawn(
        Arc::clone(&assistant),
        Arc::new(Launch(Arc::clone(&real))),
        manager::Config {
            path: Path::Cpu,
            levels: manager::USED_LEVELS,
        },
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let why = loop {
        if let State::Unavailable(why) = handle.status().state {
            break why;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{:?}",
            handle.status()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    // Level 3 is never used (ASSIST-43): nothing was started, and the page says how to fix it.
    assert!(why.starts_with(manager::LEVEL_1_NEEDED), "{why}");
    let status = handle.status();
    assert_eq!(status.level, None);
    assert!(
        status
            .skipped
            .iter()
            .any(|(level, why)| *level == Level::Unconfined && why.contains("ASSIST-43")),
        "{status:?}"
    );
    let run = setup.settings.data_dir.join("run");
    assert!(!run.join("1").exists());
    assert_eq!(
        std::fs::read_to_string(run.join(rundir::PIDFILE)).unwrap(),
        ""
    );
    handle.shutdown().await;
}

#[tokio::test]
async fn changed_or_extra_runtime_files_refuse_the_start() {
    let setup = setup(MODEL, "fake");
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    std::fs::write(setup.dir.path().join("runtime/cpu/extra.so"), b"x").unwrap();
    let Err(Fail::Other(why)) = launch.start(1, Level::Unconfined, Path::Cpu).await else {
        panic!("an extra file must refuse the start");
    };
    assert_eq!(
        why,
        "the model runtime: extra.so is not part of the pinned build"
    );
    std::fs::remove_file(setup.dir.path().join("runtime/cpu/extra.so")).unwrap();
    std::fs::write(setup.dir.path().join("runtime/cpu/LICENSE"), b"BSD").unwrap();
    let Err(Fail::Other(why)) = launch.start(2, Level::Unconfined, Path::Cpu).await else {
        panic!("a changed file must refuse the start");
    };
    assert_eq!(
        why,
        "the model runtime: LICENSE differs from the pinned file"
    );
}

#[tokio::test]
async fn a_group_writable_runtime_folder_is_refused_with_the_exact_fix() {
    use std::os::unix::fs::PermissionsExt;
    let setup = setup(MODEL, "fake");
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    // What Ubuntu's umask 002 gives a folder made with `mkdir -p`.
    let runtime = setup.dir.path().join("runtime");
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o775)).unwrap();
    let Err(Fail::Other(why)) = launch.start(1, Level::Unconfined, Path::Cpu).await else {
        panic!("a group-writable runtime folder must refuse the start");
    };
    assert!(why.starts_with("the model runtime: "), "{why}");
    assert!(why.contains("writable by group or others"), "{why}");
    // The command that fixes it, with the folder's own path.
    let fix = format!("chmod -R go-w {}", runtime.display());
    assert!(why.contains(&fix), "{why}");
    // Never relaxed: the same folder with the fix applied starts.
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!matches!(
        launch.start(2, Level::Unconfined, Path::Cpu).await,
        Err(Fail::Other(ref why)) if why.contains("writable")
    ));
}

#[tokio::test]
async fn a_library_the_check_believes_missing_never_refuses_the_start_and_leaves_a_hint() {
    // The server (the fake) really runs; its ELF is replaced by one that names a library
    // nobody has, so the advisory check believes it is missing. The start is still attempted
    // (here the stand-in is not an executable server, so it fails after the attempt), the
    // hint is remembered for the failure, and the verification itself no longer refuses.
    let server = super::super::libs::tests::elf_needing(&["libgomp-not-installed.so.1"]);
    let setup = setup_with_server(MODEL, "fake", &server);
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    let result = launch.start(1, Level::Unconfined, Path::Cpu).await;
    if let Err(Fail::Other(why) | Fail::Sandbox(why)) = &result {
        assert!(
            !why.contains("may need"),
            "the check refused the start: {why}"
        );
    }
    let hint = Launcher::failure_hint(&launch, Path::Cpu).unwrap();
    assert!(
        hint.starts_with("llama-server may need libgomp-not-installed.so.1: "),
        "{hint}"
    );
    // Nothing for the other path.
    assert_eq!(Launcher::failure_hint(&launch, Path::Gpu), None);
}

#[tokio::test]
async fn a_backend_loaded_with_dlopen_is_inspected_and_leaves_a_hint() {
    // The server (the fake) is fine; a CPU backend variant of the build, which no `DT_NEEDED`
    // of the server names, needs a library nobody has.
    let mut setup = setup(MODEL, "fake");
    let backend = super::super::libs::tests::elf_needing(&["libgomp-not-installed.so.1"]);
    let path = setup.dir.path().join("runtime/cpu/libggml-cpu-haswell.so");
    std::fs::write(&path, &backend).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    setup.build.files.push(Entry {
        name: "libggml-cpu-haswell.so".into(),
        sha256: sha(&backend),
        size: backend.len() as u64,
        mode: "0755".into(),
    });
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    // The start is attempted and not refused for it.
    let started = launch.start(1, Level::Unconfined, Path::Cpu).await;
    if let Err(Fail::Other(why) | Fail::Sandbox(why)) = &started {
        assert!(!why.contains("may need"), "{why}");
    }
    let hint = Launcher::failure_hint(&launch, Path::Cpu).unwrap();
    assert!(
        hint.starts_with("llama-server may need libgomp-not-installed.so.1: "),
        "{hint}"
    );
}

#[tokio::test]
async fn the_models_folder_gets_the_chmod_700_advice_and_the_runtime_folder_does_not() {
    use std::os::unix::fs::PermissionsExt;
    let setup = setup(MODEL, "fake");
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    std::fs::set_permissions(
        &setup.settings.models_dir,
        std::fs::Permissions::from_mode(0o770),
    )
    .unwrap();
    let Err(Fail::Other(why)) = launch.start(1, Level::Unconfined, Path::Cpu).await else {
        panic!("a models folder that is not 0700 must refuse the start");
    };
    assert!(why.starts_with("the models folder: "), "{why}");
    assert!(
        why.contains(&format!(
            "chmod 700 {}",
            setup.settings.models_dir.display()
        )),
        "{why}"
    );
    assert!(!why.contains("go-w"), "{why}");
    // The runtime folder's refusal names `go-w`, never `chmod 700`.
    std::fs::set_permissions(
        &setup.settings.models_dir,
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    std::fs::set_permissions(
        &setup.settings.runtime_dir,
        std::fs::Permissions::from_mode(0o775),
    )
    .unwrap();
    let Err(Fail::Other(why)) = launch.start(2, Level::Unconfined, Path::Cpu).await else {
        panic!("a group-writable runtime must refuse the start");
    };
    assert!(why.contains("go-w") && !why.contains("chmod 700"), "{why}");
}

#[tokio::test]
async fn a_model_that_does_not_load_is_a_counted_exit() {
    let setup = setup(b"not a gguf model", "fake");
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    let child = launch.start(1, Level::Unconfined, Path::Cpu).await.unwrap();
    let fail = tokio::time::timeout(Duration::from_secs(20), child.exited())
        .await
        .unwrap();
    assert!(
        matches!(&fail, Fail::Other(why) if why.starts_with("the model could not be loaded")),
        "{fail:?}"
    );
    child.stop().await;
}

#[tokio::test]
async fn a_port_that_cannot_be_bound_is_tried_three_times_then_fails() {
    // The server exits reporting a bind failure (even if the drain has not parsed it when the
    // exit is seen); after three ports the start fails with a definite error, never a dead
    // child reported as bound.
    let setup = setup(MODEL, "fail-bind");
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    // Three known free ports for this launcher: all three must be consumed.
    let mut holders = Vec::new();
    for _ in 0..PORT_TRIES {
        let holder = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = holder.local_addr().unwrap().port();
        PORTS
            .lock()
            .unwrap()
            .push((setup.settings.runtime_dir.clone(), port));
        holders.push((holder, port));
    }
    drop(holders);
    // Each dead child's drain settlement takes longer than the short listening deadline: the
    // classification must still happen (outside the deadline), never cancelled into "bound".
    BIND_WITHIN.lock().unwrap().insert(
        setup.settings.runtime_dir.clone(),
        Duration::from_millis(200),
    );
    DRAIN_HOLD.lock().unwrap().insert(
        setup.settings.runtime_dir.clone(),
        Duration::from_millis(400),
    );
    let result = launch.start(1, Level::Unconfined, Path::Cpu).await;
    BIND_WITHIN
        .lock()
        .unwrap()
        .remove(&setup.settings.runtime_dir);
    DRAIN_HOLD
        .lock()
        .unwrap()
        .remove(&setup.settings.runtime_dir);
    assert!(
        matches!(&result, Err(Fail::Other(why)) if why.contains("could not bind a loopback port")),
        "{:?}",
        result.err()
    );
    // All three were taken (none of this launcher's ports is left).
    assert!(
        !PORTS
            .lock()
            .unwrap()
            .iter()
            .any(|(dir, _)| *dir == setup.settings.runtime_dir)
    );
    launch.settle().await;
}

#[tokio::test]
async fn a_second_mimik_on_the_same_data_directory_cannot_run_the_assistant() {
    let setup = setup(MODEL, "fake");
    let first = launch(&setup);
    first.preflight(Path::Cpu).await.unwrap();
    let second = launch(&setup);
    assert_eq!(
        second.preflight(Path::Cpu).await,
        Err("another Mimik uses this data directory's assistant".into())
    );
}

#[tokio::test]
async fn a_model_larger_than_available_memory_is_refused() {
    let setup = setup(MODEL, "fake");
    let available = mem_available().unwrap();
    // A sparse file: nothing is written or hashed.
    let big = std::fs::OpenOptions::new()
        .write(true)
        .open(setup.settings.models_dir.join("m.gguf"))
        .unwrap();
    big.set_len(available).unwrap();
    let launch = launch(&setup);
    let refused = launch.preflight(Path::Cpu).await.unwrap_err();
    assert!(
        refused.starts_with("not enough memory for this model"),
        "{refused}"
    );
}

#[test]
fn only_a_missing_marker_makes_a_level_1_exit_a_sandbox_failure() {
    let loaded = Summary {
        failed: Failed {
            model_load: true,
            ..Failed::default()
        },
        ..Summary::default()
    };
    // The closure never reached its exec: the sandbox was not set up.
    assert!(matches!(
        classify(
            Level::Sandbox,
            Marker::Absent,
            Summary::default(),
            "exit status: 1"
        ),
        Fail::Sandbox(_)
    ));
    // After the marker, whatever the server printed (a forged "mimik sandbox:" line included, which
    // the drain no longer even recognises) it is a counted failure that keeps the level.
    assert!(matches!(
        classify(
            Level::Sandbox,
            Marker::Executed,
            Summary::default(),
            "exit status: 1"
        ),
        Fail::Other(_)
    ));
    assert!(matches!(
        classify(
            Level::Sandbox,
            Marker::Executed,
            loaded,
            "exit status: 1"
        ),
        Fail::Other(why) if why.starts_with("the model could not be loaded")
    ));
    // Not known (a read error, a reader still running): counted, the level kept.
    assert!(matches!(
        classify(
            Level::Sandbox,
            Marker::Unknown,
            Summary::default(),
            "exit status: 1"
        ),
        Fail::Other(_)
    ));
    // Level 3 has no lower level: never a sandbox failure.
    assert!(matches!(
        classify(
            Level::Unconfined,
            Marker::Absent,
            Summary::default(),
            "exit status: 1"
        ),
        Fail::Other(_)
    ));
}

struct Failing;

impl Read for Failing {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::from_raw_os_error(libc::EIO))
    }
}

#[test]
fn only_a_clean_end_without_the_marker_is_absent() {
    assert_eq!(marker_outcome(&b"exec\n"[..]), Marker::Executed);
    assert_eq!(marker_outcome(&b""[..]), Marker::Absent);
    assert_eq!(marker_outcome(Failing), Marker::Unknown);
    assert_eq!(marker_outcome(&b"ex"[..]), Marker::Unknown);
    // What follows the marker is the Hakoniwa closure's Landlock report, nothing else.
    let hako = super::super::hako::Landlock::Full { abi: 11 };
    let reported = format!("exec\n{}", hako.line());
    assert_eq!(
        marker_report(reported.as_bytes()),
        (Marker::Executed, Some(hako))
    );
    assert_eq!(
        marker_report(&b"exec\n"[..]),
        (Marker::Executed, None),
        "a closure that writes no report"
    );
    assert_eq!(
        marker_report(&b"exec\nnot a report\n"[..]),
        (Marker::Executed, None)
    );
    assert_eq!(
        marker_report(&b"nope\n{\"landlock\":\"none\"}\n"[..]),
        (Marker::Unknown, None),
        "a report without the marker first counts for nothing"
    );
}

#[tokio::test]
async fn a_marker_reader_still_running_is_unknown_and_keeps_the_level() {
    // The marker reader never ends (a slow pipe): after the bounded wait the outcome is
    // unknown, so the exit is counted at level 1.
    let readers = Readers::new(1);
    let marker = MarkerState::new();
    let outcome = settled_marker(&readers, &marker, Duration::from_millis(20)).await;
    assert_eq!(outcome, Marker::Unknown);
    assert!(matches!(
        classify(
            Level::Sandbox,
            outcome,
            Summary::default(),
            "exit status: 1"
        ),
        Fail::Other(_)
    ));
    // Once it ended cleanly without the marker: absent.
    marker.set(Marker::Absent);
    readers.end();
    assert_eq!(
        settled_marker(&readers, &marker, Duration::from_millis(20)).await,
        Marker::Absent
    );
}

/// Every process whose command line names `needle`.
fn processes_naming(needle: &str) -> Vec<i32> {
    std::fs::read_dir("/proc")
        .unwrap()
        .flatten()
        .filter_map(|entry| entry.file_name().to_string_lossy().parse::<i32>().ok())
        .filter(|pid| {
            std::fs::read(format!("/proc/{pid}/cmdline"))
                .is_ok_and(|line| String::from_utf8_lossy(&line).contains(needle))
        })
        .collect()
}

#[tokio::test]
async fn a_failure_after_the_spawn_leaves_no_process_behind() {
    let setup = setup(MODEL, "fake");
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    let marker = setup.settings.data_dir.join("run").display().to_string();
    // `signal`: the second pidfd handle, without which the child could not be stopped.
    for (generation, step) in [
        (1, "spawned"),
        (2, "recorded"),
        (3, "connector"),
        (4, "signal"),
    ] {
        *FAULT.lock().unwrap() = Some((setup.settings.runtime_dir.clone(), step));
        let result = launch.start(generation, Level::Unconfined, Path::Cpu).await;
        // The failed start's cleanup is in-flight work until the child is gone.
        launch.settle().await;
        let expected = if step == "signal" {
            "no pidfd to signal the model server".to_owned()
        } else {
            format!("injected at {step}")
        };
        assert!(
            matches!(&result, Err(Fail::Other(why)) if why == &expected),
            "{step}: {:?}",
            result.err()
        );
        // The runner the failure fired on is gone and reaped (no zombie either), and no
        // process naming the generation directory is left.
        let pid = FAULTED.lock().unwrap().take().unwrap();
        assert!(
            std::fs::metadata(format!("/proc/{pid}")).is_err(),
            "{step}: {pid} is left"
        );
        assert_eq!(processes_naming(&marker), Vec::<i32>::new(), "{step}");
        // Nor its pidfile entry.
        assert_eq!(pidfile_text(&setup), "", "{step}");
        // Nor is its generation directory.
        assert!(
            !setup
                .settings
                .data_dir
                .join("run")
                .join(generation.to_string())
                .exists()
        );
    }
}

#[tokio::test]
async fn gone_is_never_assumed_while_the_process_runs_past_the_grace() {
    let mut sleeper = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let watched = Watched::open(i32::try_from(sleeper.id()).unwrap()).unwrap();
    // The kill does nothing (as if the process could not be killed yet): past several graces
    // the wait is still pending.
    let pending = tokio::time::timeout(
        Duration::from_millis(500),
        until_gone(&watched.pidfd, Duration::from_millis(100), || {}),
    )
    .await;
    assert!(pending.is_err());
    sleeper.kill().unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        until_gone(&watched.pidfd, Duration::from_millis(100), || {}),
    )
    .await
    .unwrap();
    sleeper.wait().unwrap();
}

#[tokio::test]
async fn a_running_server_outlives_the_stop_grace() {
    let setup = setup(MODEL, "fake");
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    let child = launch
        .start_real(1, Level::Unconfined, Path::Cpu)
        .await
        .unwrap();
    // Nothing asked it to stop: past the grace it still runs and has not exited.
    tokio::time::sleep(STOP_GRACE + Duration::from_secs(2)).await;
    assert!(child.exit.borrow().is_none());
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", child.pid)).unwrap();
    assert!(
        !stat
            .rsplit(')')
            .next()
            .unwrap()
            .trim_start()
            .starts_with('Z'),
        "{stat}"
    );
    child.stop().await;
}

#[tokio::test]
async fn a_child_dropped_by_an_interrupted_start_is_killed() {
    let setup = setup(MODEL, "fake");
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    let child = launch
        .start_real(1, Level::Unconfined, Path::Cpu)
        .await
        .unwrap();
    let pid = child.pid;
    assert!(std::fs::metadata(format!("/proc/{pid}")).is_ok());
    drop(child);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let state = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        let running = !state.is_empty()
            && !state
                .rsplit(')')
                .next()
                .unwrap_or("")
                .trim_start()
                .starts_with('Z');
        if !running {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the child outlived its holder"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[test]
fn a_probe_that_only_times_out_is_retried_and_never_skips_its_level() {
    let mut calls = 0;
    let kept = retry_probe(Level::Sandbox, Duration::ZERO, &|| false, || {
        calls += 1;
        Probe::Inconclusive("did not report within 30 s".into())
    });
    assert_eq!(kept, Probed::Kept("did not report within 30 s".into()));
    assert_eq!(calls, PROBE_TRIES);
    assert_eq!(kept.eligible(), Ok(()));
    let mut calls = 0;
    let passed = retry_probe(Level::Unconfined, Duration::ZERO, &|| false, || {
        calls += 1;
        if calls < 2 {
            Probe::Inconclusive("no pidfd".into())
        } else {
            Probe::Passed
        }
    });
    assert_eq!((passed, calls), (Probed::Passed, 2));
    let mut calls = 0;
    let failed = retry_probe(Level::Sandbox, Duration::ZERO, &|| false, || {
        calls += 1;
        Probe::Failed("mimik sandbox: setup failed".into())
    });
    assert_eq!(
        (failed, calls),
        (Probed::Failed("mimik sandbox: setup failed".into()), 1)
    );
}

#[test]
fn a_transient_endpoint_error_is_inconclusive_a_refusal_is_not() {
    for code in [libc::EMFILE, libc::ENFILE, libc::ENOMEM, libc::EAGAIN] {
        assert!(
            matches!(
                endpoint_verdict(&std::io::Error::from_raw_os_error(code)),
                Probe::Inconclusive(_)
            ),
            "{code}"
        );
    }
    assert!(matches!(
        endpoint_verdict(&std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "the socket's peer is another user"
        )),
        Probe::Failed(_)
    ));
    assert!(matches!(
        endpoint_verdict(&std::io::Error::from_raw_os_error(libc::ECONNREFUSED)),
        Probe::Failed(_)
    ));
}

#[tokio::test]
async fn a_leftover_still_running_blocks_the_start_and_keeps_its_entry() {
    let setup = setup(MODEL, "fake");
    let launch = launch(&setup);
    std::fs::create_dir_all(&setup.settings.data_dir).unwrap();
    let run = rundir::open(&setup.settings.data_dir).unwrap();
    let mut leftover = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let pid = i32::try_from(leftover.id()).unwrap();
    let entry = rundir::Entry::of(9, pid, vec![rundir::executable(pid).unwrap()]).unwrap();
    {
        let mut pidfile = Pidfile::acquire(run.as_fd()).unwrap().unwrap();
        pidfile.record(&entry).unwrap();
    }
    drop(rundir::create_generation(run.as_fd(), "7").unwrap());
    let generation = setup.settings.data_dir.join("run/7");
    // Killed, but not yet ended: the start is refused, the entry and the old generation stay.
    *SWEEP_PENDING.lock().unwrap() = Some(setup.settings.data_dir.clone());
    let refused = launch.preflight(Path::Cpu).await.unwrap_err();
    *SWEEP_PENDING.lock().unwrap() = None;
    assert!(refused.contains("has not exited yet"), "{refused}");
    assert!(generation.exists());
    let pidfile_text =
        std::fs::read_to_string(setup.settings.data_dir.join("run/assistant.pid")).unwrap();
    assert!(pidfile_text.starts_with("9\n"), "{pidfile_text}");
    // Once it has ended, the next preflight sweeps and proceeds.
    leftover.wait().unwrap();
    launch.preflight(Path::Cpu).await.unwrap();
    assert!(!generation.exists());
}

fn pidfile_text(setup: &Setup) -> String {
    std::fs::read_to_string(setup.settings.data_dir.join("run/assistant.pid")).unwrap()
}

/// Starts generation 1 at level 3 paused at `step`, drops the start, cancels, and checks
/// that the settle waits for the blocking work, which then leaves nothing behind.
async fn cancelled_at(step: &'static str) {
    let setup = setup(MODEL, "fake");
    let launch = Arc::new(launch(&setup));
    launch.preflight(Path::Cpu).await.unwrap();
    let (arrived_sender, arrived) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    PAUSE.lock().unwrap().push(Pause {
        dir: setup.settings.runtime_dir.clone(),
        step,
        arrived: arrived_sender,
        release: released,
    });
    let starting = {
        let launch = Arc::clone(&launch);
        tokio::spawn(async move {
            launch
                .start(1, Level::Unconfined, Path::Cpu)
                .await
                .map(|_| ())
        })
    };
    tokio::task::spawn_blocking(move || arrived.recv_timeout(Duration::from_secs(30)).unwrap())
        .await
        .unwrap();
    starting.abort();
    let _ = starting.await;
    launch.cancel();
    let settling = {
        let launch = Arc::clone(&launch);
        tokio::spawn(async move { launch.settle().await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !settling.is_finished(),
        "{step}: settled while the start's blocking work still ran"
    );
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), settling)
        .await
        .unwrap()
        .unwrap();
    let marker = setup.settings.data_dir.join("run").display().to_string();
    assert_eq!(processes_naming(&marker), Vec::<i32>::new(), "{step}");
    assert_eq!(pidfile_text(&setup), "", "{step}");
    assert!(!setup.settings.data_dir.join("run/1").exists(), "{step}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_start_cancelled_before_its_spawn_spawns_nothing_and_settles_after_it() {
    cancelled_at("before-spawn").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_start_cancelled_after_its_spawn_never_records_and_leaves_nothing() {
    cancelled_at("before-record").await;
}

#[tokio::test]
async fn a_stale_clear_never_removes_a_newer_entry() {
    let setup = setup(MODEL, "fake");
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    let me = i32::try_from(std::process::id()).unwrap();
    let entry = rundir::Entry::of(5, me, vec!["/x".into()]).unwrap();
    launch.0.record(&entry).unwrap();
    // A probe of an earlier session ends after generation 5 recorded itself.
    launch.0.clear(PROBE_KEYS + 1);
    assert!(pidfile_text(&setup).starts_with("5\n"));
    launch.0.clear(5);
    assert_eq!(pidfile_text(&setup), "");
}

#[tokio::test]
async fn a_failed_starts_guard_kills_and_waits_for_the_reaper_too() {
    let runner = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let mut reaper_process = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let sandbox = Arc::new(Sandbox {
        reaper: Watched::open(i32::try_from(reaper_process.id()).unwrap()).unwrap(),
        pid_namespace: 0,
        inner: std::sync::OnceLock::new(),
    });
    let work = Work::new();
    let mut owned = Owned::new(runner, work.enter());
    owned.reaper = Some(Arc::clone(&sandbox));
    drop(owned);
    tokio::time::timeout(Duration::from_secs(10), work.settle())
        .await
        .unwrap();
    assert!(endpoint::exited(&sandbox.reaper.pidfd));
    reaper_process.wait().unwrap();
}

/// Whether `pid` is a live process (a zombie is not).
fn pid_alive(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        !stat
            .rsplit(')')
            .next()
            .unwrap_or("")
            .trim_start()
            .starts_with('Z')
    })
}

/// With Hakoniwa the reaper is outside the PID namespace and the server (PID 1 inside) holds
/// the memory: a reaper that has gone says nothing about the server. A sandbox is gone only
/// when its inner process is, and a forced stop kills that process first.
#[tokio::test]
async fn a_failed_starts_guard_kills_and_waits_for_the_inner_process_too() {
    let runner = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    // The reaper is already gone; the server is not.
    let mut reaper_process = std::process::Command::new("true").spawn().unwrap();
    let mut inner_process = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let reaper_watch = Watched::open(i32::try_from(reaper_process.id()).unwrap()).unwrap();
    reaper_process.wait().unwrap();
    let sandbox = Arc::new(Sandbox {
        reaper: reaper_watch,
        pid_namespace: 0,
        inner: std::sync::OnceLock::from(endpoint::InnerPin {
            watched: Watched::open(i32::try_from(inner_process.id()).unwrap()).unwrap(),
            verified: true,
        }),
    });
    let work = Work::new();
    let mut owned = Owned::new(runner, work.enter());
    owned.reaper = Some(Arc::clone(&sandbox));
    drop(owned);
    tokio::time::timeout(Duration::from_secs(10), work.settle())
        .await
        .unwrap();
    assert!(endpoint::exited(
        &sandbox.inner.get().unwrap().watched.pidfd
    ));
    inner_process.wait().unwrap();
}

#[tokio::test]
async fn the_exit_is_published_only_once_the_inner_process_is_gone() {
    let mut runner = std::process::Command::new("true").spawn().unwrap();
    let runner_watch = Watched::open(i32::try_from(runner.id()).unwrap()).unwrap();
    let mut reaper_process = std::process::Command::new("true").spawn().unwrap();
    let inner_process = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let inner_pid = i32::try_from(inner_process.id()).unwrap();
    let sandbox = Arc::new(Sandbox {
        reaper: Watched::open(i32::try_from(reaper_process.id()).unwrap()).unwrap(),
        pid_namespace: 0,
        inner: std::sync::OnceLock::from(endpoint::InnerPin {
            watched: Watched::open(inner_pid).unwrap(),
            verified: true,
        }),
    });
    reaper_process.wait().unwrap();
    let (sender, mut receiver) = watch::channel(None);
    let sampled = Arc::new(Mutex::new(None));
    let on_gone = {
        let sampled = Arc::clone(&sampled);
        move || {
            // The instant the exit is about to be published: is the server still there?
            *sampled.lock().unwrap() = Some(pid_alive(inner_pid));
        }
    };
    let _ = runner.wait();
    tokio::spawn(watch_exit(
        runner,
        runner_watch,
        Some(sandbox),
        sender,
        on_gone,
    ));
    tokio::time::timeout(Duration::from_secs(10), receiver.wait_for(Option::is_some))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        *sampled.lock().unwrap(),
        Some(false),
        "the server outlived the exit"
    );
    let mut inner_process = inner_process;
    let _ = inner_process.wait();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reaper_that_outlives_the_wait_keeps_its_entry_and_blocks_the_settle() {
    let setup = setup(MODEL, "fake");
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    let runner = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let mut reaper_process = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();
    let me = i32::try_from(std::process::id()).unwrap();
    launch
        .0
        .record(&rundir::Entry::of(7, me, vec!["/x".into()]).unwrap())
        .unwrap();
    let sandbox = Arc::new(Sandbox {
        reaper: Watched::open(i32::try_from(reaper_process.id()).unwrap()).unwrap(),
        pid_namespace: 0,
        inner: std::sync::OnceLock::new(),
    });
    let mut owned = Owned::new(runner, launch.0.work().enter());
    owned.reaper = Some(Arc::clone(&sandbox));
    owned.recorded = Some((Arc::clone(&launch.0.pidfile), 7));
    // The kill does not reach it (as with a process that cannot end yet).
    owned.unkillable = true;
    drop(owned);
    // Past the 5 s the cleanup used to give up after: still tracked, the entry kept.
    let settling = tokio::time::timeout(REAPER_WITHIN + Duration::from_secs(1), launch.settle());
    assert!(
        settling.await.is_err(),
        "settled while the reaper still ran"
    );
    assert!(pidfile_text(&setup).starts_with("7\n"));
    reaper_process.kill().unwrap();
    reaper_process.wait().unwrap();
    tokio::time::timeout(Duration::from_secs(10), launch.settle())
        .await
        .unwrap();
    assert_eq!(pidfile_text(&setup), "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_abandoned_spawn_counts_until_its_cleanup_ends() {
    let setup = setup(MODEL, "fake");
    let launch = Arc::new(launch(&setup));
    launch.preflight(Path::Cpu).await.unwrap();
    let pause = |step| {
        let (arrived_sender, arrived) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        PAUSE.lock().unwrap().push(Pause {
            dir: setup.settings.runtime_dir.clone(),
            step,
            arrived: arrived_sender,
            release: released,
        });
        (arrived, release)
    };
    let (spawned, release_spawned) = pause("spawned");
    let (cleaning, release_cleanup) = pause("cleanup");
    let starting = {
        let launch = Arc::clone(&launch);
        tokio::spawn(async move {
            launch
                .start(1, Level::Unconfined, Path::Cpu)
                .await
                .map(|_| ())
        })
    };
    // Past the last cancel check: the spawn succeeds, for a start nobody awaits any more.
    tokio::task::spawn_blocking(move || spawned.recv_timeout(Duration::from_secs(30)).unwrap())
        .await
        .unwrap();
    starting.abort();
    let _ = starting.await;
    launch.cancel();
    release_spawned.send(()).unwrap();
    tokio::task::spawn_blocking(move || cleaning.recv_timeout(Duration::from_secs(30)).unwrap())
        .await
        .unwrap();
    let settling = {
        let launch = Arc::clone(&launch);
        tokio::spawn(async move { launch.settle().await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !settling.is_finished(),
        "settled while the abandoned spawn's cleanup ran"
    );
    release_cleanup.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), settling)
        .await
        .unwrap()
        .unwrap();
    let marker = setup.settings.data_dir.join("run").display().to_string();
    assert_eq!(processes_naming(&marker), Vec::<i32>::new());
    assert_eq!(pidfile_text(&setup), "");
    assert!(!setup.settings.data_dir.join("run/1").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_preflight_dropped_during_its_sweep_settles_after_it_and_the_next_one_proceeds() {
    let setup = setup(MODEL, "fake");
    let launch = Arc::new(launch(&setup));
    let (arrived_sender, arrived) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    PAUSE.lock().unwrap().push(Pause {
        dir: setup.settings.runtime_dir.clone(),
        step: "sweep",
        arrived: arrived_sender,
        release: released,
    });
    let preflight = {
        let launch = Arc::clone(&launch);
        tokio::spawn(async move { launch.preflight(Path::Cpu).await })
    };
    tokio::task::spawn_blocking(move || arrived.recv_timeout(Duration::from_secs(30)).unwrap())
        .await
        .unwrap();
    preflight.abort();
    let _ = preflight.await;
    launch.cancel();
    let settling = {
        let launch = Arc::clone(&launch);
        tokio::spawn(async move { launch.settle().await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!settling.is_finished(), "settled while the sweep ran");
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), settling)
        .await
        .unwrap()
        .unwrap();
    // The pidfile came back: no "another Mimik".
    launch.preflight(Path::Cpu).await.unwrap();
}

#[test]
fn a_transient_proc_error_in_the_listener_scan_stays_inconclusive() {
    use procfs::ProcError;
    let emfile = || ProcError::Io(std::io::Error::from_raw_os_error(libc::EMFILE), None);
    // The descriptor walk: a closed descriptor is skipped, any other error fails the scan
    // with its errno, which the probe's verdict treats as transient.
    let scanned = endpoint::socket_inodes(
        [
            Ok(Some(5)),
            Err(ProcError::NotFound(None)),
            Ok(None),
            Ok(Some(7)),
        ]
        .into_iter(),
    )
    .unwrap();
    assert_eq!(scanned.into_iter().collect::<Vec<_>>(), [5, 7]);
    let failed = endpoint::socket_inodes([Ok(Some(5)), Err(emfile())].into_iter()).unwrap_err();
    assert!(
        matches!(endpoint_verdict(&failed), Probe::Inconclusive(_)),
        "{failed}"
    );
    // The tables' reads: the same errno.
    assert!(matches!(
        endpoint_verdict(&endpoint::proc_error(emfile())),
        Probe::Inconclusive(_)
    ));
    assert!(matches!(
        endpoint_verdict(&endpoint::proc_error(ProcError::Incomplete(None))),
        Probe::Inconclusive(_)
    ));
}

#[test]
fn a_cancel_ends_the_retry_pause_at_once() {
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let started = std::time::Instant::now();
    let probed = retry_probe(
        Level::Sandbox,
        Duration::from_secs(30),
        &|| cancelled.load(Ordering::SeqCst),
        || {
            cancelled.store(true, Ordering::SeqCst);
            Probe::Inconclusive("timed out".into())
        },
    );
    assert_eq!(probed, Probed::Failed("cancelled".into()));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn a_cancel_ends_a_wait_for_the_endpoint() {
    let work = Work::new();
    let ticket = work.enter();
    work.cancel();
    tokio::time::timeout(Duration::from_secs(2), until_cancelled(&ticket))
        .await
        .unwrap();
}

#[tokio::test]
async fn three_real_spawn_failures_keep_the_level_for_the_start() {
    let mut setup = setup(MODEL, "fake");
    // The runner cannot be started at all: each spawn really fails.
    setup.settings.runner.program = PathBuf::from("/nonexistent/mimik");
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    let real = Arc::clone(&launch.0);
    let probed = tokio::task::spawn_blocking(move || {
        let ticket = real.work().enter();
        let probed = real.probe_with_retries(Path::Cpu, Level::Unconfined, &ticket);
        (probed, real.probes.load(Ordering::SeqCst))
    })
    .await
    .unwrap();
    assert!(
        matches!(&probed.0, Probed::Kept(why) if why.contains("runner")),
        "{probed:?}"
    );
    // Three attempts (the sequence started at 1).
    assert_eq!(probed.1, 1 + PROBE_TRIES as u64);
}

#[test]
fn a_probe_port_taken_meanwhile_is_tried_again() {
    let taken = serde_json::json!({"ok": false, "port": "address in use", "retry": true});
    assert!(matches!(report_verdict(&taken), Probe::Inconclusive(_)));
    let failed = serde_json::json!({"ok": false, "model": "unreadable", "retry": false});
    assert!(matches!(report_verdict(&failed), Probe::Failed(_)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_probe_command_ends_only_after_its_probes_cleanup() {
    let setup = setup(MODEL, "fake");
    let launch = Arc::new(launch(&setup));
    launch.preflight(Path::Cpu).await.unwrap();
    let (arrived_sender, arrived) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    PAUSE.lock().unwrap().push(Pause {
        dir: setup.settings.runtime_dir.clone(),
        step: "cleanup",
        arrived: arrived_sender,
        release: released,
    });
    let probing = {
        let launch = Arc::clone(&launch);
        tokio::spawn(async move { super::super::probe::probe_and_settle(&launch).await })
    };
    tokio::task::spawn_blocking(move || arrived.recv_timeout(Duration::from_secs(60)).unwrap())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !probing.is_finished(),
        "the probe command ended before its cleanup"
    );
    release.send(()).unwrap();
    let report = tokio::time::timeout(Duration::from_secs(60), probing)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        report["levels"]["3"]["ok"],
        serde_json::json!(true),
        "{report}"
    );
    assert_eq!(pidfile_text(&setup), "");
}

#[tokio::test]
async fn level_3_executes_and_loads_from_the_checked_real_path_not_the_alias() {
    let mut setup = setup(MODEL, "fake");
    let root = setup.dir.path().to_path_buf();
    let open = root.join("open");
    std::fs::create_dir(&open).unwrap();
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
    // The runtime folder reached through a link held in a folder others can write.
    std::os::unix::fs::symlink(&root, open.join("rt")).unwrap();
    setup.settings.runtime_dir = open.join("rt/runtime");
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    let child = launch
        .start_real(1, Level::Unconfined, Path::Cpu)
        .await
        .unwrap();
    let cpu = std::fs::canonicalize(root.join("runtime/cpu")).unwrap();
    let environ = std::fs::read(format!("/proc/{}/environ", child.pid)).unwrap();
    // LD_LIBRARY_PATH names the verified directory by descriptor (its inode), not a path.
    let library = format!("LD_LIBRARY_PATH=/proc/self/fd/{}", fds::BUILD_DIR);
    assert!(
        environ
            .split(|byte| *byte == 0)
            .any(|entry| entry == library.as_bytes()),
        "{}",
        String::from_utf8_lossy(&environ)
    );
    // That descriptor is the verified cpu directory.
    let fd_dir = std::fs::read_link(format!("/proc/{}/fd/{}", child.pid, fds::BUILD_DIR)).unwrap();
    assert_eq!(fd_dir, cpu, "{}", fd_dir.display());
    assert_eq!(
        std::fs::read_link(format!("/proc/{}/exe", child.pid)).unwrap(),
        cpu.join("llama-server")
    );
    // The cwd is the checked build folder, so ggml's search of the current directory finds
    // only it, never Mimik's own (attacker-influenced) cwd.
    assert_eq!(
        std::fs::read_link(format!("/proc/{}/cwd", child.pid)).unwrap(),
        cpu
    );
    // The pidfile names the real executable, so a later sweep recognises it.
    assert!(
        pidfile_text(&setup).contains(&cpu.join("llama-server").display().to_string()),
        "{}",
        pidfile_text(&setup)
    );
    child.stop().await;
}

/// A pause at `step` for this setup's launcher: (arrived, release).
fn pause_for(
    setup: &Setup,
    step: &'static str,
) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
    let (arrived_sender, arrived) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    PAUSE.lock().unwrap().push(Pause {
        dir: setup.settings.runtime_dir.clone(),
        step,
        arrived: arrived_sender,
        release: released,
    });
    (arrived, release)
}

/// With the earlier attempt's cleanup held, a start of generation 2 does not reach its spawn
/// (and leaves the earlier entry alone); released, it proceeds.
async fn the_next_spawn_waits(
    setup: &Setup,
    launch: &Arc<Launch>,
    release_cleanup: std::sync::mpsc::Sender<()>,
) {
    let (spawning, release_spawn) = pause_for(setup, "before-spawn");
    let starting = {
        let launch = Arc::clone(launch);
        tokio::spawn(async move { launch.start_real(2, Level::Unconfined, Path::Cpu).await })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        spawning.try_recv().is_err(),
        "a spawn began beside an earlier attempt's cleanup"
    );
    assert!(!pidfile_text(setup).starts_with("2\n"));
    release_cleanup.send(()).unwrap();
    tokio::task::spawn_blocking(move || spawning.recv_timeout(Duration::from_secs(30)).unwrap())
        .await
        .unwrap();
    release_spawn.send(()).unwrap();
    let child = starting.await.unwrap().unwrap();
    assert!(pidfile_text(setup).starts_with("2\n"));
    child.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_a_failed_start_the_next_spawn_waits_for_its_cleanup() {
    let setup = setup(MODEL, "fake");
    let launch = Arc::new(launch(&setup));
    launch.preflight(Path::Cpu).await.unwrap();
    let (cleaning, release_cleanup) = pause_for(&setup, "cleanup");
    *FAULT.lock().unwrap() = Some((setup.settings.runtime_dir.clone(), "connector"));
    assert!(launch.start(1, Level::Unconfined, Path::Cpu).await.is_err());
    tokio::task::spawn_blocking(move || cleaning.recv_timeout(Duration::from_secs(30)).unwrap())
        .await
        .unwrap();
    assert!(pidfile_text(&setup).starts_with("1\n"));
    the_next_spawn_waits(&setup, &launch, release_cleanup).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_a_probe_the_first_start_waits_for_its_cleanup() {
    let setup = setup(MODEL, "fake");
    let launch = Arc::new(launch(&setup));
    launch.preflight(Path::Cpu).await.unwrap();
    let (cleaning, release_cleanup) = pause_for(&setup, "cleanup");
    let real = Arc::clone(&launch.0);
    let probed = tokio::task::spawn_blocking(move || {
        let ticket = real.work().enter();
        real.probe_with_retries(Path::Cpu, Level::Unconfined, &ticket)
    })
    .await
    .unwrap();
    assert_eq!(probed, Probed::Passed);
    tokio::task::spawn_blocking(move || cleaning.recv_timeout(Duration::from_secs(30)).unwrap())
        .await
        .unwrap();
    the_next_spawn_waits(&setup, &launch, release_cleanup).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_preflight_during_a_sweep_says_the_sweep_runs() {
    let setup = setup(MODEL, "fake");
    let launch = Arc::new(launch(&setup));
    let (sweeping, release) = pause_for(&setup, "sweep");
    let first = {
        let launch = Arc::clone(&launch);
        tokio::spawn(async move { launch.preflight(Path::Cpu).await })
    };
    tokio::task::spawn_blocking(move || sweeping.recv_timeout(Duration::from_secs(30)).unwrap())
        .await
        .unwrap();
    let refused = launch.preflight(Path::Cpu).await.unwrap_err();
    assert!(refused.contains("leftover sweep"), "{refused}");
    release.send(()).unwrap();
    first.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_panic_in_the_sweep_ends_the_sweep_and_keeps_the_pidfile() {
    let setup = setup(MODEL, "fake");
    let launch = launch(&setup);
    *SWEEP_PANIC.lock().unwrap() = Some(setup.settings.data_dir.clone());
    let failed = launch.preflight(Path::Cpu).await.unwrap_err();
    *SWEEP_PANIC.lock().unwrap() = None;
    assert!(failed.contains("did not finish"), "{failed}");
    // Neither "the sweep is still running" nor "another Mimik": the next preflight sweeps.
    launch.preflight(Path::Cpu).await.unwrap();
}

#[test]
fn a_slow_exit_is_said_once_and_still_waited_for() {
    let until = std::time::Instant::now() + Duration::from_millis(400);
    let mut warnings = 0;
    wait_exited(
        &|| std::time::Instant::now() < until,
        &|| {},
        Duration::from_millis(100),
        &mut || warnings += 1,
    );
    assert!(std::time::Instant::now() >= until);
    assert_eq!(warnings, 1);
}

#[tokio::test]
async fn a_probe_whose_port_is_taken_tries_again_on_a_fresh_port() {
    let setup = setup(MODEL, "fake");
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    // Another process holds the port the launcher picked first.
    let holder = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let taken = holder.local_addr().unwrap().port();
    PORTS
        .lock()
        .unwrap()
        .push((setup.settings.runtime_dir.clone(), taken));
    let real = Arc::clone(&launch.0);
    let (first, probed, attempts) = tokio::task::spawn_blocking(move || {
        let ticket = real.work().enter();
        // The inner probe itself (valid model and token) against the taken port.
        let first = real.probe_level(Path::Cpu, Level::Unconfined, &ticket);
        PORTS
            .lock()
            .unwrap()
            .push((real.settings.runtime_dir.clone(), taken));
        let probed = real.probe_with_retries(Path::Cpu, Level::Unconfined, &ticket);
        (first, probed, real.probes.load(Ordering::SeqCst))
    })
    .await
    .unwrap();
    assert_eq!(
        first,
        Probe::Inconclusive("the probe's port was taken meanwhile".into())
    );
    // Retried on a new port: passed on the second attempt (sequence from 1: one probe, then
    // two attempts).
    assert_eq!(probed, Probed::Passed);
    assert_eq!(attempts, 1 + 1 + 2);
    drop(holder);
}

#[derive(Debug)]
struct Stalled;

impl crate::assistant::transport::Connect for Stalled {
    fn connect(&self) -> crate::assistant::transport::Connecting {
        Box::pin(std::future::pending())
    }

    fn authority(&self) -> String {
        "127.0.0.1:1".into()
    }
}

#[tokio::test]
async fn a_cancel_ends_the_probes_endpoint_check() {
    let work = Work::new();
    let ticket = work.enter();
    let connector: Arc<dyn crate::assistant::transport::Connect> = Arc::new(Stalled);
    let checking = tokio::spawn({
        let ticket = ticket.clone();
        async move { checked_endpoint(&connector, &ticket).await }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    work.cancel();
    let verdict = tokio::time::timeout(Duration::from_secs(2), checking)
        .await
        .expect("the endpoint check outlived the cancel")
        .unwrap();
    assert_eq!(verdict, Probe::Failed("cancelled".into()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_start_cancelled_during_its_verification_is_cancelled() {
    let setup = setup(MODEL, "fake");
    let launch = Arc::new(launch(&setup));
    launch.preflight(Path::Cpu).await.unwrap();
    let (verifying, release) = pause_for(&setup, "runtime-opened");
    let starting = {
        let launch = Arc::clone(&launch);
        tokio::spawn(async move {
            launch
                .start_real(1, Level::Unconfined, Path::Cpu)
                .await
                .map(|_| ())
        })
    };
    tokio::task::spawn_blocking(move || verifying.recv_timeout(Duration::from_secs(30)).unwrap())
        .await
        .unwrap();
    launch.cancel();
    release.send(()).unwrap();
    let result = starting.await.unwrap();
    assert!(
        matches!(&result, Err(Fail::Other(why)) if why == "cancelled"),
        "{:?}",
        result.err()
    );
}

#[tokio::test]
async fn a_start_whose_record_fails_fails_and_leaves_nothing() {
    let setup = setup(MODEL, "fake");
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    *RECORD_FAILS.lock().unwrap() = Some(setup.settings.runtime_dir.clone());
    let result = launch.start(1, Level::Unconfined, Path::Cpu).await;
    *RECORD_FAILS.lock().unwrap() = None;
    assert!(
        matches!(&result, Err(Fail::Other(why)) if why == "the pidfile cannot be written"),
        "{:?}",
        result.err()
    );
    launch.settle().await;
    let marker = setup.settings.data_dir.join("run").display().to_string();
    assert_eq!(processes_naming(&marker), Vec::<i32>::new());
}

#[tokio::test]
async fn a_torn_pidfile_refuses_the_start_and_keeps_the_generations() {
    let setup = setup(MODEL, "fake");
    let launch = launch(&setup);
    std::fs::create_dir_all(&setup.settings.data_dir).unwrap();
    let run = rundir::open(&setup.settings.data_dir).unwrap();
    {
        let mut pidfile = Pidfile::acquire(run.as_fd()).unwrap().unwrap();
        pidfile.write_raw(b"9\n4242\n");
    }
    drop(rundir::create_generation(run.as_fd(), "9").unwrap());
    let refused = launch.preflight(Path::Cpu).await.unwrap_err();
    assert!(refused.contains("cannot be read"), "{refused}");
    assert!(setup.settings.data_dir.join("run/9").exists());
    // The owner removes the torn file; the next preflight re-acquires and proceeds.
    std::fs::remove_file(setup.settings.data_dir.join("run/assistant.pid")).unwrap();
    launch.preflight(Path::Cpu).await.unwrap();
    assert!(!setup.settings.data_dir.join("run/9").exists());
}

#[tokio::test]
async fn level_3_loads_from_the_verified_inode_after_a_runtime_rename() {
    let setup = setup(MODEL, "fake");
    let root = setup.dir.path().to_path_buf();
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    let cpu = std::fs::canonicalize(root.join("runtime/cpu")).unwrap();
    let before = rustix::fs::stat(&cpu).unwrap();
    let child = launch
        .start_real(1, Level::Unconfined, Path::Cpu)
        .await
        .unwrap();
    // The runtime directory is renamed after the start: the handed-over descriptor still names
    // the verified inode, so the loader is not redirected.
    std::fs::rename(root.join("runtime/cpu"), root.join("runtime/moved")).unwrap();
    let fd_inode = rustix::fs::stat(format!("/proc/{}/fd/{}", child.pid, fds::BUILD_DIR)).unwrap();
    assert_eq!(
        (fd_inode.st_dev, fd_inode.st_ino),
        (before.st_dev, before.st_ino)
    );
    let cwd_inode = rustix::fs::stat(format!("/proc/{}/cwd", child.pid)).unwrap();
    assert_eq!(
        (cwd_inode.st_dev, cwd_inode.st_ino),
        (before.st_dev, before.st_ino)
    );
    child.stop().await;
}

#[tokio::test]
async fn a_start_whose_start_time_is_unreadable_fails_and_leaves_nothing() {
    let setup = setup(MODEL, "fake");
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    *NO_START_TIME.lock().unwrap() = Some(setup.settings.runtime_dir.clone());
    let result = launch.start(1, Level::Unconfined, Path::Cpu).await;
    *NO_START_TIME.lock().unwrap() = None;
    assert!(
        matches!(&result, Err(Fail::Other(why)) if why.contains("start time cannot be read")),
        "{:?}",
        result.err()
    );
    launch.settle().await;
    let marker = setup.settings.data_dir.join("run").display().to_string();
    assert_eq!(processes_naming(&marker), Vec::<i32>::new());
    assert_eq!(pidfile_text(&setup), "");
}

#[test]
fn the_cleanup_waits_for_a_runner_without_a_pidfd_and_says_so() {
    // A runner slower than the warning, with no pidfd kept (as if it could not be opened):
    // the try_wait fallback waits it out and says so, never a bare unsaid wait.
    let runner = std::process::Command::new("sleep")
        .arg("6")
        .spawn()
        .unwrap();
    let pid = runner.id();
    let before = RUNNER_WARNINGS.load(Ordering::SeqCst);
    let cleanup = Cleanup {
        child: Some(runner),
        runner_pidfd: None,
        reaper: None,
        recorded: None,
        ticket: None,
        cleanups: None,
        dir: PathBuf::new(),
        unkillable: false,
        no_runner_pidfd: true,
    };
    let started = std::time::Instant::now();
    cleanup.run();
    assert!(started.elapsed() >= Duration::from_secs(5));
    assert!(RUNNER_WARNINGS.load(Ordering::SeqCst) > before);
    // Reaped: no zombie left.
    assert!(std::fs::metadata(format!("/proc/{pid}")).is_err());
}

/// A connector whose socket appears only after `missing` attempts (`errno` until then).
#[derive(Debug)]
struct LateSocket {
    missing: std::sync::atomic::AtomicUsize,
    errno: i32,
    keep: Mutex<Vec<tokio::net::UnixStream>>,
}

impl LateSocket {
    fn connector(missing: usize, errno: i32) -> Arc<dyn crate::assistant::transport::Connect> {
        Arc::new(Self {
            missing: std::sync::atomic::AtomicUsize::new(missing),
            errno,
            keep: Mutex::new(Vec::new()),
        })
    }
}

impl crate::assistant::transport::Connect for LateSocket {
    fn connect(&self) -> crate::assistant::transport::Connecting {
        let late = self.missing.load(Ordering::SeqCst) > 0;
        if late {
            self.missing.fetch_sub(1, Ordering::SeqCst);
            let errno = self.errno;
            return Box::pin(async move { Err(std::io::Error::from_raw_os_error(errno)) });
        }
        let (ours, theirs) = tokio::net::UnixStream::pair().unwrap();
        self.keep.lock().unwrap().push(theirs);
        Box::pin(async move { Ok(crate::assistant::transport::Stream::Unix(ours)) })
    }

    fn authority(&self) -> String {
        crate::assistant::transport::UNIX_AUTHORITY.into()
    }
}

#[tokio::test]
async fn a_probe_socket_that_appears_late_still_passes_and_never_fails() {
    let work = Work::new();
    let ticket = work.enter();
    // Not there yet, not accepting yet, or reset as it starts: all "not ready", retried.
    for errno in [libc::ENOENT, libc::ECONNREFUSED, libc::ECONNRESET] {
        let late = LateSocket::connector(3, errno);
        assert_eq!(
            checked_endpoint_within(&late, &ticket, Duration::from_secs(5)).await,
            Probe::Passed,
            "{errno}"
        );
        // Never there within the window: inconclusive (the level is kept), not a failure.
        let never = LateSocket::connector(usize::MAX, errno);
        assert!(
            matches!(
                checked_endpoint_within(&never, &ticket, Duration::from_millis(300)).await,
                Probe::Inconclusive(_)
            ),
            "{errno}"
        );
    }
}

#[tokio::test]
async fn a_start_whose_socket_is_not_ready_yet_is_not_listening_not_failed() {
    // /health against a socket not created, not accepting or reset yet: "not listening",
    // which the start's health wait retries until its deadline, never a definite failure.
    for errno in [libc::ENOENT, libc::ECONNREFUSED, libc::ECONNRESET] {
        let server = Llama::new(
            Bounded::new(LateSocket::connector(usize::MAX, errno)),
            "token",
            "alias",
        );
        assert_eq!(
            server.health().await,
            super::super::server::Health::NotListening,
            "{errno}"
        );
    }
}

#[tokio::test]
async fn a_runtime_path_with_a_newline_is_refused_before_anything_is_spawned() {
    let mut setup = setup(MODEL, "fake");
    let dir = setup.dir.path().join("new\nline");
    std::fs::create_dir(&dir).unwrap();
    let runner = dir.join("runner");
    std::fs::copy(&setup.settings.runner.program, &runner).unwrap();
    setup.settings.runner.program = runner;
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    // run/1 already there: had the generation directory been created first, the start would
    // fail on it ("already exists") instead of on the newline.
    std::fs::create_dir(setup.settings.data_dir.join("run/1")).unwrap();
    let result = launch.start(1, Level::Unconfined, Path::Cpu).await;
    assert!(
        matches!(&result, Err(Fail::Other(why)) if why.starts_with("runtime path contains a newline")),
        "{:?}",
        result.err()
    );
    launch.settle().await;
    let marker = setup.settings.data_dir.join("run").display().to_string();
    assert_eq!(processes_naming(&marker), Vec::<i32>::new());
    assert_eq!(pidfile_text(&setup), "");
}

#[tokio::test]
async fn level_3_is_refused_unless_explicitly_allowed() {
    let setup = setup(MODEL, "fake");
    // Mimik's own launcher (no allowance): whatever a Config asks, level 3 never starts.
    let launch = Launch(Arc::new(Real::with_build(
        setup.settings.clone(),
        Some(setup.build.clone()),
    )));
    launch.preflight(Path::Cpu).await.unwrap();
    let result = launch.start(1, Level::Unconfined, Path::Cpu).await;
    // A sandbox-class refusal: the manager neither counts nor retries it.
    assert!(
        matches!(&result, Err(Fail::Sandbox(why)) if why == "level 3 is not used in this release"),
        "{:?}",
        result.err()
    );
    assert!(!setup.settings.data_dir.join("run/1").exists());
    assert_eq!(pidfile_text(&setup), "");
}

#[tokio::test]
async fn a_runner_path_with_a_newline_is_refused_before_any_directory_is_made() {
    let mut setup = setup(MODEL, "fake");
    setup.settings.runner.program = PathBuf::from("/usr/bin/mim\nik");
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    let real = Arc::clone(&launch.0);
    real.held().preflighted = true;
    // run/1 already there: a generation directory made before the check would fail on it.
    std::fs::create_dir(setup.settings.data_dir.join("run/1")).unwrap();
    let refused = tokio::task::spawn_blocking(move || {
        let ticket = real.work().enter();
        let checked = real.verify(Path::Cpu, Level::Sandbox, &ticket).unwrap();
        let job = Job {
            name: "1".into(),
            key: 1,
            ticket,
        };
        real.spawn_sandbox(
            &job,
            Path::Cpu,
            &checked.files,
            &checked.model,
            &Token::new(),
            &Command::Server { device: None },
            None,
            &checked.build_dir,
        )
        .err()
    })
    .await
    .unwrap();
    assert!(
        matches!(&refused, Some(Fail::Other(why)) if why.starts_with("runtime path contains a newline")),
        "{refused:?}"
    );
}

/// A fake sandbox for the pin tests: the "reaper" is a shell (a child of this test, standing
/// for the runner's child) that starts a `sleep` (its one child, the "server") and reports
/// its pid.
struct FakeSandbox {
    reaper: std::process::Child,
    inner_pid: i32,
    inner_start: u64,
}

fn fake_sandbox() -> FakeSandbox {
    use std::io::BufRead;
    let mut reaper = std::process::Command::new("sh")
        .args(["-c", "sleep 30 & echo $!; wait"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    std::io::BufReader::new(reaper.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let inner_pid: i32 = line.trim().parse().unwrap();
    let inner_start = rundir::start_time(inner_pid).unwrap();
    FakeSandbox {
        reaper,
        inner_pid,
        inner_start,
    }
}

fn started_of(reaper: &std::process::Child, inner: (i32, u64)) -> Started {
    Started {
        child_pid: i32::try_from(reaper.id()).unwrap(),
        pid_namespace: endpoint::NAMESPACE_UNREPORTED,
        inner_pid: Some(inner.0),
        inner_start: Some(inner.1),
        ended: false,
    }
}

/// Pins `started` for a guard over a stand-in runner, then drops the guard and waits for its
/// cleanup: what the guard left standing afterwards is what it did not wait for or kill.
async fn pinned_then_cleaned(started: &Started) -> Result<Arc<Sandbox>, String> {
    let runner = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let work = Work::new();
    let mut owned = Owned::new(runner, work.enter());
    let me = i32::try_from(std::process::id()).unwrap();
    let outcome = match pin_sandbox(&mut owned, started, me) {
        Ok(pinned) => Ok(pinned.sandbox),
        Err(Pin::Gone(error)) => Err(format!("gone: {error}")),
        Err(Pin::Fail(Fail::Other(why) | Fail::Sandbox(why))) => Err(why),
    };
    drop(owned);
    tokio::time::timeout(Duration::from_secs(10), work.settle())
        .await
        .unwrap();
    outcome
}

/// Mimik pauses before it pins; the reaper is killed meanwhile (left unreaped, as the
/// runner's child), and the server lives on, reparented. It is still pinned, by identity,
/// and the cleanup waits for it.
#[tokio::test]
async fn a_server_that_outlives_its_killed_reaper_is_pinned_and_waited_for() {
    let mut fake = fake_sandbox();
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(i32::try_from(fake.reaper.id()).unwrap()).unwrap(),
        rustix::process::Signal::KILL,
    )
    .unwrap();
    // Give the kill time to land and the server to be reparented (not reaped: it is alive).
    while pid_alive(i32::try_from(fake.reaper.id()).unwrap()) {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(pid_alive(fake.inner_pid));
    let started = started_of(&fake.reaper, (fake.inner_pid, fake.inner_start));
    let sandbox = pinned_then_cleaned(&started).await.unwrap();
    assert!(
        sandbox.inner.get().is_some(),
        "the server was not pinned although its reaper was gone"
    );
    assert!(
        !pid_alive(fake.inner_pid),
        "the cleanup left the server running"
    );
    let _ = fake.reaper.wait();
}

/// A server that is already gone (its pid reaped) pins nothing, and the reaper is still
/// attached and waited for.
#[tokio::test]
async fn a_dead_server_pins_nothing_and_the_reaper_is_still_waited_for() {
    let mut fake = fake_sandbox();
    let mut gone = std::process::Command::new("true").spawn().unwrap();
    let gone_pid = i32::try_from(gone.id()).unwrap();
    let gone_start = rundir::start_time(gone_pid).unwrap();
    gone.wait().unwrap();
    let started = started_of(&fake.reaper, (gone_pid, gone_start));
    let sandbox = pinned_then_cleaned(&started).await.unwrap();
    assert!(sandbox.inner.get().is_none());
    assert!(
        endpoint::exited(&sandbox.reaper.pidfd),
        "the reaper was not waited for"
    );
    let _ = fake.reaper.wait();
    // The real server of this fixture was never named: the cleanup of the reaper leaves it.
    let _ = rustix::process::kill_process(
        rustix::process::Pid::from_raw(fake.inner_pid).unwrap(),
        rustix::process::Signal::KILL,
    );
}

/// A pid that another process has now (the reported server's pid reused, so another start
/// time) is not the server: it is not pinned, never killed, and the reaper is waited for.
#[tokio::test]
async fn a_reused_server_pid_is_never_pinned_or_killed() {
    let mut fake = fake_sandbox();
    let mut bystander = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let bystander_pid = i32::try_from(bystander.id()).unwrap();
    let wrong_start = rundir::start_time(bystander_pid).unwrap() + 1;
    let started = started_of(&fake.reaper, (bystander_pid, wrong_start));
    let sandbox = pinned_then_cleaned(&started).await.unwrap();
    assert!(sandbox.inner.get().is_none());
    assert!(endpoint::exited(&sandbox.reaper.pidfd));
    assert!(pid_alive(bystander_pid), "an unrelated process was killed");
    bystander.kill().unwrap();
    bystander.wait().unwrap();
    let _ = fake.reaper.wait();
    let _ = rustix::process::kill_process(
        rustix::process::Pid::from_raw(fake.inner_pid).unwrap(),
        rustix::process::Signal::KILL,
    );
}

/// A Hakoniwa runner that names no server fails the start, the reaper attached and waited for.
#[tokio::test]
async fn a_start_without_a_reported_server_fails_with_the_reaper_waited_for() {
    let mut fake = fake_sandbox();
    let mut started = started_of(&fake.reaper, (0, 0));
    started.inner_pid = None;
    started.inner_start = None;
    let error = pinned_then_cleaned(&started).await.unwrap_err();
    assert!(error.contains("not reported"), "{error}");
    let _ = fake.reaper.wait();
    let _ = rustix::process::kill_process(
        rustix::process::Pid::from_raw(fake.inner_pid).unwrap(),
        rustix::process::Signal::KILL,
    );
}

fn emfile() -> std::io::Error {
    std::io::Error::from_raw_os_error(libc::EMFILE)
}

#[test]
fn a_server_is_gone_only_when_it_is_proven_and_unverified_when_its_start_cannot_be_read() {
    let mut sleeper = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let pid = i32::try_from(sleeper.id()).unwrap();
    let start = rundir::start_time(pid).unwrap();
    // The reported start time, read: ours.
    assert!(matches!(
        endpoint::open_inner_with(pid, start, |_| Ok(start)),
        Ok(endpoint::Inner::Ours(_))
    ));
    // Another start time: the pid was reused, so the reported process is gone.
    assert!(matches!(
        endpoint::open_inner_with(pid, start, |_| Ok(start + 1)),
        Ok(endpoint::Inner::Gone)
    ));
    // The process vanished under the read (`ENOENT`, `ESRCH`): gone.
    for errno in [libc::ENOENT, libc::ESRCH] {
        assert!(matches!(
            endpoint::open_inner_with(pid, start, |_| Err(std::io::Error::from_raw_os_error(
                errno
            ))),
            Ok(endpoint::Inner::Gone)
        ));
    }
    // Any other failure (no descriptors, no permission, an unreadable line) proves nothing:
    // the pidfd is kept, as unverified.
    for error in [
        emfile as fn() -> std::io::Error,
        || std::io::Error::from_raw_os_error(libc::EACCES),
        || std::io::Error::new(std::io::ErrorKind::InvalidData, "garbled"),
    ] {
        match endpoint::open_inner_with(pid, start, |_| Err(error())) {
            Ok(endpoint::Inner::Unverified(watched)) => assert!(watched.alive()),
            other => panic!("{other:?}"),
        }
    }
    sleeper.kill().unwrap();
    sleeper.wait().unwrap();
}

/// A live server whose start time could not be read is waited for, never signalled: no exit is
/// published while it runs, and the watcher goes on once it ends by itself.
#[tokio::test]
async fn an_unverified_live_server_keeps_the_exit_unpublished_and_is_never_signalled() {
    let mut runner = std::process::Command::new("true").spawn().unwrap();
    let runner_watch = Watched::open(i32::try_from(runner.id()).unwrap()).unwrap();
    let mut reaper_process = std::process::Command::new("true").spawn().unwrap();
    let reaper_watch = Watched::open(i32::try_from(reaper_process.id()).unwrap()).unwrap();
    let mut stranger = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let stranger_pid = i32::try_from(stranger.id()).unwrap();
    let pinned = match endpoint::open_inner_with(stranger_pid, 1, |_| Err(emfile())).unwrap() {
        endpoint::Inner::Unverified(watched) => watched,
        other => panic!("{other:?}"),
    };
    let sandbox = Arc::new(Sandbox {
        reaper: reaper_watch,
        pid_namespace: 0,
        inner: std::sync::OnceLock::from(endpoint::InnerPin {
            watched: pinned,
            verified: false,
        }),
    });
    reaper_process.wait().unwrap();
    let _ = runner.wait();
    let (sender, mut receiver) = watch::channel(None);
    tokio::spawn(watch_exit(
        runner,
        runner_watch,
        Some(sandbox),
        sender,
        || {},
    ));
    // Past the grace in which a verified one would have been signalled (and then some).
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert!(
        receiver.borrow().is_none(),
        "the exit was published with the server running"
    );
    assert!(
        pid_alive(stranger_pid),
        "an unverified process was signalled"
    );
    // It ends by itself: now the exit is published.
    stranger.kill().unwrap();
    stranger.wait().unwrap();
    tokio::time::timeout(Duration::from_secs(10), receiver.wait_for(Option::is_some))
        .await
        .unwrap()
        .unwrap();
}

/// What `pin_or_end` makes of a reaper that no longer exists: only a runner that
/// said it had ended (`ended`) makes that acceptable; a missing reaper is otherwise an error,
/// never proof that the server is gone.
#[tokio::test]
async fn a_missing_reaper_fails_the_start_unless_the_runner_said_it_had_ended() {
    let setup = setup(MODEL, "fake");
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    let run = launch.0.run_dir().unwrap();
    let generation = Generation::create(&run, "9").unwrap();
    let mut gone = std::process::Command::new("true").spawn().unwrap();
    let reaper_pid = i32::try_from(gone.id()).unwrap();
    gone.wait().unwrap();
    let runner = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let work = Work::new();
    let mut owned = Owned::new(runner, work.enter());
    let me = i32::try_from(std::process::id()).unwrap();
    let mut started = Started {
        child_pid: reaper_pid,
        pid_namespace: endpoint::NAMESPACE_UNREPORTED,
        inner_pid: None,
        inner_start: None,
        ended: false,
    };
    let refused = Real::pin_or_end(&mut owned, &started, me, &generation);
    assert!(
        matches!(&refused, Err(Fail::Other(why)) if why.contains("reaper")),
        "{:?}",
        refused.map(|_| ())
    );
    started.ended = true;
    let (sandbox, pinned, _) = Real::pin_or_end(&mut owned, &started, me, &generation).unwrap();
    assert!(sandbox.is_none() && pinned.is_none());
    drop(owned);
    tokio::time::timeout(Duration::from_secs(10), work.settle())
        .await
        .unwrap();
}
#[tokio::test]
async fn a_delayed_marker_report_is_required_before_readiness() {
    let marker = MarkerState::new();
    let delayed = Arc::clone(&marker);
    let publish = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        delayed.set_landlock(Some(super::super::hako::Landlock::Full { abi: 11 }));
        delayed.set(Marker::Executed);
    });
    assert_eq!(
        marker.require_landlock(Duration::from_secs(1)).await,
        Ok(())
    );
    publish.await.unwrap();
    assert_eq!(
        marker.landlock(),
        Some(super::super::hako::Landlock::Full { abi: 11 })
    );
    assert!(
        MarkerState::new()
            .require_landlock(Duration::from_millis(10))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn readiness_accepts_partial_and_unavailable_but_never_a_finished_missing_report() {
    for report in [
        super::super::hako::Landlock::Partial { abi: 3 },
        super::super::hako::Landlock::Unavailable,
    ] {
        let marker = MarkerState::new();
        marker.set_landlock(Some(report));
        marker.set(Marker::Executed);
        assert_eq!(
            marker.require_landlock(Duration::from_secs(1)).await,
            Ok(())
        );
    }
    for outcome in [Marker::Absent, Marker::Unknown, Marker::Executed] {
        let marker = MarkerState::new();
        marker.set(outcome);
        let error = marker
            .require_landlock(Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(error.contains("did not report"), "{error}");
    }
}

#[tokio::test]
async fn a_retry_that_fails_before_the_inspection_carries_no_earlier_hint() {
    let setup = setup(MODEL, "fake");
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    let old = "llama-server may need libgomp.so.1: from an earlier verification";
    let set = |hint: Option<&str>| {
        launch.0.library_hints.lock().unwrap()[0] = hint.map(str::to_owned);
    };
    // A start refused before any verification (level 3 is not started in this release).
    set(Some(old));
    let refused = Real::with_build(setup.settings.clone(), Some(setup.build.clone()));
    let refused = Launch(Arc::new(refused));
    refused.0.library_hints.lock().unwrap()[0] = Some(old.into());
    let _ = refused.start(1, Level::Unconfined, Path::Cpu).await;
    assert_eq!(refused.failure_hint(Path::Cpu), None);
    // A verification that refuses the runtime folder before reaching the library check.
    set(Some(old));
    std::fs::set_permissions(
        setup.dir.path().join("runtime"),
        std::fs::Permissions::from_mode(0o775),
    )
    .unwrap();
    let ticket = launch.0.work.enter();
    assert!(launch.0.verify(Path::Cpu, Level::Sandbox, &ticket).is_err());
    assert_eq!(launch.failure_hint(Path::Cpu), None);
    // A probe (the GPU listing and the level probes verify too) starts from nothing as well.
    set(Some(old));
    let _ = launch.probe(Path::Cpu).await;
    assert_eq!(launch.failure_hint(Path::Cpu), None);
}

#[tokio::test]
async fn a_selected_model_is_hashed_and_verified_against_its_own_recorded_hash() {
    let setup = setup(MODEL, "fake");
    let launch = launch(&setup);
    // Hashing reports the file's SHA-256 and size (the trial records them).
    let file = launch.hash_model("m.gguf").await.unwrap();
    assert_eq!(file.sha256, sha(MODEL));
    assert_eq!(file.size, MODEL.len() as u64);
    let other: &[u8] = b"GGUF another synthetic model";
    let path = setup.dir.path().join("models/n.gguf");
    std::fs::write(&path, other).unwrap();
    let recorded = launch.hash_model("n.gguf").await.unwrap();
    assert_eq!(recorded.sha256, sha(other));
    // Names that are not one file of the models folder cannot be hashed.
    for refused in ["../m.gguf", "missing.gguf", "", "a/b.gguf"] {
        assert!(launch.hash_model(refused).await.is_err(), "{refused:?}");
    }
    // The selected model, not the start flags' one, is what the next start verifies.
    let candidate = crate::assistant::runtime::models::Model {
        spec: "local:n.gguf".into(),
        name: "n.gguf".into(),
        sha256: recorded.sha256.clone(),
        alias: "other".into(),
        max_tokens: 256,
        size: Some(recorded.size),
    };
    launch.select(&candidate);
    assert_eq!(launch.0.chosen().alias, "other");
    let ticket = launch.0.work().enter();
    let checked = launch.0.verify(Path::Cpu, Level::Sandbox, &ticket).unwrap();
    assert_eq!(checked.model.name, "n.gguf");
    assert_eq!(hex::encode(checked.model.sha256), recorded.sha256);
    // The file changes after its hash was recorded: the refusal is "the model file changed",
    // which the manager does not retry.
    std::fs::write(&path, b"GGUF swapped after the trial").unwrap();
    let Err(why) = launch.0.verify(Path::Cpu, Level::Sandbox, &ticket) else {
        panic!("a changed model file must refuse the start")
    };
    assert!(why.starts_with("the model file changed"), "{why}");
    assert!(why.starts_with(manager::MODEL_CHANGED));
    // Any other model failure keeps its own wording.
    std::fs::remove_file(&path).unwrap();
    let Err(why) = launch.0.verify(Path::Cpu, Level::Sandbox, &ticket) else {
        panic!("a missing model file must refuse the start")
    };
    assert_eq!(why, "the model: n.gguf is missing");
}
