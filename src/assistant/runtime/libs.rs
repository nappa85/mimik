//! A shared library the model server needs and the host lacks (`libgomp.so.1` on a minimal
//! Ubuntu server): named before the sandbox starts, so the page can say which library and what
//! to install instead of reporting a server that never answered `/health` with nothing on its
//! error output.
//!
//! Nothing is executed. The `DT_NEEDED` names are read from the ELF dynamic section of the
//! verified server and of each verified runtime library, from their verified descriptors (so
//! what is inspected is what was hashed, whatever happens to the paths afterwards), with the
//! `object` crate (DEP-26). Each name is looked for among the verified runtime files first (the
//! server's own folder) and then in the library directories the sandbox binds
//! ([`bwrap::LIBRARY_DIRS`], a few folders down: multiarch and compiler-runtime folders). A name found nowhere is
//! reported; a file that cannot be read or parsed, or a name that is not a plain library name,
//! makes the answer "unknown" and the start goes on as it would without this check.
use super::bwrap::LIBRARY_DIRS;
use super::files::Verified;
use object::read::elf::{Dyn, FileHeader};
use object::{Endianness, elf};
use std::collections::BTreeSet;
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::fs::FileExt;
use std::path::PathBuf;

/// The most a library file may weigh for its dynamic section to be read (a bound on memory).
const MAX_BYTES: u64 = 256 << 20;

/// The `DT_NEEDED` names of the 64-bit ELF `data` (`None`: not an ELF this can read, or no
/// dynamic section).
pub(crate) fn needed(data: &[u8]) -> Option<Vec<String>> {
    let header = elf::FileHeader64::<Endianness>::parse(data).ok()?;
    let endian = header.endian().ok()?;
    let sections = header.sections(endian, data).ok()?;
    // The section index returned with the entries is that of their string table.
    let (entries, strings_index) = sections.dynamic(endian, data).ok()??;
    let strings = sections.strings(endian, data, strings_index).ok()?;
    let mut names = Vec::new();
    for entry in entries {
        if entry.tag(endian) == elf::DT_NEEDED {
            let name = entry.string(endian, strings).ok()?;
            names.push(String::from_utf8(name.to_vec()).ok()?);
        }
    }
    Some(names)
}

/// A plain library file name: nothing a path, a page or a command line would read as more.
fn plain(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c))
}

/// The libraries `start` needs, directly or through the runtime files, that are neither among
/// those files (`in_runtime`) nor in the system directories (`in_system`). `read` gives a
/// runtime file's bytes. `None` when anything cannot be told.
pub(crate) fn missing_from(
    start: &str,
    read: &dyn Fn(&str) -> Option<Vec<u8>>,
    in_runtime: &dyn Fn(&str) -> bool,
    in_system: &dyn Fn(&str) -> bool,
) -> Option<Vec<String>> {
    let mut queue = vec![start.to_owned()];
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut missing: BTreeSet<String> = BTreeSet::new();
    while let Some(file) = queue.pop() {
        if !seen.insert(file.clone()) {
            continue;
        }
        for name in needed(&read(&file)?)? {
            if !plain(&name) {
                return None;
            }
            if in_runtime(&name) {
                queue.push(name);
            } else if !in_system(&name) {
                missing.insert(name);
            }
        }
    }
    Some(missing.into_iter().collect())
}

/// Reads `size` bytes of the descriptor from offset 0 (its own offset is left alone).
pub(crate) fn read_fd(fd: BorrowedFd<'_>, size: u64) -> Option<Vec<u8>> {
    if size > MAX_BYTES {
        return None;
    }
    let file = std::fs::File::from(fd.try_clone_to_owned().ok()?);
    let mut data = vec![0; usize::try_from(size).ok()?];
    file.read_exact_at(&mut data, 0).ok()?;
    Some(data)
}

/// How deep under a library directory names are collected (`/usr/lib/gcc/x86_64-pc-linux-gnu/15`
/// is three folders down; `ld.so.cache` points into such folders).
const DEPTH: usize = 4;
/// The most names collected (a bound on the walk).
const MAX_NAMES: usize = 400_000;

/// The file names (files and links) under `dirs`, to [`DEPTH`] folders down, not following
/// links to folders below the top ones: the names the sandbox's loader could resolve in the
/// folders it binds.
pub(crate) fn system_names(dirs: &[PathBuf]) -> BTreeSet<String> {
    fn walk(dir: &std::path::Path, depth: usize, names: &mut BTreeSet<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            if names.len() >= MAX_NAMES {
                return;
            }
            let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
            if is_dir {
                if depth + 1 < DEPTH {
                    walk(&entry.path(), depth + 1, names);
                }
            } else {
                names.insert(entry.file_name().to_string_lossy().into_owned());
            }
        }
    }
    let mut names = BTreeSet::new();
    for dir in dirs {
        walk(dir, 0, &mut names);
    }
    names
}

/// What the owner can install for `library`.
fn remedy(library: &str) -> String {
    if library.starts_with("libgomp.so") {
        "install libgomp1 (Debian/Ubuntu) or your distribution's OpenMP runtime".to_owned()
    } else if library.starts_with("libvulkan.so") {
        "install libvulkan1 (Debian/Ubuntu) or your distribution's Vulkan loader".to_owned()
    } else {
        "install the package of your distribution that provides it".to_owned()
    }
}

/// The start-time error for `missing` libraries of `server` (`None`: none).
pub(crate) fn message(server: &str, missing: &[String]) -> Option<String> {
    let first = missing.first()?;
    let also = if missing.len() > 1 {
        format!(" (also missing: {})", missing[1..].join(", "))
    } else {
        String::new()
    };
    Some(format!("{server} needs {first}: {}{also}", remedy(first)))
}

/// The error for the libraries the verified `files` (the server `server` among them) need and
/// the host lacks, from their descriptors; `None` when none is missing or nothing could be told.
pub(crate) fn explain(files: &[Verified], server: &str) -> Option<String> {
    let find = |name: &str| files.iter().find(|file| file.name == name);
    let dirs: Vec<PathBuf> = LIBRARY_DIRS.iter().map(PathBuf::from).collect();
    let system = system_names(&dirs);
    let missing = missing_from(
        server,
        &|name| find(name).and_then(|file| read_fd(file.fd.as_fd(), file.identity.size)),
        &|name| find(name).is_some(),
        &|name| system.contains(name),
    )?;
    message(server, &missing)
}

/// The same over a folder on disk (the live check of a pinned build).
#[cfg(test)]
pub(crate) fn explain_dir(dir: &std::path::Path, server: &str) -> Option<String> {
    let dirs: Vec<PathBuf> = LIBRARY_DIRS.iter().map(PathBuf::from).collect();
    let system = system_names(&dirs);
    let missing = missing_from(
        server,
        &|name| std::fs::read(dir.join(name)).ok(),
        &|name| plain(name) && dir.join(name).is_file(),
        &|name| system.contains(name),
    )?;
    message(server, &missing)
}

#[cfg(test)]
#[path = "libs_tests.rs"]
pub(crate) mod tests;
