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

/// The same as a 32-bit ELF (only the class byte differs: not usable by a 64-bit file).
fn elf32_stub() -> Vec<u8> {
    let mut data = elf_needing(&[]);
    data[4] = 1;
    data
}

/// A library folder holding `files` (name and bytes), and a host searching only there.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new(files: &[(&str, Vec<u8>)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib");
        std::fs::create_dir_all(&root).unwrap();
        for (name, bytes) in files {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        Self { _dir: dir, root }
    }

    fn host<'a>(&self, cancelled: &'a dyn Fn() -> bool) -> Host<'a> {
        Host {
            standard: Vec::new(),
            cache: Some(Vec::new()),
            roots: vec![self.root.clone()],
            budget: Budget::DEFAULT,
            cancelled,
        }
        .with_multiarch()
    }
}

fn runtime(files: &[(&str, Vec<u8>)]) -> (Vec<String>, impl Fn(&str) -> Option<Vec<u8>>) {
    let names = files.iter().map(|file| file.0.to_owned()).collect();
    let files: Vec<(String, Vec<u8>)> = files
        .iter()
        .map(|file| (file.0.to_owned(), file.1.clone()))
        .collect();
    (names, move |name: &str| {
        files
            .iter()
            .find(|file| file.0 == name)
            .map(|file| file.1.clone())
    })
}

fn missing(runtime_files: &[(&str, Vec<u8>)], host: &Host<'_>) -> Option<Vec<String>> {
    let (names, read) = runtime(runtime_files);
    missing_from(&names, &read, host)
}

const NEVER: &dyn Fn() -> bool = &|| false;

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

#[test]
fn a_backend_the_server_loads_with_dlopen_is_inspected_like_the_server() {
    // The server's own graph is fine; the CPU backend variant, not in it, needs libgomp.
    let fixture = Fixture::new(&[("libc.so.6", elf_needing(&[]))]);
    let files = [
        ("llama-server", elf_needing(&["libc.so.6", "libllama.so.0"])),
        ("libllama.so.0", elf_needing(&["libc.so.6"])),
        (
            "libggml-cpu-haswell.so",
            elf_needing(&["libgomp.so.1", "libc.so.6"]),
        ),
        ("libggml-vulkan.so", elf_needing(&["libvulkan.so.1"])),
        ("LICENSE", b"MIT".to_vec()),
    ];
    assert_eq!(
        missing(&files, &fixture.host(NEVER)).unwrap(),
        ["libgomp.so.1", "libvulkan.so.1"]
    );
    let why = message(
        "llama-server",
        &missing(&files, &fixture.host(NEVER)).unwrap(),
    )
    .unwrap();
    assert!(
        why.starts_with("llama-server may need libgomp.so.1: install libgomp1 (Debian/Ubuntu)"),
        "{why}"
    );
    assert!(
        why.contains("OpenMP runtime") && why.contains("also missing: libvulkan.so.1"),
        "{why}"
    );
    assert_eq!(message("llama-server", &[]), None);
    // Installed: nothing is missing, and the licence was no problem.
    let complete = Fixture::new(&[
        ("libc.so.6", elf_needing(&[])),
        ("libgomp.so.1", elf_needing(&[])),
        ("libvulkan.so.1", elf_needing(&[])),
    ]);
    assert_eq!(missing(&files, &complete.host(NEVER)).unwrap().len(), 0);
}

#[test]
fn a_candidate_must_be_a_usable_library_not_just_a_name() {
    let mut fixture = Fixture::new(&[
        ("libstub.so.1", b"x".to_vec()),
        ("lib32.so.1", elf32_stub()),
        ("libgood.so.1", elf_needing(&[])),
    ]);
    std::os::unix::fs::symlink("libabsent.so.9", fixture.root.join("libdangling.so.1")).unwrap();
    std::os::unix::fs::symlink("libgood.so.1", fixture.root.join("liblink.so.1")).unwrap();
    std::fs::create_dir(fixture.root.join("libdir.so.1")).unwrap();
    let files = [(
        "llama-server",
        elf_needing(&[
            "libstub.so.1",
            "lib32.so.1",
            "libdangling.so.1",
            "libdir.so.1",
            "libgood.so.1",
            "liblink.so.1",
        ]),
    )];
    // A stub file, a 32-bit library, a dangling link and a folder do not count; a resolving
    // link and a real library do.
    let host = fixture.host(NEVER);
    assert_eq!(
        missing(&files, &host).unwrap(),
        [
            "lib32.so.1",
            "libdangling.so.1",
            "libdir.so.1",
            "libstub.so.1"
        ]
    );
    fixture.root.push("unused");
}

#[test]
fn a_name_found_only_where_the_loader_would_not_look_is_unknown() {
    // Deep in a folder that is neither a default folder nor in the cache: unknown, not missing
    // and not found.
    let fixture = Fixture::new(&[("vendor/deep/libodd.so.1", elf_needing(&[]))]);
    let files = [("llama-server", elf_needing(&["libodd.so.1"]))];
    assert_eq!(missing(&files, &fixture.host(NEVER)), None);
    // The same name in the cache is found.
    let mut host = fixture.host(NEVER);
    host.cache = Some(vec![(
        "libodd.so.1".into(),
        fixture.root.join("vendor/deep/libodd.so.1"),
    )]);
    assert_eq!(missing(&files, &host).unwrap().len(), 0);
    // A cache entry whose file is not usable does not count.
    let mut host = fixture.host(NEVER);
    host.cache = Some(vec![(
        "libodd.so.1".into(),
        fixture.root.join("vendor/absent.so"),
    )]);
    assert_eq!(missing(&files, &host), None);
    // The default folders: the folder itself and a multiarch one directly under it.
    let standard = Fixture::new(&[
        ("libhere.so.1", elf_needing(&[])),
        ("x86_64-linux-gnu/libmulti.so.1", elf_needing(&[])),
    ]);
    let files = [(
        "llama-server",
        elf_needing(&["libhere.so.1", "libmulti.so.1"]),
    )];
    assert_eq!(missing(&files, &standard.host(NEVER)).unwrap().len(), 0);
}

#[test]
fn an_incomplete_search_is_unknown_never_missing() {
    let files = [("llama-server", elf_needing(&["libabsent.so.1"]))];
    let fixture = Fixture::new(&[
        ("a/one", b"1".to_vec()),
        ("a/two", b"2".to_vec()),
        ("b/three", b"3".to_vec()),
    ]);
    // Complete: missing.
    assert_eq!(
        missing(&files, &fixture.host(NEVER)).unwrap(),
        ["libabsent.so.1"]
    );
    // The entry budget.
    let mut host = fixture.host(NEVER);
    host.budget.entries = 2;
    assert_eq!(missing(&files, &host), None);
    // The folder budget.
    let mut host = fixture.host(NEVER);
    host.budget.dirs = 1;
    assert_eq!(missing(&files, &host), None);
    // The depth bound (folders below it were not looked at).
    let deep = Fixture::new(&[("a/b/c/d/e", b"x".to_vec())]);
    let mut host = deep.host(NEVER);
    host.budget.depth = 2;
    assert_eq!(missing(&files, &host), None);
    // A cancel.
    let cancelled: &dyn Fn() -> bool = &|| true;
    assert_eq!(missing(&files, &fixture.host(cancelled)), None);
}

#[test]
fn an_unreadable_folder_makes_the_search_incomplete() {
    use std::os::unix::fs::PermissionsExt;
    let files = [("llama-server", elf_needing(&["libabsent.so.1"]))];
    let fixture = Fixture::new(&[("locked/file", b"x".to_vec()), ("open/file", b"x".to_vec())]);
    let locked = fixture.root.join("locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let readable = std::fs::read_dir(&locked).is_ok();
    let result = missing(&files, &fixture.host(NEVER));
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    if readable {
        eprintln!("SKIPPED: this user reads a folder of mode 000 (root)");
        return;
    }
    assert_eq!(result, None);
}

#[test]
fn a_folder_reached_twice_is_walked_once() {
    // `/lib` and `/usr/lib` can be the same folder: the second root costs nothing.
    let files = [("llama-server", elf_needing(&["libabsent.so.1"]))];
    let fixture = Fixture::new(&[("a/one", b"1".to_vec()), ("a/two", b"2".to_vec())]);
    let alias = fixture.root.with_file_name("alias");
    std::os::unix::fs::symlink(&fixture.root, &alias).unwrap();
    let mut host = fixture.host(NEVER);
    host.roots.push(alias);
    // lib: a (one, two) = 3 entries, 2 folders.
    host.budget = Budget {
        entries: 3,
        dirs: 2,
        depth: 6,
    };
    assert_eq!(missing(&files, &host).unwrap(), ["libabsent.so.1"]);
}

#[test]
fn anything_unreadable_or_not_a_plain_name_is_unknown_not_missing() {
    let fixture = Fixture::new(&[]);
    // A file that cannot be read, an ELF that cannot be parsed, a name with a path or markup.
    assert_eq!(missing(&[], &fixture.host(NEVER)).unwrap().len(), 0);
    let (names, _) = runtime(&[("llama-server", Vec::new())]);
    assert_eq!(missing_from(&names, &|_| None, &fixture.host(NEVER)), None);
    let broken = [b"\x7fELF\x02\x01\x01 truncated".to_vec()];
    assert_eq!(
        missing(&[("llama-server", broken[0].clone())], &fixture.host(NEVER)),
        None
    );
    for bad in ["../evil.so", "<script>.so", "a b.so", ".hidden", ""] {
        let files = [("llama-server", elf_needing(&[bad]))];
        assert_eq!(missing(&files, &fixture.host(NEVER)), None, "{bad:?}");
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
    // A file over 256 MiB is not read (the answer is then "unknown").
    assert_eq!(read_fd(file.as_fd(), MAX_BYTES + 1), None);
}

/// A glibc `ld.so.cache` (new format only) holding `entries`.
fn cache_bytes(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut strings = Vec::new();
    let mut table = Vec::new();
    let base = 48 + entries.len() * 24;
    for (name, path) in entries {
        let key = base + strings.len();
        strings.extend_from_slice(name.as_bytes());
        strings.push(0);
        let value = base + strings.len();
        strings.extend_from_slice(path.as_bytes());
        strings.push(0);
        table.extend_from_slice(&0x0303_i32.to_le_bytes());
        table.extend_from_slice(&u32::try_from(key).unwrap().to_le_bytes());
        table.extend_from_slice(&u32::try_from(value).unwrap().to_le_bytes());
        table.extend_from_slice(&[0; 4]);
        table.extend_from_slice(&0_u64.to_le_bytes());
    }
    let mut out = b"glibc-ld.so.cache1.1".to_vec();
    out.extend_from_slice(&u32::try_from(entries.len()).unwrap().to_le_bytes());
    out.extend_from_slice(&u32::try_from(strings.len()).unwrap().to_le_bytes());
    out.resize(48, 0);
    out.extend(table);
    out.extend(strings);
    out
}

#[test]
fn the_loader_cache_is_parsed_read_only() {
    let parsed = parse_cache(
        &cache_bytes(&[
            (
                "libgomp.so.1",
                "/usr/lib/gcc/x86_64-pc-linux-gnu/15/libgomp.so.1",
            ),
            ("libc.so.6", "/lib64/libc.so.6"),
        ]),
        NEVER,
    )
    .unwrap();
    assert_eq!(parsed[0].0, "libgomp.so.1");
    assert_eq!(
        parsed[0].1,
        PathBuf::from("/usr/lib/gcc/x86_64-pc-linux-gnu/15/libgomp.so.1")
    );
    assert_eq!(parsed[1].1, PathBuf::from("/lib64/libc.so.6"));
    // Old format in front of the new one (the cache of most distributions).
    let mut both = b"ld.so-1.7.0\0\0\0\0\0\0\0\0".to_vec();
    both.extend(cache_bytes(&[("liba.so.1", "/lib/liba.so.1")]));
    assert_eq!(parse_cache(&both, NEVER).unwrap().len(), 1);
    assert_eq!(parse_cache(b"junk", NEVER), None);
    // A count or an offset beyond the file is not read.
    let mut cut = cache_bytes(&[("liba.so.1", "/lib/liba.so.1")]);
    cut.truncate(60);
    assert_eq!(parse_cache(&cut, NEVER), None);
    // This host's own cache, when it has one.
    if let Ok(real) = std::fs::read("/etc/ld.so.cache") {
        assert!(parse_cache(&real, NEVER).is_some_and(|entries| !entries.is_empty()));
    }
}

/// The pinned builds on this host (`MIMIK_LIVE_RUNTIME`, the unpacked runtime): every file of
/// the CPU and the Vulkan build is inspected (backends included) and nothing is missing.
#[test]
#[ignore = "live: the pinned runtime (MIMIK_LIVE_RUNTIME) and this host's libraries"]
fn live_the_pinned_servers_report_nothing_missing_on_this_host() {
    let runtime =
        PathBuf::from(std::env::var_os("MIMIK_LIVE_RUNTIME").expect("MIMIK_LIVE_RUNTIME"));
    for build in ["cpu", "vulkan"] {
        // `Some(empty)` is "known, and nothing missing"; `None` would be "unknown".
        assert_eq!(
            explain_dir(&runtime.join(build)).map(|missing| missing.len()),
            Some(0),
            "{build}"
        );
    }
}

#[test]
fn a_crafted_cache_is_unrecognised_not_amplified() {
    let plain_cache = cache_bytes(&[("liba.so.1", "/lib/liba.so.1")]);
    // Offsets into the entry table or the header (overlapping what they must not).
    for at in [0_u32, 20, 48] {
        let mut crafted = plain_cache.clone();
        crafted[52..56].copy_from_slice(&at.to_le_bytes());
        assert_eq!(parse_cache(&crafted, NEVER), None, "key offset {at}");
        let mut crafted = plain_cache.clone();
        crafted[56..60].copy_from_slice(&at.to_le_bytes());
        assert_eq!(parse_cache(&crafted, NEVER), None, "value offset {at}");
    }
    // An offset past the end, and a count the file cannot hold.
    let mut past = plain_cache.clone();
    past[52..56].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(parse_cache(&past, NEVER), None);
    let mut many = plain_cache.clone();
    many[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(parse_cache(&many, NEVER), None);
    // A string over the bound (no end within 4 KiB).
    let long = "a".repeat(5000);
    assert_eq!(
        parse_cache(&cache_bytes(&[(&long, "/lib/x.so")]), NEVER),
        None
    );
    // Amplification: many entries pointing at the same long string cost bounded work, and the
    // parse gives up instead of reading it all.
    let shared = "s".repeat(4000);
    let mut amplified = cache_bytes(&[(&shared, "/lib/x.so")]);
    let entries = 6000_usize;
    let table_end = 48 + entries * 24;
    let mut table = Vec::new();
    for _ in 0..entries {
        table.extend_from_slice(&0x0303_i32.to_le_bytes());
        table.extend_from_slice(&u32::try_from(table_end).unwrap().to_le_bytes());
        table.extend_from_slice(&u32::try_from(table_end).unwrap().to_le_bytes());
        table.extend_from_slice(&[0; 12]);
    }
    amplified.clear();
    amplified.extend_from_slice(b"glibc-ld.so.cache1.1");
    amplified.extend_from_slice(&u32::try_from(entries).unwrap().to_le_bytes());
    amplified.resize(48, 0);
    amplified.extend(table);
    amplified.extend_from_slice(shared.as_bytes());
    amplified.push(0);
    assert_eq!(parse_cache(&amplified, NEVER), None);
    // A cancel is not an answer.
    assert_eq!(parse_cache(&plain_cache, &|| true), None);
}

#[test]
fn an_unrecognised_cache_is_unknown_and_an_empty_one_is_known() {
    // The old format alone (no new-format section): not recognised.
    let mut old = b"ld.so-1.7.0\0".to_vec();
    old.extend_from_slice(&1_u32.to_le_bytes());
    old.extend_from_slice(&[0; 24]);
    assert_eq!(parse_cache(&old, NEVER), None);
    // A cache that parsed and holds nothing.
    assert_eq!(parse_cache(&cache_bytes(&[]), NEVER), Some(Vec::new()));
    let files = [("llama-server", elf_needing(&["libabsent.so.1"]))];
    let fixture = Fixture::new(&[("x", b"x".to_vec())]);
    // Known empty cache and a complete search: missing. Unreadable cache: unknown.
    assert_eq!(
        missing(&files, &fixture.host(NEVER)).unwrap(),
        ["libabsent.so.1"]
    );
    let mut host = fixture.host(NEVER);
    host.cache = None;
    assert_eq!(missing(&files, &host), None);
}

#[test]
fn a_cache_entry_settles_a_name_with_no_standard_folders() {
    let fixture = Fixture::new(&[("vendor/libodd.so.1", elf_needing(&[]))]);
    let host = Host {
        standard: Vec::new(),
        cache: Some(vec![(
            "libodd.so.1".into(),
            fixture.root.join("vendor/libodd.so.1"),
        )]),
        roots: Vec::new(),
        budget: Budget::DEFAULT,
        cancelled: NEVER,
    };
    let wanted = BTreeMap::from([(
        "libodd.so.1".to_owned(),
        kind_of(&elf_needing(&[])).unwrap(),
    )]);
    assert_eq!(host.find(&wanted)["libodd.so.1"], Found::Yes);
}

/// A FIFO named like a library, and a link to one: neither is ever blocked on. The check runs in
/// a child process under a hard timeout, so a regression hangs the child, not the test run.
#[test]
fn a_fifo_or_a_link_to_one_never_hangs_the_search() {
    use std::process::{Command, Stdio};
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "assistant::runtime::libs::tests::fifo_child",
            "--nocapture",
        ])
        .env("MIMIK_TEST_FIFO_ROOT", dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() > deadline {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("the search hung on a FIFO");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let mut out = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    assert!(status.success(), "{out}");
    assert!(out.contains("fifo checked"), "{out}");
}

#[test]
fn fifo_child() {
    let Some(root) = std::env::var_os("MIMIK_TEST_FIFO_ROOT") else {
        return;
    };
    let root = PathBuf::from(root).join("lib");
    std::fs::create_dir_all(&root).unwrap();
    let fifo = root.join("libfifo.so.1");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    std::os::unix::fs::symlink("libfifo.so.1", root.join("liblinkfifo.so.1")).unwrap();
    let files = [(
        "llama-server",
        elf_needing(&["libfifo.so.1", "liblinkfifo.so.1"]),
    )];
    let host = Host {
        standard: vec![root.clone()],
        cache: Some(Vec::new()),
        roots: vec![root],
        budget: Budget::DEFAULT,
        cancelled: NEVER,
    };
    // Neither counts as a library; the walk skips the FIFO entry and the link is refused after
    // a non-blocking open. (The names are then known to be nowhere usable.)
    let found = missing(&files, &host);
    assert!(found.is_none_or(|m| m.len() == 2));
    println!("fifo checked");
}
