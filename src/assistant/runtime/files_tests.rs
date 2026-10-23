use super::super::manifest::{Build, File as Entry};
use super::*;
use std::os::unix::fs::PermissionsExt;

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn models() -> (tempfile::TempDir, OwnedFd) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let fd = open_dir(dir.path(), Owners::Mine, true).unwrap();
    (dir, fd)
}

#[test]
fn a_model_is_opened_checked_and_hashed_from_its_descriptor() {
    let (dir, fd) = models();
    std::fs::write(dir.path().join("m.gguf"), b"model one").unwrap();
    let cache = HashCache::default();
    let model = model(fd.as_fd(), "m.gguf", &sha(b"model one"), &cache).unwrap();
    assert_eq!(hex::encode(model.sha256), sha(b"model one"));
    assert_eq!(
        super::model(fd.as_fd(), "m.gguf", &sha(b"other"), &cache).unwrap_err(),
        Refusal::Changed("m.gguf".into())
    );
}

#[test]
fn links_directories_fifos_and_writable_files_are_refused() {
    let (dir, fd) = models();
    let cache = HashCache::default();
    std::fs::write(dir.path().join("real.gguf"), b"x").unwrap();
    std::os::unix::fs::symlink("real.gguf", dir.path().join("link.gguf")).unwrap();
    std::fs::create_dir(dir.path().join("real")).unwrap();
    std::fs::write(dir.path().join("real/inner.gguf"), b"x").unwrap();
    std::os::unix::fs::symlink("real", dir.path().join("via")).unwrap();
    std::fs::create_dir(dir.path().join("folder.gguf")).unwrap();
    rustix::fs::mknodat(
        rustix::fs::CWD,
        dir.path().join("fifo.gguf"),
        FileType::Fifo,
        Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    std::fs::write(dir.path().join("open.gguf"), b"x").unwrap();
    std::fs::set_permissions(
        dir.path().join("open.gguf"),
        std::fs::Permissions::from_mode(0o666),
    )
    .unwrap();
    std::fs::write(dir.path().join("group.gguf"), b"x").unwrap();
    std::fs::set_permissions(
        dir.path().join("group.gguf"),
        std::fs::Permissions::from_mode(0o620),
    )
    .unwrap();
    let x = sha(b"x");
    let refused = |name: &str| model(fd.as_fd(), name, &x, &cache).unwrap_err();
    assert_eq!(refused("link.gguf"), Refusal::Link("link.gguf".into()));
    assert_eq!(
        refused("via/inner.gguf"),
        Refusal::Link("via/inner.gguf".into())
    );
    assert_eq!(
        refused("../escape.gguf"),
        Refusal::Link("../escape.gguf".into())
    );
    assert_eq!(
        refused("folder.gguf"),
        Refusal::NotRegular("folder.gguf".into())
    );
    // The FIFO does not block the open.
    assert_eq!(
        refused("fifo.gguf"),
        Refusal::NotRegular("fifo.gguf".into())
    );
    assert_eq!(refused("open.gguf"), Refusal::Writable("open.gguf".into()));
    assert_eq!(
        refused("group.gguf"),
        Refusal::Writable("group.gguf".into())
    );
    assert_eq!(
        refused("absent.gguf"),
        Refusal::Missing("absent.gguf".into())
    );
    // A real file in a real subfolder is fine.
    assert!(model(fd.as_fd(), "real/inner.gguf", &x, &cache).is_ok());
}

#[test]
fn a_model_swapped_after_the_check_is_still_the_hashed_inode() {
    let (dir, fd) = models();
    std::fs::write(dir.path().join("m.gguf"), b"the hashed model").unwrap();
    let cache = HashCache::default();
    let model = model(fd.as_fd(), "m.gguf", &sha(b"the hashed model"), &cache).unwrap();
    std::fs::write(dir.path().join("swap"), b"another file").unwrap();
    std::fs::rename(dir.path().join("swap"), dir.path().join("m.gguf")).unwrap();
    // What the child would load through the descriptor is the inode that was hashed.
    let through = std::fs::read(format!("/proc/self/fd/{}", {
        use std::os::fd::AsRawFd;
        model.fd.as_raw_fd()
    }))
    .unwrap();
    assert_eq!(through, b"the hashed model");
}

#[test]
fn a_hash_is_reused_while_the_identity_is_unchanged() {
    let (dir, fd) = models();
    std::fs::write(dir.path().join("a"), b"first").unwrap();
    std::fs::write(dir.path().join("b"), b"second").unwrap();
    let cache = HashCache::default();
    let (a, stat) = open_file(fd.as_fd(), "a", Owners::Mine).unwrap();
    let identity = Identity::of(&stat);
    let (b, _) = open_file(fd.as_fd(), "b", Owners::Mine).unwrap();
    assert_eq!(
        hex::encode(cache.hash("k", a.as_fd(), identity).unwrap()),
        sha(b"first")
    );
    // Same identity: not re-read (the other descriptor's content is never looked at).
    assert_eq!(
        hex::encode(cache.hash("k", b.as_fd(), identity).unwrap()),
        sha(b"first")
    );
    let mut changed = identity;
    changed.size += 1;
    assert_eq!(
        hex::encode(cache.hash("k", b.as_fd(), changed).unwrap()),
        sha(b"second")
    );
}

#[test]
fn private_folders_must_be_0700_and_not_links() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models");
    std::fs::create_dir(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o750)).unwrap();
    assert_eq!(
        open_dir(&path, Owners::Mine, true).unwrap_err(),
        Refusal::Mode(path.display().to_string())
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(open_dir(&path, Owners::Mine, true).is_ok());
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert_eq!(
        open_dir(&link, Owners::Mine, true).unwrap_err(),
        Refusal::Link(link.display().to_string())
    );
}

fn build(files: &[(&str, &[u8])]) -> Build {
    Build {
        path: "cpu".into(),
        directory: "cpu".into(),
        server: files[0].0.into(),
        files: files
            .iter()
            .map(|(name, content)| Entry {
                name: (*name).into(),
                sha256: sha(content),
                size: content.len() as u64,
                mode: "0755".into(),
            })
            .collect(),
    }
}

fn runtime_dir(files: &[(&str, &[u8])]) -> (tempfile::TempDir, OwnedFd) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("cpu")).unwrap();
    for (name, content) in files {
        let path = dir.path().join("cpu").join(name);
        std::fs::write(&path, content).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let fd = open_dir(dir.path(), Owners::MineOrRoot, false).unwrap();
    (dir, fd)
}

const FILES: &[(&str, &[u8])] = &[("llama-server", b"server"), ("libllama.so.0", b"lib")];

#[test]
fn the_runtime_holds_exactly_the_pinned_files() {
    let cache = HashCache::default();
    let (_dir, fd) = runtime_dir(FILES);
    let files = runtime(fd.as_fd(), &build(FILES), &cache).unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].name, "llama-server");

    let (dir, fd) = runtime_dir(FILES);
    std::fs::write(dir.path().join("cpu/extra.so"), b"x").unwrap();
    assert_eq!(
        runtime(fd.as_fd(), &build(FILES), &cache).unwrap_err(),
        Refusal::Extra("extra.so".into())
    );

    let (dir, fd) = runtime_dir(FILES);
    std::fs::remove_file(dir.path().join("cpu/libllama.so.0")).unwrap();
    assert_eq!(
        runtime(fd.as_fd(), &build(FILES), &cache).unwrap_err(),
        Refusal::Missing("libllama.so.0".into())
    );

    let (dir, fd) = runtime_dir(FILES);
    std::fs::write(dir.path().join("cpu/libllama.so.0"), b"LIB").unwrap();
    assert_eq!(
        runtime(fd.as_fd(), &build(FILES), &cache).unwrap_err(),
        Refusal::Changed("libllama.so.0".into())
    );

    let (dir, fd) = runtime_dir(FILES);
    std::fs::set_permissions(
        dir.path().join("cpu/libllama.so.0"),
        std::fs::Permissions::from_mode(0o775),
    )
    .unwrap();
    assert_eq!(
        runtime(fd.as_fd(), &build(FILES), &cache).unwrap_err(),
        Refusal::Writable("libllama.so.0".into())
    );

    let (dir, fd) = runtime_dir(FILES);
    std::fs::remove_file(dir.path().join("cpu/libllama.so.0")).unwrap();
    std::fs::write(dir.path().join("real"), b"lib").unwrap();
    std::os::unix::fs::symlink(
        dir.path().join("real"),
        dir.path().join("cpu/libllama.so.0"),
    )
    .unwrap();
    assert_eq!(
        runtime(fd.as_fd(), &build(FILES), &cache).unwrap_err(),
        Refusal::Link("libllama.so.0".into())
    );

    let (dir, fd) = runtime_dir(FILES);
    std::fs::set_permissions(
        dir.path().join("cpu"),
        std::fs::Permissions::from_mode(0o777),
    )
    .unwrap();
    assert_eq!(
        runtime(fd.as_fd(), &build(FILES), &cache).unwrap_err(),
        Refusal::Writable("cpu".into())
    );
}

#[test]
fn ancestors_writable_by_others_are_refused() {
    // The temporary folder (sticky, world-writable) is such an ancestor.
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        ancestors_safe(dir.path()),
        Err(Refusal::Writable(_))
    ));
    assert!(ancestors_safe(Path::new("/")).is_ok());
}

#[test]
fn a_linked_ancestor_is_checked_where_it_leads() {
    // A safe-looking path through a link into a folder whose parent others can write.
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/runtime-tests");
    std::fs::create_dir_all(&base).unwrap();
    std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o755)).unwrap();
    let dir = tempfile::Builder::new().tempdir_in(&base).unwrap();
    let open = dir.path().join("open");
    std::fs::create_dir_all(open.join("real/runtime")).unwrap();
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
    std::os::unix::fs::symlink(open.join("real"), dir.path().join("link")).unwrap();
    assert_eq!(
        ancestors_safe(&dir.path().join("link/runtime")),
        Err(Refusal::Writable(open.display().to_string()))
    );
}
