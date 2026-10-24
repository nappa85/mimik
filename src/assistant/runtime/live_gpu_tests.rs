//! The GPU path live.
//!
//! - `live_fake_gpu_*` (host, bubblewrap, no GPU): the fake server (`MIMIK_LIVE_FAKE_SERVER`)
//!   as both builds and `/dev/null` standing in for a render node of a fake sysfs tree, so
//!   the sandbox wiring (one node bound per listing, the probe opening the node, `--device`
//!   and the offload check, the fallback) runs without touching a GPU.
//! - `live_gpu_*` (the dev host's GPU): the pinned Vulkan build and the real model. They
//!   (they use Mimik's release binary, `MIMIK_LIVE_MIMIK`, as runner, probe and shim) refuse to
//!   run unless `MIMIK_LIVE_GPU=1` is set and the amdgpu fan reads manual
//!   (`pwm1_enable` 1, `pwm1` at least 230), or `MIMIK_LIVE_GPU_SHORT=1` is set: run them
//!   only through `tools/assist_gpu_live.py`, which also enforces the machine schedule and the
//!   thermal limits, and sets the short variable only for its fixed list of short tests
//!   (automatic fan allowed, junction limit 75 °C). `live_gpu_landlock_*` needs the
//!   `MIMIK_LIVE_MIMIK` binary built with `sandbox-hakoniwa`.
use super::super::super::device::tests::Tree;
use super::super::super::manager::{self, State};
use super::*;
use crate::assistant::jobs::Assistant;

/// Both builds of the fake server, the device choice `auto` over `roots`.
fn fake_gpu_build(scratch: &std::path::Path, alias: &str, roots: Roots) -> Arc<Real> {
    fake_gpu_build_with(scratch, alias, roots, &[])
}

/// [`fake_gpu_build`] with `extra` files `(name, bytes)` in the Vulkan build too.
fn fake_gpu_build_with(
    scratch: &std::path::Path,
    alias: &str,
    roots: Roots,
    extra: &[(&str, Vec<u8>)],
) -> Arc<Real> {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;
    let cpu = fake_build(scratch, alias);
    let server = std::fs::read(env("MIMIK_LIVE_FAKE_SERVER")).unwrap();
    let vulkan = scratch.join("runtime/vulkan");
    std::fs::create_dir_all(&vulkan).unwrap();
    std::fs::write(vulkan.join("llama-server"), &server).unwrap();
    std::fs::set_permissions(
        vulkan.join("llama-server"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let file = super::super::super::manifest::File {
        name: "llama-server".into(),
        sha256: hex::encode(Sha256::digest(&server)),
        size: server.len() as u64,
        mode: "0755".into(),
    };
    let mut vulkan_files = vec![file.clone()];
    for (name, bytes) in extra {
        std::fs::write(vulkan.join(name), bytes).unwrap();
        std::fs::set_permissions(vulkan.join(name), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        vulkan_files.push(super::super::super::manifest::File {
            name: (*name).into(),
            sha256: hex::encode(Sha256::digest(bytes)),
            size: bytes.len() as u64,
            mode: "0755".into(),
        });
    }
    let build = |path: &str| super::super::super::manifest::Build {
        path: path.into(),
        directory: path.into(),
        server: "llama-server".into(),
        files: if path == "vulkan" {
            vulkan_files.clone()
        } else {
            vec![file.clone()]
        },
    };
    let mut settings = cpu.settings.clone();
    settings.device = Choice::Auto;
    settings.roots = roots;
    Arc::new(Real::with_builds(
        settings,
        Some(build("cpu")),
        Some(build("vulkan")),
    ))
}

/// The manager over `real` on the GPU path, until `done` holds (two minutes at most).
async fn run_until(
    real: &Arc<Real>,
    max_tokens: u32,
    done: impl Fn(&manager::Status) -> bool,
) -> manager::Handle {
    run_with(&Assistant::new(), real, max_tokens, done).await
}

/// As [`run_until`], over `assistant`.
async fn run_with(
    assistant: &Arc<Assistant>,
    real: &Arc<Real>,
    max_tokens: u32,
    done: impl Fn(&manager::Status) -> bool,
) -> manager::Handle {
    let handle = manager::spawn(
        Arc::clone(assistant),
        Arc::new(Launch(Arc::clone(real))),
        manager::Config {
            path: Path::Gpu,
            max_tokens,
            levels: manager::USED_LEVELS,
        },
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    while !done(&handle.status()) {
        assert!(
            std::time::Instant::now() < deadline,
            "{:?}",
            handle.status()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    handle
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: bubblewrap, fake server (MIMIK_LIVE_*)"]
async fn live_fake_gpu_each_node_is_listed_alone_and_the_start_uses_the_listed_device() {
    let scratch = scratch();
    let tree = Tree::new(&[
        ("0000:0d:00.0", 129, Some(8 << 30)),
        ("0000:0c:00.0", 128, Some(8 << 30)),
    ]);
    let real = fake_gpu_build(scratch.path(), "fake", tree.roots());
    let handle = run_until(&real, 64, |status| status.state == State::Ready).await;
    let status = handle.status();
    // Each sandboxed listing saw exactly its one node; the identical cards were told apart
    // by PCI address and the lower one taken; every layer went to the listed device.
    assert_eq!(status.path, Path::Gpu, "{status:?}");
    assert_eq!(status.gpu_note, None, "{status:?}");
    assert!(status.device_notes.is_empty(), "{status:?}");
    let device = status.device.clone().unwrap();
    assert_eq!(device.pci, "0000:0c:00.0");
    assert_eq!(device.name, "Vulkan0");
    assert_eq!(status.budgets.unwrap().bounds.bytes, 4096);
    assert_eq!(status.level, Some(Level::Bubblewrap));
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: bubblewrap, fake server (MIMIK_LIVE_*)"]
async fn live_fake_gpu_a_partial_offload_ends_on_the_cpu_saying_why() {
    let scratch = scratch();
    let tree = Tree::new(&[("0000:0c:00.0", 128, Some(8 << 30))]);
    let real = fake_gpu_build(scratch.path(), "partial-offload", tree.roots());
    let handle = run_until(&real, 64, |status| {
        status.state == State::Ready && status.path == Path::Cpu
    })
    .await;
    let status = handle.status();
    assert!(
        status
            .gpu_note
            .as_deref()
            .is_some_and(|why| why.contains("only 30 of 41 layers")),
        "{status:?}"
    );
    assert_eq!(status.failures, 0);
    assert_eq!(status.budgets.unwrap().bounds.bytes, 1024);
    handle.shutdown().await;
}

/// A Vulkan backend (not in the server's own dependencies: the real build loads
/// `libggml-vulkan.so` at run time) that needs a library the host lacks: found by the real
/// inspection of the build's files, reported through the GPU to CPU fallback, and the CPU path
/// runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: bubblewrap, fake server (MIMIK_LIVE_*)"]
async fn live_fake_gpu_a_missing_vulkan_library_falls_back_to_the_cpu_saying_which() {
    let scratch = scratch();
    let tree = Tree::new(&[("0000:0c:00.0", 128, Some(8 << 30))]);
    let backend = super::super::libs::tests::elf_needing(&["libvulkan-not-installed.so.1"]);
    let real = fake_gpu_build_with(
        scratch.path(),
        "missing-vulkan",
        tree.roots(),
        &[("libggml-vulkan.so", backend)],
    );
    let handle = run_until(&real, 64, |status| {
        status.state == State::Ready && status.path == Path::Cpu
    })
    .await;
    let status = handle.status();
    assert!(
        status
            .gpu_note
            .as_deref()
            .is_some_and(|why| why.contains("needs libvulkan-not-installed.so.1")),
        "{status:?}"
    );
    assert_eq!(status.failures, 0, "{status:?}");
    assert_eq!(status.device, None, "{status:?}");
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: bubblewrap, fake server (MIMIK_LIVE_*)"]
async fn live_fake_gpu_no_render_node_runs_the_cpu_path() {
    let scratch = scratch();
    let tree = Tree::new(&[]);
    let real = fake_gpu_build(scratch.path(), "fake", tree.roots());
    let handle = run_until(&real, 64, |status| {
        status.state == State::Ready && status.path == Path::Cpu
    })
    .await;
    assert_eq!(
        handle.status().gpu_note.as_deref(),
        Some("no render node of a PCI GPU was found")
    );
    handle.shutdown().await;
}

/// The tests that may run on the automatic fan (the same list as `SHORT_TESTS` in
/// `tools/assist_gpu_live.py`, which a Python test compares with this one).
const SHORT_TESTS: [&str; 4] = [
    "live_gpu_the_rx6650_reaches_ready_with_gpu_budgets",
    "live_gpu_killing_llama_server_mid_job_ends_the_job_neutral_and_runs_on_the_cpu",
    "live_gpu_killing_the_whole_sandbox_ends_on_the_cpu_saying_why",
    "live_gpu_landlock_the_rx6650_reaches_ready_under_hakoniwa",
];

/// Whether `test` runs in short mode (`short` is the variable's value): the automatic fan is
/// then allowed, only for a test on the short list.
///
/// # Panics
///
/// Short mode was asked for and `test` is not on the short list.
fn short_mode(short: Option<&str>, test: &str) -> bool {
    if short != Some("1") {
        return false;
    }
    assert!(
        SHORT_TESTS.contains(&test),
        "refusing: {test} is not on the short list (tools/assist_gpu_live.py SHORT_TESTS); it needs the manual fan"
    );
    true
}

#[test]
fn short_mode_refuses_a_test_that_is_not_on_the_short_list() {
    for name in SHORT_TESTS {
        assert!(short_mode(Some("1"), name));
        assert!(!short_mode(None, name));
        assert!(!short_mode(Some("0"), name));
    }
    for name in ["live_gpu_a_sustained_benchmark", "live_fake_gpu_x", ""] {
        assert!(
            std::panic::catch_unwind(|| short_mode(Some("1"), name)).is_err(),
            "{name}"
        );
        // Not in short mode the list does not matter (the manual fan is checked instead).
        assert!(!short_mode(None, name));
    }
}

/// Refuses unless the live GPU run was asked for and the amdgpu fan reads manual at 230 or
/// more (the manual setting resets on every reboot; on the automatic curve the junction
/// reached 85 °C in 22 s at a sustained 143 W, 2026-10-20). A short test run by the tool's
/// short mode (`MIMIK_LIVE_GPU_SHORT=1`: automatic fan, the tool's 75 °C limit) needs no
/// manual fan.
fn gpu_allowed(test: &str) {
    assert_eq!(
        std::env::var("MIMIK_LIVE_GPU").as_deref(),
        Ok("1"),
        "refusing: run the GPU checks only through tools/assist_gpu_live.py"
    );
    if short_mode(std::env::var("MIMIK_LIVE_GPU_SHORT").ok().as_deref(), test) {
        return;
    }
    let chip = std::fs::read_dir("/sys/class/hwmon")
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            std::fs::read_to_string(path.join("name")).is_ok_and(|name| name.trim() == "amdgpu")
        })
        .expect("refusing: no amdgpu fan to check");
    let read = |name: &str| -> u32 {
        std::fs::read_to_string(chip.join(name))
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(0)
    };
    assert!(
        read("pwm1_enable") == 1 && read("pwm1") >= 230,
        "refusing: the GPU fan is not manual at 230 or more"
    );
}

/// The real runtime (`MIMIK_LIVE_RUNTIME` with `cpu/` and `vulkan/`) and the default model
/// (`MIMIK_LIVE_GPU_MODELS`, a 0700 folder holding it).
fn real_gpu(scratch: &std::path::Path) -> Arc<Real> {
    real_gpu_on(scratch, Backend::Bwrap)
}

/// [`real_gpu`] on `backend`.
fn real_gpu_on(scratch: &std::path::Path, backend: Backend) -> Arc<Real> {
    let mut settings = settings(scratch, "probe-inner", Vec::new());
    settings.models_dir = env("MIMIK_LIVE_GPU_MODELS");
    settings.model = crate::assistant::model::local_name(crate::assistant::DEFAULT_MODEL)
        .unwrap()
        .to_owned();
    settings.model_sha256 = crate::assistant::DEFAULT_MODEL_SHA256.to_owned();
    settings.alias = "granite".into();
    settings.ctx_size = CTX_SIZE;
    // The CPU fallback is measured at the ASSIST-41 default (physical cores minus one, 5
    // here); at 2 threads it is "too slow" (docs/results.md).
    settings.threads = 5;
    settings.device = Choice::Auto;
    settings.backend = backend;
    settings.roots = Roots::default();
    // The real code path: Mimik's own binary as the runner, the probe and the shim (the test
    // binary cannot take the probe's `--render` option).
    let mimik = env("MIMIK_LIVE_MIMIK");
    settings.runner.program.clone_from(&mimik);
    settings.runner.env.clear();
    settings.runner.args = RunnerCommand::mimik().unwrap().args;
    settings.inner = InnerCommand {
        executable: mimik,
        ..InnerCommand::mimik().unwrap()
    };
    real(settings)
}

/// The measurement as one JSON line for `docs/results.md`.
fn report(status: &manager::Status) {
    let calibration = status.calibration.as_ref().map(|c| {
        json!({"prefix_tokens": c.prefix_tokens, "density": c.density,
            "samples": c.samples.iter().map(|s| json!({"wall_ms": s.wall_ms,
                "prompt_n": s.prompt_n, "prompt_ms": s.prompt_ms, "cached_n": s.cached_n,
                "predicted_n": s.predicted_n, "predicted_ms": s.predicted_ms,
                "filler_tokens": s.filler_tokens})).collect::<Vec<_>>()})
    });
    let budgets = status.budgets.map(|b| {
        json!({"bytes": b.bounds.bytes, "tokens": b.bounds.tokens, "run_s": b.run.as_secs(),
            "depth": b.depth, "queue_s": b.queue_wait.as_secs()})
    });
    println!(
        "GPU-LIVE {}",
        json!({"path": format!("{:?}", status.path), "device": status.device.as_ref().map(|d| &d.description),
            "gpu_note": status.gpu_note, "calibration": calibration, "budgets": budgets})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live GPU: only through tools/assist_gpu_live.py (fan manual, schedule)"]
async fn live_gpu_the_rx6650_reaches_ready_with_gpu_budgets() {
    gpu_allowed("live_gpu_the_rx6650_reaches_ready_with_gpu_budgets");
    let scratch = scratch();
    let real = real_gpu(scratch.path());
    let handle = run_until(&real, 352, |status| {
        status.state == State::Ready || matches!(status.state, State::Unavailable(_))
    })
    .await;
    let status = handle.status();
    report(&status);
    assert_eq!(status.path, Path::Gpu, "{status:?}");
    let device = status.device.clone().unwrap();
    assert!(!device::is_software(&device.description));
    let budgets = status.budgets.unwrap();
    assert!(budgets.run <= Duration::from_secs(60));
    handle.shutdown().await;
}

/// The GPU under Hakoniwa with the Landlock layer in force (owed since ASSIST-49; the rule
/// table is unit-tested, this shows that Vulkan discovery, the render node and the driver data
/// work under it). A short test.
#[cfg(feature = "sandbox-hakoniwa")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live GPU: only through tools/assist_gpu_live.py --short (schedule, thermal guard)"]
async fn live_gpu_landlock_the_rx6650_reaches_ready_under_hakoniwa() {
    gpu_allowed("live_gpu_landlock_the_rx6650_reaches_ready_under_hakoniwa");
    let scratch = scratch();
    let real = real_gpu_on(scratch.path(), Backend::Hakoniwa);
    let handle = run_until(&real, 352, |status| {
        status.state == State::Ready || matches!(status.state, State::Unavailable(_))
    })
    .await;
    let status = handle.status();
    report(&status);
    assert_eq!(status.path, Path::Gpu, "{status:?}");
    assert!(!device::is_software(
        &status.device.clone().unwrap().description
    ));
    assert!(
        matches!(
            status.landlock,
            Some(super::super::super::hako::Landlock::Full { .. })
        ),
        "{status:?}"
    );
    handle.shutdown().await;
}

/// A "heal 15" interpretation of the sheet's Current HP field.
fn heal_draft() -> crate::assistant::request::Draft {
    crate::assistant::request::Draft {
        phrase: "heal 15".into(),
        target: crate::assistant::contract::EnvelopeTarget {
            alias: crate::assistant::contract::TARGET_ALIAS,
            id: "hp".into(),
            label: Some("Current HP".into()),
            value: Some(84),
        },
        target_address: "field:hp".into(),
        items: Vec::new(),
    }
}

/// Starts a job of account 1 on sheet 1 at `revision`.
fn start_job(assistant: &Arc<Assistant>, revision: i64) -> crate::assistant::jobs::JobId {
    let origin = crate::assistant::jobs::Origin {
        sheet: 1,
        target: "field:hp".into(),
        revision,
    };
    let registration = assistant.register(1, revision);
    assistant
        .start(&registration, 1, origin, heal_draft())
        .expect("the job starts")
        .0
}

/// Polls job `id` until `done` holds for its poll (`within` at most).
async fn poll_until(
    assistant: &Arc<Assistant>,
    id: crate::assistant::jobs::JobId,
    within: Duration,
    done: impl Fn(&crate::assistant::jobs::Poll) -> bool,
) -> crate::assistant::jobs::Poll {
    let deadline = std::time::Instant::now() + within;
    loop {
        let poll = assistant.poll(1, 1, id);
        if done(&poll) {
            return poll;
        }
        assert!(std::time::Instant::now() < deadline, "{poll:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The runner's PID from the generation's pidfile (the entry: generation, then the PID,
/// `rundir.rs`).
fn runner_pid(scratch: &std::path::Path) -> i32 {
    let pidfile = std::fs::read_to_string(scratch.join("data/run/assistant.pid")).unwrap();
    pidfile
        .lines()
        .nth(1)
        .and_then(|pid| pid.trim().parse().ok())
        .expect("the pidfile names the runner")
}

/// The descendants of `runner` that are the GPU build's llama-server (by its command line:
/// the executable's file name and the `--device` flag only the GPU path passes).
fn llama_servers(runner: i32) -> Vec<i32> {
    super::descendants(runner)
        .into_iter()
        .filter(|pid| {
            let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
                return false;
            };
            let args: Vec<&[u8]> = raw.split(|byte| *byte == 0).collect();
            let named = args.first().is_some_and(|argv0| {
                std::path::Path::new(
                    <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(argv0),
                )
                .file_name()
                    == Some(std::ffi::OsStr::new("llama-server"))
            });
            named && args.iter().any(|arg| *arg == b"--device")
        })
        .collect()
}

fn kill(pid: i32) {
    let _ = rustix::process::kill_process(
        rustix::process::Pid::from_raw(pid).unwrap(),
        rustix::process::Signal::KILL,
    );
}

/// What the CPU path must then show: ready at level 1 on the CPU budgets, the GPU failure
/// said and not counted.
fn assert_cpu_after_gpu_failure(status: &manager::Status) {
    report(status);
    assert_eq!(status.path, Path::Cpu, "{status:?}");
    assert_eq!(status.level, Some(Level::Bubblewrap), "{status:?}");
    assert!(status.gpu_note.is_some(), "{status:?}");
    assert_eq!(status.failures, 0, "{status:?}");
    assert_eq!(status.device, None, "{status:?}");
    assert_eq!(status.budgets.unwrap().bounds.bytes, 1024);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live GPU: only through tools/assist_gpu_live.py (fan manual, schedule)"]
async fn live_gpu_killing_llama_server_mid_job_ends_the_job_neutral_and_runs_on_the_cpu() {
    use crate::assistant::jobs::{Answer, Poll};
    gpu_allowed("live_gpu_killing_llama_server_mid_job_ends_the_job_neutral_and_runs_on_the_cpu");
    let scratch = scratch();
    let real = real_gpu(scratch.path());
    let assistant = Assistant::new();
    tokio::spawn(Arc::clone(&assistant).work());
    let handle = run_with(&assistant, &real, 352, |status| {
        status.state == State::Ready
    })
    .await;
    assert_eq!(handle.status().path, Path::Gpu);
    // A real job, confirmed executing before anything is killed.
    let job = start_job(&assistant, 5);
    poll_until(&assistant, job, Duration::from_secs(30), |poll| {
        matches!(poll, Poll::Running)
    })
    .await;
    // Only the verified llama-server goes; the rest of the sandbox is noted to be gone later.
    let runner = runner_pid(scratch.path());
    let servers = llama_servers(runner);
    assert_eq!(servers.len(), 1, "exactly one llama-server: {servers:?}");
    let sandbox: Vec<i32> = super::descendants(runner);
    kill(servers[0]);
    // The job ends on the generation change, neutral for the breaker.
    let done = poll_until(&assistant, job, Duration::from_secs(120), |poll| {
        matches!(poll, Poll::Done(_))
    })
    .await;
    assert_eq!(done, Poll::Done(Answer::Restarted));
    assert!(assistant.breaker_closed());
    // The old sandbox is gone before the CPU path launches anything.
    let deadline = std::time::Instant::now() + Duration::from_secs(600);
    while handle.status().path != Path::Cpu {
        assert!(
            std::time::Instant::now() < deadline,
            "{:?}",
            handle.status()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        sandbox.iter().all(|pid| !super::alive(*pid)),
        "the GPU sandbox was still running when the CPU path began"
    );
    while handle.status().state != State::Ready {
        assert!(
            std::time::Instant::now() < deadline,
            "{:?}",
            handle.status()
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_cpu_after_gpu_failure(&handle.status());
    // A following job on the CPU path works.
    let next = start_job(&assistant, 6);
    let answer = poll_until(&assistant, next, Duration::from_secs(180), |poll| {
        matches!(poll, Poll::Done(_))
    })
    .await;
    assert!(
        matches!(
            answer,
            Poll::Done(Answer::Options(_) | Answer::Nothing | Answer::Readings(_))
        ),
        "{answer:?}"
    );
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live GPU: only through tools/assist_gpu_live.py (fan manual, schedule)"]
async fn live_gpu_killing_the_whole_sandbox_ends_on_the_cpu_saying_why() {
    gpu_allowed("live_gpu_killing_the_whole_sandbox_ends_on_the_cpu_saying_why");
    let scratch = scratch();
    let real = real_gpu(scratch.path());
    let handle = run_until(&real, 352, |status| status.state == State::Ready).await;
    assert_eq!(handle.status().path, Path::Gpu);
    // The sandbox's processes are the runner's descendants.
    for pid in super::descendants(runner_pid(scratch.path())) {
        kill(pid);
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(600);
    while !(handle.status().state == State::Ready && handle.status().path == Path::Cpu) {
        assert!(
            std::time::Instant::now() < deadline,
            "{:?}",
            handle.status()
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_cpu_after_gpu_failure(&handle.status());
    handle.shutdown().await;
}
