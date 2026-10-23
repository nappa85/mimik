//! Device choice: the render-node scan over fake sysfs trees, the listing format pinned from
//! the pinned build, software ICDs excluded before any count, and the ranking.
use super::*;
use std::os::unix::fs::symlink;

/// `--list-devices` of the pinned Vulkan build in a sandbox with no render node (captured on
/// the dev host, 2026-10-23, stdout byte for byte).
const NONE: &str = "Available devices:\n  (none)\n";

/// One device as the pinned build prints it (`common/arg.cpp`, `common_print_available_devices`:
/// `"  %s: %s (%zu MiB, %zu MiB free)\n"`; RADV's `deviceName` for the dev host's card).
const RX6650: &str = "Available devices:\n  Vulkan0: AMD Radeon RX 6650 XT (RADV NAVI23) (8176 MiB, 7912 MiB free)\n";

/// A software ICD beside a hardware GPU (ggml lists only GPU-type devices by itself; the
/// exclusion by description is Mimik's own, before any count).
const WITH_LAVAPIPE: &str = "Available devices:\n  Vulkan0: llvmpipe (LLVM 19.1.7, 256 bits) (32000 MiB, 32000 MiB free)\n  Vulkan1: AMD Radeon RX 6650 XT (RADV NAVI23) (8176 MiB, 7912 MiB free)\n";

#[test]
fn the_listing_format_is_the_pinned_builds() {
    assert_eq!(parse_listing(NONE).unwrap(), Vec::<Listed>::new());
    assert_eq!(
        parse_listing(RX6650).unwrap(),
        [Listed {
            name: "Vulkan0".into(),
            description: "AMD Radeon RX 6650 XT (RADV NAVI23)".into(),
            total_mib: 8176,
            free_mib: 7912,
        }]
    );
    // Anything else is a changed format, never guessed at.
    for broken in [
        "",
        "  Vulkan0: x (1 MiB, 1 MiB free)\n",
        "Available devices:\n  Vulkan0 x (1 MiB, 1 MiB free)\n",
        "Available devices:\n  Vulkan0: x (1 GiB, 1 GiB free)\n",
        "Available devices:\n  Vulkan 0: x (1 MiB, 1 MiB free)\n",
        "Available devices:\nsomething else\n",
    ] {
        assert!(parse_listing(broken).is_err(), "{broken:?}");
    }
}

#[test]
fn software_icds_are_excluded_before_the_count() {
    let listed = parse_listing(WITH_LAVAPIPE).unwrap();
    assert_eq!(listed.len(), 2);
    let hardware = one_hardware(&listed).unwrap();
    assert_eq!(hardware.name, "Vulkan1");
    for software in [
        "llvmpipe (LLVM 15.0.7, 256 bits)",
        "Lavapipe",
        "SwiftShader Device (Subzero)",
    ] {
        assert!(is_software(software), "{software}");
    }
    assert!(!is_software("AMD Radeon RX 6650 XT (RADV NAVI23)"));
}

#[test]
fn only_llvmpipe_or_nothing_means_no_hardware_gpu() {
    let only = parse_listing(
        "Available devices:\n  Vulkan0: llvmpipe (LLVM 19.1.7, 256 bits) (32000 MiB, 32000 MiB free)\n",
    )
    .unwrap();
    assert!(one_hardware(&only).unwrap_err().contains("no hardware GPU"));
    assert!(
        one_hardware(&parse_listing(NONE).unwrap())
            .unwrap_err()
            .contains("no hardware GPU")
    );
    let two = parse_listing(
        "Available devices:\n  Vulkan0: AMD Radeon RX 6650 XT (RADV NAVI23) (8176 MiB, 7912 MiB free)\n  Vulkan1: AMD Radeon RX 6650 XT (RADV NAVI23) (8176 MiB, 7912 MiB free)\n",
    )
    .unwrap();
    // One node bound must show one GPU: two means the node did not isolate enumeration.
    assert!(one_hardware(&two).unwrap_err().contains("2 hardware GPUs"));
}

#[test]
fn the_device_choice_accepts_auto_cpu_and_a_pci_address() {
    assert_eq!(Choice::parse("auto").unwrap(), Choice::Auto);
    assert_eq!(Choice::parse("cpu").unwrap(), Choice::Cpu);
    assert_eq!(
        Choice::parse("0000:0c:00.0").unwrap(),
        Choice::Pci("0000:0c:00.0".into())
    );
    assert_eq!(
        Choice::parse("0C:00.0").unwrap(),
        Choice::Pci("0000:0c:00.0".into())
    );
    for refused in [
        "",
        "gpu",
        "0000:0c:00",
        "0000:0c:00.8",
        "../x",
        "0000:0c:00.0 ",
    ] {
        assert!(Choice::parse(refused).is_err(), "{refused:?}");
    }
}

/// A fake sysfs and `/dev/dri`: each card as (PCI address, render minor, VRAM bytes).
pub(crate) struct Tree {
    dir: tempfile::TempDir,
}

impl Tree {
    pub(crate) fn new(cards: &[(&str, u32, Option<u64>)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let sys = root.join("sys");
        let dev = root.join("dev/dri");
        std::fs::create_dir_all(sys.join("class/drm")).unwrap();
        std::fs::create_dir_all(sys.join("bus/pci")).unwrap();
        std::fs::create_dir_all(&dev).unwrap();
        for (pci, minor, vram) in cards {
            let device = sys.join("devices/pci0000:00/0000:00:03.1").join(pci);
            let name = format!("renderD{minor}");
            std::fs::create_dir_all(device.join("drm").join(&name)).unwrap();
            symlink("../../../../bus/pci", device.join("subsystem")).unwrap();
            std::fs::write(device.join("class"), "0x030000\n").unwrap();
            std::fs::write(device.join("vendor"), "0x1002\n").unwrap();
            std::fs::write(device.join("device"), "0x73ef\n").unwrap();
            if let Some(vram) = vram {
                std::fs::write(device.join("mem_info_vram_total"), format!("{vram}\n")).unwrap();
            }
            // The character device itself: `/dev/null` stands in (its own numbers).
            let null = rustix::fs::stat("/dev/null").unwrap();
            let (major, null_minor) = (
                rustix::fs::major(null.st_rdev),
                rustix::fs::minor(null.st_rdev),
            );
            std::fs::write(
                device.join("drm").join(&name).join("dev"),
                format!("{major}:{null_minor}\n"),
            )
            .unwrap();
            symlink(
                format!("../../devices/pci0000:00/0000:00:03.1/{pci}/drm/{name}"),
                sys.join("class/drm").join(&name),
            )
            .unwrap();
            symlink(
                format!("../../../{pci}"),
                device.join("drm").join(&name).join("device"),
            )
            .unwrap();
            symlink("/dev/null", dev.join(&name)).unwrap();
        }
        // A card node and a connector are not render nodes.
        std::fs::create_dir_all(sys.join("devices/virtual/drm/card0")).unwrap();
        symlink(
            "../../devices/virtual/drm/card0",
            sys.join("class/drm/card0"),
        )
        .unwrap();
        Self { dir }
    }

    pub(crate) fn roots(&self) -> Roots {
        Roots {
            sys: self.dir.path().join("sys"),
            dev: self.dir.path().join("dev/dri"),
        }
    }
}

#[test]
fn render_nodes_are_found_with_their_pci_address_and_memory() {
    let tree = Tree::new(&[("0000:0c:00.0", 128, Some(8_573_157_376))]);
    let nodes = scan(&tree.roots());
    assert_eq!(nodes.len(), 1);
    let node = &nodes[0];
    assert_eq!(node.name, "renderD128");
    assert_eq!(node.pci, "0000:0c:00.0");
    assert_eq!(node.vram, Some(8_573_157_376));
    assert!(node.sysfs.ends_with("0000:0c:00.0"));
    assert_eq!(node.dev, tree.roots().dev.join("renderD128"));
    assert!(node.sysfs.is_absolute());
}

#[test]
fn a_node_whose_device_numbers_differ_from_sysfs_is_not_used() {
    let tree = Tree::new(&[("0000:0c:00.0", 128, None)]);
    let roots = tree.roots();
    std::fs::remove_file(roots.dev.join("renderD128")).unwrap();
    symlink("/dev/zero", roots.dev.join("renderD128")).unwrap();
    assert_eq!(scan(&roots), Vec::<Node>::new());
}

fn candidate(pci: &str, vram: Option<u64>, total_mib: u64) -> Candidate {
    Candidate {
        node: Node {
            name: format!("renderD{}", 128),
            dev: PathBuf::from("/dev/dri/renderD128"),
            rdev: (226, 128),
            sysfs: PathBuf::from(format!("/sys/devices/pci0000:00/{pci}")),
            pci: pci.into(),
            vram,
        },
        listed: Listed {
            name: "Vulkan0".into(),
            description: "AMD Radeon RX 6650 XT (RADV NAVI23)".into(),
            total_mib,
            free_mib: total_mib,
        },
    }
}

#[test]
fn two_identical_cards_are_told_apart_and_remembered_by_pci_address() {
    let cards = [
        candidate("0000:0d:00.0", Some(8 << 30), 8176),
        candidate("0000:0c:00.0", Some(8 << 30), 8176),
    ];
    // Ties go to the lower PCI address.
    assert_eq!(
        select(&cards, &Choice::Auto, None).unwrap().node.pci,
        "0000:0c:00.0"
    );
    // A remembered choice stays while that card is listed.
    assert_eq!(
        select(&cards, &Choice::Auto, Some("0000:0d:00.0"))
            .unwrap()
            .node
            .pci,
        "0000:0d:00.0"
    );
    assert_eq!(
        select(&cards, &Choice::Auto, Some("0000:0e:00.0"))
            .unwrap()
            .node
            .pci,
        "0000:0c:00.0"
    );
    // A PCI address selects that card, and only that card.
    assert_eq!(
        select(&cards, &Choice::Pci("0000:0d:00.0".into()), None)
            .unwrap()
            .node
            .pci,
        "0000:0d:00.0"
    );
    assert!(select(&cards, &Choice::Pci("0000:0e:00.0".into()), None).is_err());
    assert!(select(&cards, &Choice::Cpu, None).is_err());
    assert!(select(&[], &Choice::Auto, None).is_err());
}

#[test]
fn dedicated_ranks_before_others_only_when_sysfs_says_so() {
    // The integrated one reports more memory, but only the other has VRAM in sysfs.
    let dedicated = candidate("0000:0d:00.0", Some(4 << 30), 4096);
    let integrated = candidate("0000:00:08.0", None, 16384);
    assert_eq!(
        select(
            &[integrated.clone(), dedicated.clone()],
            &Choice::Auto,
            None
        )
        .unwrap()
        .node
        .pci,
        "0000:0d:00.0"
    );
    // Where sysfs says nothing for either, memory decides, then the address.
    let a = candidate("0000:0d:00.0", None, 4096);
    let b = candidate("0000:00:08.0", None, 16384);
    assert_eq!(
        select(&[a, b], &Choice::Auto, None).unwrap().node.pci,
        "0000:00:08.0"
    );
    let _ = (dedicated, integrated);
}
