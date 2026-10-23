//! The launcher's GPU path without a GPU: the flag vector, the check at the real start, the
//! offload line, discovery over fake sysfs trees with bubblewrap forced unusable, and the
//! levels the GPU path refuses. The live GPU checks are `live_tests.rs` (`live_gpu_*`, run
//! only by `tools/assist_gpu_live.py`).
use super::super::device::tests::Tree;
use super::*;

fn settings(dir: &std::path::Path, device: Choice, roots: Roots) -> Settings {
    Settings {
        data_dir: dir.join("data"),
        models_dir: dir.join("models"),
        runtime_dir: dir.join("runtime"),
        model: "m.gguf".into(),
        model_sha256: "0".repeat(64),
        alias: "m".into(),
        threads: 3,
        ctx_size: 256,
        runner: RunnerCommand::mimik().unwrap(),
        inner: InnerCommand::mimik().unwrap(),
        device,
        roots,
    }
}

fn listed(name: &str, description: &str) -> Listed {
    Listed {
        name: name.into(),
        description: description.into(),
        total_mib: 8176,
        free_mib: 8000,
    }
}

const RX6650: &str = "AMD Radeon RX 6650 XT (RADV NAVI23)";

#[test]
fn the_gpu_flag_vector_offloads_every_layer_on_the_listed_device_and_the_cpu_names_none() {
    let dir = tempfile::tempdir().unwrap();
    let real = Real::new(settings(dir.path(), Choice::Auto, Roots::default()));
    let gpu = real.server_args(
        "/models/model.gguf",
        "/run/llama/llama.sock",
        None,
        "/run/token",
        Some("Vulkan0"),
    );
    let pairs: Vec<(&str, &str)> = gpu
        .windows(2)
        .map(|w| (w[0].as_str(), w[1].as_str()))
        .collect();
    assert!(pairs.contains(&("--n-gpu-layers", "99")));
    assert!(pairs.contains(&("--device", "Vulkan0")));
    assert!(pairs.contains(&("--fit", "off")));
    assert!(pairs.contains(&("--threads", "3")));
    let cpu = real.server_args(
        "/models/model.gguf",
        "/run/llama/llama.sock",
        None,
        "/run/token",
        None,
    );
    assert!(cpu.windows(2).any(|w| w == ["--n-gpu-layers", "0"]));
    assert!(!cpu.iter().any(|arg| arg == "--device"));
    // The rest of the vector is the same on both paths.
    let strip = |args: &[String]| -> Vec<String> {
        let mut out = Vec::new();
        let mut skip = 0;
        for arg in args {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            if arg == "--n-gpu-layers" || arg == "--device" {
                skip = 1;
                continue;
            }
            out.push(arg.clone());
        }
        out
    };
    assert_eq!(strip(&gpu), strip(&cpu));
}

#[test]
fn the_real_start_takes_the_name_from_the_new_listing_and_refuses_a_changed_gpu() {
    let recorded = listed("Vulkan1", RX6650);
    // Enumeration order changed: the same card is now Vulkan0 in its sandbox.
    assert_eq!(
        check_relisted(&recorded, &[listed("Vulkan0", RX6650)]).unwrap(),
        "Vulkan0"
    );
    // Software ICDs are still excluded first.
    assert_eq!(
        check_relisted(
            &recorded,
            &[
                listed("Vulkan0", "llvmpipe (LLVM 19.1.7, 256 bits)"),
                listed("Vulkan1", RX6650)
            ]
        )
        .unwrap(),
        "Vulkan1"
    );
    // Another GPU, none, or two: a GPU failure.
    assert!(
        check_relisted(
            &recorded,
            &[listed("Vulkan0", "AMD Radeon RX 7600 (RADV NAVI33)")]
        )
        .unwrap_err()
        .contains("changed")
    );
    assert!(check_relisted(&recorded, &[]).is_err());
    assert!(
        check_relisted(
            &recorded,
            &[listed("Vulkan0", RX6650), listed("Vulkan1", RX6650)]
        )
        .is_err()
    );
}

#[test]
fn the_gpu_path_needs_every_layer_on_the_gpu() {
    assert_eq!(offload_verdict(Some((41, 41)), false), Some(Ok(())));
    assert!(
        offload_verdict(Some((30, 41)), false)
            .unwrap()
            .unwrap_err()
            .contains("only 30 of 41")
    );
    assert!(offload_verdict(Some((0, 0)), false).unwrap().is_err());
    assert!(offload_verdict(Some((41, 41)), true).unwrap().is_err());
    assert!(offload_verdict(None, true).unwrap().is_err());
    assert_eq!(offload_verdict(None, false), None);
}

#[test]
fn the_cpu_choice_lists_no_device() {
    let dir = tempfile::tempdir().unwrap();
    let tree = Tree::new(&[("0000:0c:00.0", 128, Some(8 << 30))]);
    let real = Arc::new(Real::new(settings(dir.path(), Choice::Cpu, tree.roots())));
    let ticket = real.work().enter();
    assert!(
        real.discover(&ticket)
            .unwrap_err()
            .contains("--assistant-device cpu")
    );
    assert_eq!(real.held().device_notes, Vec::<String>::new());
    assert!(real.held().gpu.is_none());
    assert_eq!(real.probes.load(Ordering::SeqCst), 1, "nothing was spawned");
}

#[test]
fn discovery_names_each_render_node_it_could_not_list() {
    let dir = tempfile::tempdir().unwrap();
    let tree = Tree::new(&[
        ("0000:0c:00.0", 128, Some(8 << 30)),
        ("0000:0d:00.0", 129, Some(8 << 30)),
    ]);
    let settings = settings(dir.path(), Choice::Auto, tree.roots());
    NO_BWRAP.lock().unwrap().push(settings.runtime_dir.clone());
    let real = Arc::new(Real::new(settings));
    let ticket = real.work().enter();
    let why = real.discover(&ticket).unwrap_err();
    assert!(why.contains("0000:0c:00.0 (renderD128)"), "{why}");
    assert!(why.contains("0000:0d:00.0 (renderD129)"), "{why}");
    assert!(why.contains("bubblewrap is not installed"), "{why}");
    assert_eq!(real.held().device_notes.len(), 2);
    assert!(real.held().gpu.is_none());
}

#[test]
fn a_chosen_pci_address_without_a_render_node_is_no_gpu() {
    let dir = tempfile::tempdir().unwrap();
    let tree = Tree::new(&[("0000:0c:00.0", 128, None)]);
    let real = Arc::new(Real::new(settings(
        dir.path(),
        Choice::Pci("0000:0e:00.0".into()),
        tree.roots(),
    )));
    let ticket = real.work().enter();
    assert_eq!(
        real.discover(&ticket).unwrap_err(),
        "no render node of a PCI GPU was found"
    );
}

#[tokio::test]
async fn the_gpu_path_is_started_at_level_1_only() {
    let dir = tempfile::tempdir().unwrap();
    let launch = Launch(Arc::new(
        Real::new(settings(dir.path(), Choice::Auto, Roots::default())).allowing_unconfined(),
    ));
    for level in [Level::Landlock, Level::Unconfined] {
        assert!(
            matches!(
                launch.start_real(1, level, Path::Gpu).await,
                Err(Fail::Sandbox(why)) if why.contains("only at level 1")
            ),
            "{level:?}"
        );
    }
}
