use super::*;
use std::os::unix::fs::PermissionsExt;

const LISTING: &str = "\tlinux-vdso.so.1 (0x00007ffd)\n\tlibggml.so.0 => /opt/llama/libggml.so.0 (0x00007f)\n\tlibgomp.so.1 => not found\n\tlibvulkan.so.1 => not found\n\tlibc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0x00007f)\n";

#[test]
fn only_the_libraries_reported_as_not_found_are_named() {
    assert_eq!(not_found(LISTING), ["libgomp.so.1", "libvulkan.so.1"]);
    assert_eq!(not_found("\tlibc.so.6 => /lib/libc.so.6 (0x1)\n").len(), 0);
    assert_eq!(not_found("").len(), 0);
}

/// A stand-in loader: a script that prints `listing` for `--list`.
fn loader(dir: &Path, listing: &str) -> PathBuf {
    let path = dir.join("ld.so");
    std::fs::write(
        &path,
        format!("#!/bin/sh\n[ \"$1\" = --list ] || exit 3\ncat <<'EOF'\n{listing}EOF\n"),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn a_missing_library_is_named_with_what_to_install() {
    let dir = tempfile::tempdir().unwrap();
    let why = explain_with(
        &loader(dir.path(), LISTING),
        Path::new("/opt/llama/llama-server"),
        dir.path(),
    )
    .unwrap();
    assert!(
        why.starts_with("llama-server needs libgomp.so.1: install libgomp1 (Debian/Ubuntu)"),
        "{why}"
    );
    assert!(why.contains("OpenMP runtime"), "{why}");
    assert!(why.contains("also missing: libvulkan.so.1"), "{why}");
}

#[test]
fn nothing_is_said_when_nothing_is_missing_or_the_loader_cannot_run() {
    let dir = tempfile::tempdir().unwrap();
    let ok = loader(dir.path(), "\tlibc.so.6 => /lib/libc.so.6 (0x1)\n");
    assert_eq!(
        explain_with(&ok, Path::new("/opt/llama/llama-server"), dir.path()),
        None
    );
    assert_eq!(
        explain_with(
            &dir.path().join("absent"),
            Path::new("/opt/llama/llama-server"),
            dir.path()
        ),
        None
    );
}

#[test]
fn the_hosts_own_loader_finds_nothing_missing_in_a_system_program() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(explain(Path::new("/bin/sh"), dir.path()), None);
}
