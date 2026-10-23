//! The GPU path live.
//!
//! - `live_fake_gpu_*` (host, bubblewrap, no GPU): the fake server (`MIMIK_LIVE_FAKE_SERVER`)
//!   as both builds and `/dev/null` standing in for a render node of a fake sysfs tree, so
//!   the sandbox wiring (one node bound per listing, the probe opening the node, `--device`
//!   and the offload check, the fallback) runs without touching a GPU.
//! - `live_gpu_*` (the dev host's GPU): the pinned Vulkan build and the real model. They
//!   refuse to run unless `MIMIK_LIVE_GPU=1` is set and the amdgpu fan reads manual
//!   (`pwm1_enable` 1, `pwm1` at least 230): run them only through
//!   `tools/assist_gpu_live.py`, which also enforces the machine schedule and the thermal
//!   limits.
use super::super::super::device::tests::Tree;
use super::super::super::manager::{self, State};
use super::*;
use crate::assistant::jobs::Assistant;

/// Both builds of the fake server, the device choice `auto` over `roots`.
fn fake_gpu_build(scratch: &std::path::Path, alias: &str, roots: Roots) -> Arc<Real> {
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
    let build = |path: &str| super::super::super::manifest::Build {
        path: path.into(),
        directory: path.into(),
        server: "llama-server".into(),
        files: vec![file.clone()],
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
    let handle = manager::spawn(
        Assistant::new(),
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

/// Refuses unless the live GPU run was asked for and the amdgpu fan reads manual at 230 or
/// more (the manual setting resets on every reboot; on the automatic curve the junction
/// reached 85 °C in 22 s, 2026-10-20).
fn gpu_allowed() {
    assert_eq!(
        std::env::var("MIMIK_LIVE_GPU").as_deref(),
        Ok("1"),
        "refusing: run the GPU checks only through tools/assist_gpu_live.py"
    );
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
    let mut settings = settings(scratch, "probe-inner", Vec::new());
    settings.models_dir = env("MIMIK_LIVE_GPU_MODELS");
    settings.model = crate::assistant::model::local_name(crate::assistant::DEFAULT_MODEL)
        .unwrap()
        .to_owned();
    settings.model_sha256 = crate::assistant::DEFAULT_MODEL_SHA256.to_owned();
    settings.alias = "granite".into();
    settings.ctx_size = CTX_SIZE;
    settings.threads = 2;
    settings.device = Choice::Auto;
    settings.roots = Roots::default();
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
    gpu_allowed();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live GPU: only through tools/assist_gpu_live.py (fan manual, schedule)"]
async fn live_gpu_killing_the_child_mid_job_ends_on_the_cpu_saying_why() {
    gpu_allowed();
    let scratch = scratch();
    let real = real_gpu(scratch.path());
    let handle = run_until(&real, 352, |status| status.state == State::Ready).await;
    assert_eq!(handle.status().path, Path::Gpu);
    // The sandbox's processes are the runner's descendants: kill the llama-server inside.
    let pidfile = std::fs::read_to_string(scratch.path().join("data/run/assistant.pid")).unwrap();
    // The entry: generation, then the runner's PID (`rundir.rs`).
    let runner: i32 = pidfile
        .lines()
        .nth(1)
        .and_then(|pid| pid.trim().parse().ok())
        .expect("the pidfile names the runner");
    for pid in super::descendants(runner) {
        let _ = rustix::process::kill_process(
            rustix::process::Pid::from_raw(pid).unwrap(),
            rustix::process::Signal::KILL,
        );
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
    let status = handle.status();
    report(&status);
    assert!(status.gpu_note.is_some());
    assert_eq!(status.failures, 0);
    assert_eq!(status.budgets.unwrap().bounds.bytes, 1024);
    handle.shutdown().await;
}
