use super::*;
use std::os::fd::AsFd;

/// A minimal but valid 64-bit little-endian ELF shared object with a dynamic section whose
/// `DT_NEEDED` entries name `libraries` (built here, so the fixture is exact and small).
pub(crate) fn elf_needing(libraries: &[&str]) -> Vec<u8> {
    let mut dynstr = vec![0_u8];
    let mut offsets = Vec::new();
    for library in libraries {
        offsets.push(dynstr.len() as u64);
        dynstr.extend_from_slice(library.as_bytes());
        dynstr.push(0);
    }
    // The dynamic entries are 8-byte aligned in the file.
    while dynstr.len() % 8 != 0 {
        dynstr.push(0);
    }
    let mut dynamic = Vec::new();
    for offset in &offsets {
        dynamic.extend_from_slice(&elf::DT_NEEDED.0.to_le_bytes());
        dynamic.extend_from_slice(&offset.to_le_bytes());
    }
    dynamic.extend_from_slice(&[0; 16]);
    let shstrtab = b"\0.dynstr\0.dynamic\0.shstrtab\0".to_vec();
    let dynstr_at = 64_u64;
    let dynamic_at = dynstr_at + dynstr.len() as u64;
    let shstrtab_at = dynamic_at + dynamic.len() as u64;
    let headers_at = (shstrtab_at + shstrtab.len() as u64).next_multiple_of(8);
    let mut out = Vec::new();
    out.extend_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    out.extend_from_slice(&3_u16.to_le_bytes()); // ET_DYN
    out.extend_from_slice(&62_u16.to_le_bytes()); // x86-64
    out.extend_from_slice(&1_u32.to_le_bytes());
    out.extend_from_slice(&0_u64.to_le_bytes()); // entry
    out.extend_from_slice(&0_u64.to_le_bytes()); // phoff
    out.extend_from_slice(&headers_at.to_le_bytes()); // shoff
    out.extend_from_slice(&0_u32.to_le_bytes()); // flags
    out.extend_from_slice(&64_u16.to_le_bytes()); // ehsize
    out.extend_from_slice(&0_u16.to_le_bytes()); // phentsize
    out.extend_from_slice(&0_u16.to_le_bytes()); // phnum
    out.extend_from_slice(&64_u16.to_le_bytes()); // shentsize
    out.extend_from_slice(&4_u16.to_le_bytes()); // shnum
    out.extend_from_slice(&3_u16.to_le_bytes()); // shstrndx
    assert_eq!(out.len(), 64);
    out.extend_from_slice(&dynstr);
    out.extend_from_slice(&dynamic);
    out.extend_from_slice(&shstrtab);
    out.resize(usize::try_from(headers_at).unwrap(), 0);
    // name, type, flags, addr, offset, size, link, info, addralign, entsize
    let header = |name: u32, kind: u32, offset: u64, size: u64, link: u32, entsize: u64| {
        let mut h = Vec::new();
        h.extend_from_slice(&name.to_le_bytes());
        h.extend_from_slice(&kind.to_le_bytes());
        h.extend_from_slice(&0_u64.to_le_bytes());
        h.extend_from_slice(&0_u64.to_le_bytes());
        h.extend_from_slice(&offset.to_le_bytes());
        h.extend_from_slice(&size.to_le_bytes());
        h.extend_from_slice(&link.to_le_bytes());
        h.extend_from_slice(&0_u32.to_le_bytes());
        h.extend_from_slice(&1_u64.to_le_bytes());
        h.extend_from_slice(&entsize.to_le_bytes());
        h
    };
    out.extend_from_slice(&[0; 64]);
    out.extend(header(1, 3, dynstr_at, dynstr.len() as u64, 0, 0));
    out.extend(header(9, 6, dynamic_at, dynamic.len() as u64, 1, 16));
    out.extend(header(18, 3, shstrtab_at, shstrtab.len() as u64, 0, 0));
    out
}

#[test]
fn the_dt_needed_names_of_a_real_elf_are_read() {
    assert_eq!(
        needed(&elf_needing(&["libgomp.so.1", "libc.so.6"])).unwrap(),
        ["libgomp.so.1", "libc.so.6"]
    );
    assert_eq!(needed(&elf_needing(&[])).unwrap().len(), 0);
    assert_eq!(needed(b"not an elf at all"), None);
    assert_eq!(needed(&[]), None);
}

fn from_memory(files: &[(&str, Vec<u8>)], system: &[&str], start: &str) -> Option<Vec<String>> {
    let find = |name: &str| files.iter().find(|file| file.0 == name);
    missing_from(
        start,
        &|name| find(name).map(|file| file.1.clone()),
        &|name| find(name).is_some(),
        &|name| system.contains(&name),
    )
}

#[test]
fn a_library_named_by_an_elf_and_found_nowhere_is_reported() {
    let server = elf_needing(&["libllama.so.0", "libc.so.6"]);
    let llama = elf_needing(&["libgomp.so.1", "libc.so.6", "libggml.so.0"]);
    let ggml = elf_needing(&["libm.so.6"]);
    let files = [
        ("llama-server", server),
        ("libllama.so.0", llama),
        ("libggml.so.0", ggml),
    ];
    // Through a runtime library (transitively), not only the server's own list.
    assert_eq!(
        from_memory(&files, &["libc.so.6", "libm.so.6"], "llama-server").unwrap(),
        ["libgomp.so.1"]
    );
    // With it installed nothing is missing.
    assert_eq!(
        from_memory(
            &files,
            &["libc.so.6", "libm.so.6", "libgomp.so.1"],
            "llama-server"
        )
        .unwrap()
        .len(),
        0
    );
    let why = message(
        "llama-server",
        &["libgomp.so.1".into(), "libvulkan.so.1".into()],
    )
    .unwrap();
    assert!(
        why.starts_with("llama-server needs libgomp.so.1: install libgomp1 (Debian/Ubuntu)"),
        "{why}"
    );
    assert!(
        why.contains("OpenMP runtime") && why.contains("also missing: libvulkan.so.1"),
        "{why}"
    );
    assert_eq!(message("llama-server", &[]), None);
}

#[test]
fn anything_unreadable_or_not_a_plain_name_is_unknown_not_missing() {
    // A file that cannot be read, one that is not an ELF, a name with a path or markup in it.
    assert_eq!(from_memory(&[], &[], "llama-server"), None);
    assert_eq!(
        from_memory(&[("llama-server", b"junk".to_vec())], &[], "llama-server"),
        None
    );
    for bad in ["../evil.so", "<script>.so", "a b.so", ".hidden", ""] {
        let files = [("llama-server", elf_needing(&[bad]))];
        assert_eq!(from_memory(&files, &[], "llama-server"), None, "{bad:?}");
    }
}

#[test]
fn inspection_reads_the_descriptor_not_the_path() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("llama-server");
    let original = elf_needing(&["libreal-original.so.1"]);
    std::fs::write(&path, &original).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    // What verification holds: the descriptor and its size.
    let file = std::fs::File::open(&path).unwrap();
    let size = file.metadata().unwrap().len();
    // The path is then swapped (a rename over it, as an ancestor or the file itself could be).
    let other = dir.path().join("other");
    std::fs::write(&other, elf_needing(&["libswapped.so.1"])).unwrap();
    std::fs::rename(&other, &path).unwrap();
    assert_eq!(
        needed(&std::fs::read(&path).unwrap()).unwrap(),
        ["libswapped.so.1"]
    );
    // The descriptor still shows the verified file.
    let seen = read_fd(file.as_fd(), size).unwrap();
    assert_eq!(seen, original);
    assert_eq!(needed(&seen).unwrap(), ["libreal-original.so.1"]);
    // A file over the bound is not read.
    assert_eq!(read_fd(file.as_fd(), MAX_BYTES + 1), None);
}

#[test]
fn system_directories_are_searched_with_their_multiarch_folders() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("x86_64-linux-gnu")).unwrap();
    std::fs::write(dir.path().join("x86_64-linux-gnu/libgomp.so.1"), b"x").unwrap();
    std::fs::write(dir.path().join("libplain.so.1"), b"x").unwrap();
    std::fs::create_dir_all(dir.path().join("other")).unwrap();
    std::fs::write(dir.path().join("other/libhidden.so.1"), b"x").unwrap();
    let dirs = vec![dir.path().to_path_buf()];
    assert!(in_dirs("libgomp.so.1", &dirs));
    assert!(in_dirs("libplain.so.1", &dirs));
    assert!(!in_dirs("libhidden.so.1", &dirs));
    assert!(!in_dirs("libabsent.so.1", &dirs));
}

/// The pinned builds on this host (`MIMIK_LIVE_RUNTIME`, the unpacked runtime): nothing is
/// missing for the CPU or the Vulkan server.
#[test]
#[ignore = "live: the pinned runtime (MIMIK_LIVE_RUNTIME) and this host's libraries"]
fn live_the_pinned_servers_report_nothing_missing_on_this_host() {
    let runtime =
        PathBuf::from(std::env::var_os("MIMIK_LIVE_RUNTIME").expect("MIMIK_LIVE_RUNTIME"));
    for build in ["cpu", "vulkan"] {
        let dir = runtime.join(build);
        assert_eq!(explain_dir(&dir, "llama-server"), None, "{build}");
        // `None` also means "unknown": the check must really have read the files.
        let dirs: Vec<PathBuf> = LIBRARY_DIRS.iter().map(PathBuf::from).collect();
        let found = missing_from(
            "llama-server",
            &|name| std::fs::read(dir.join(name)).ok(),
            &|name| plain(name) && dir.join(name).is_file(),
            &|name| in_dirs(name, &dirs),
        );
        assert_eq!(found.map(|missing| missing.len()), Some(0), "{build}");
    }
}
