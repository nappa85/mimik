//! The real launcher at level 3 against a fake llama-server (`examples/fake_llama_server.rs`,
//! built by `cargo test --all-targets`), the test binary as the runner and as the probe.
//! Level 1 needs bubblewrap and the pinned build and runs in `live_tests.rs`.
use super::super::drain::Failed;
use super::super::manager::{self, State};
use super::super::manifest::File as Entry;
use super::super::test_helper;
use super::*;
use crate::assistant::jobs::{Assistant, RuntimeState};
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;

const HELPER: &str = "assistant::runtime::launch::tests::helper";
const RUNNER: &str = "assistant::runtime::runner::tests::helper";

/// The probe inside its level: the test binary, its level arguments after `probe-inner`.
#[test]
fn helper() {
    match test_helper::mode().as_deref() {
        Some("probe-inner") => {}
        Some("shim") => std::process::exit(super::super::probe::shim()),
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
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/runtime-tests");
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
        "{} is missing: run the tests with --all-targets",
        path.display()
    );
    path
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
    let dir = scratch();
    let root = dir.path();
    let cpu = root.join("runtime/cpu");
    std::fs::create_dir_all(&cpu).unwrap();
    let server = std::fs::read(fake_server()).unwrap();
    std::fs::write(cpu.join("llama-server"), &server).unwrap();
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
                sha256: sha(&server),
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
            shim_args: {
                let mut args = helper_args(HELPER);
                args.push("shim".into());
                args
            },
            shim_env: vec![(test_helper::VARIABLE.into(), "shim".into())],
        },
    };
    Setup {
        dir,
        settings,
        build,
    }
}

fn launch(setup: &Setup) -> Launch {
    Launch(Arc::new(Real::with_build(
        setup.settings.clone(),
        Some(setup.build.clone()),
    )))
}

const MODEL: &[u8] = b"GGUF a synthetic model";

fn bwrap_installed() -> bool {
    bwrap::PATHS
        .iter()
        .any(|path| std::path::Path::new(path).exists())
}

#[tokio::test]
async fn the_probe_passes_at_level_3_and_names_level_1s_cause() {
    let setup = setup(MODEL, "fake");
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    let levels = launch.probe(Path::Cpu).await;
    assert_eq!(levels[2], (Level::Unconfined, Ok(())), "{levels:?}");
    assert_eq!(levels[1].0, Level::Landlock);
    assert!(levels[1].1.is_err());
    if !bwrap_installed() {
        assert_eq!(
            levels[0],
            (
                Level::Bubblewrap,
                Err("bubblewrap is not installed (/usr/bin/bwrap or /bin/bwrap)".into())
            )
        );
    }
    // The probes' directories are gone; a second probe works the same.
    let run: Vec<_> = std::fs::read_dir(setup.settings.data_dir.join("run"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(run, [rundir::PIDFILE]);
    let again = launch.probe(Path::Cpu).await;
    assert_eq!(again[2], (Level::Unconfined, Ok(())), "{again:?}");
}

fn cmdline(pid: i32) -> String {
    String::from_utf8_lossy(&std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default())
        .replace('\0', " ")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_manager_reaches_ready_and_shutdown_removes_everything() {
    let setup = setup(MODEL, "fake");
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
            max_tokens: 64,
        },
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while assistant.state() != RuntimeState::Ready {
        assert!(
            std::time::Instant::now() < deadline,
            "{:?}",
            handle.status()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let status = handle.status();
    assert_eq!(status.state, State::Ready);
    if !bwrap_installed() {
        assert_eq!(status.level, Some(Level::Unconfined));
    }
    assert!(status.budgets.is_some());
    let run = setup.settings.data_dir.join("run");
    let generation = run.join(status.generation.to_string());
    if status.level == Some(Level::Unconfined) {
        // The token file was deleted once /health answered; the token is in no command line.
        assert!(!generation.join(token::FILE).exists());
        let pidfile = std::fs::read_to_string(run.join(rundir::PIDFILE)).unwrap();
        let pid: i32 = pidfile.lines().nth(1).unwrap().parse().unwrap();
        let line = cmdline(pid);
        assert!(line.contains("--api-key-file"), "{line}");
        assert!(line.contains("/proc/self/fd/8"), "{line}");
        assert!(!line.contains("--api-key "), "{line}");
    }
    assert!(generation.is_dir());
    handle.shutdown().await;
    assert!(!generation.exists());
    assert_eq!(
        std::fs::read_to_string(run.join(rundir::PIDFILE)).unwrap(),
        ""
    );
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
async fn a_port_that_cannot_be_bound_is_tried_three_times() {
    let setup = setup(MODEL, "fail-bind");
    let launch = launch(&setup);
    launch.preflight(Path::Cpu).await.unwrap();
    let child = launch.start(1, Level::Unconfined, Path::Cpu).await.unwrap();
    let fail = tokio::time::timeout(Duration::from_secs(30), child.exited())
        .await
        .unwrap();
    assert!(matches!(fail, Fail::Other(_)), "{fail:?}");
    child.stop().await;
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
fn only_a_missing_shim_marker_makes_a_level_1_exit_a_sandbox_failure() {
    let loaded = Summary {
        failed: Failed {
            model_load: true,
            ..Failed::default()
        },
        ..Summary::default()
    };
    // The shim never reached its exec: the sandbox was not set up.
    assert!(matches!(
        classify(
            Level::Bubblewrap,
            Marker::Absent,
            Summary::default(),
            "exit status: 1"
        ),
        Fail::Sandbox(_)
    ));
    // After the marker, whatever the server printed (a forged "bwrap:" line included, which
    // the drain no longer even recognises) it is a counted failure that keeps the level.
    assert!(matches!(
        classify(
            Level::Bubblewrap,
            Marker::Executed,
            Summary::default(),
            "exit status: 1"
        ),
        Fail::Other(_)
    ));
    assert!(matches!(
        classify(Level::Bubblewrap, Marker::Executed, loaded, "exit status: 1"),
        Fail::Other(why) if why.starts_with("the model could not be loaded")
    ));
    // Not known (a read error, a reader still running): counted, the level kept.
    assert!(matches!(
        classify(
            Level::Bubblewrap,
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
            Level::Bubblewrap,
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
    let kept = retry_probe(Level::Bubblewrap, Duration::ZERO, || {
        calls += 1;
        Probe::Inconclusive("did not report within 30 s".into())
    });
    assert_eq!(kept, Probed::Kept("did not report within 30 s".into()));
    assert_eq!(calls, PROBE_TRIES);
    assert_eq!(kept.eligible(), Ok(()));
    let mut calls = 0;
    let passed = retry_probe(Level::Unconfined, Duration::ZERO, || {
        calls += 1;
        if calls < 2 {
            Probe::Inconclusive("no pidfd".into())
        } else {
            Probe::Passed
        }
    });
    assert_eq!((passed, calls), (Probed::Passed, 2));
    let mut calls = 0;
    let failed = retry_probe(Level::Bubblewrap, Duration::ZERO, || {
        calls += 1;
        Probe::Failed("bwrap: setup failed".into())
    });
    assert_eq!(
        (failed, calls),
        (Probed::Failed("bwrap: setup failed".into()), 1)
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

#[test]
fn a_shim_whose_exec_fails_says_why_after_its_marker() {
    let (read, write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::empty()).unwrap();
    let spec = super::super::probe::ShimSpec {
        marker: std::os::fd::AsRawFd::as_raw_fd(&write),
        argv: vec!["/nonexistent/llama-server".into()],
        env: Vec::new(),
    };
    let output = test_helper::command("assistant::runtime::launch::tests::helper", "shim")
        .env(
            super::super::probe::SHIM_VARIABLE,
            serde_json::to_string(&spec).unwrap(),
        )
        .output()
        .unwrap();
    drop(write);
    assert_eq!(output.status.code(), Some(127));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("mimik shim: cannot execute /nonexistent/llama-server"),
        "{stderr}"
    );
    let mut marker = [0_u8; 5];
    std::fs::File::from(read).read_exact(&mut marker).unwrap();
    assert_eq!(&marker, super::super::probe::MARKER);
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
