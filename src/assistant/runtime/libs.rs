//! A shared library the model server needs and the host lacks (`libgomp.so.1` on a minimal
//! Ubuntu server): found before the sandbox starts, by the dynamic loader's own list mode, so
//! the page can name the library instead of reporting a server that never answered `/health`
//! with nothing on its error output.
//!
//! The loader is run on the already verified server file with the runtime folder as its
//! library path and nothing else of the environment; its list mode resolves the libraries and
//! does not run the program. Without a loader at a known place, or if it cannot be run, nothing
//! is said (the start goes on and fails as before).
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where the dynamic loader lives on the platforms Mimik ships for.
const LOADERS: [&str; 5] = [
    "/lib64/ld-linux-x86-64.so.2",
    "/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2",
    "/usr/lib64/ld-linux-x86-64.so.2",
    "/lib/ld-linux-aarch64.so.1",
    "/lib/aarch64-linux-gnu/ld-linux-aarch64.so.1",
];

/// The libraries `--list` output reports as not found.
pub(crate) fn not_found(listing: &str) -> Vec<String> {
    listing
        .lines()
        .filter_map(|line| {
            let (name, rest) = line.trim().split_once("=>")?;
            (rest.trim() == "not found").then(|| name.trim().to_owned())
        })
        .collect()
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

/// The start-time error for the libraries of `server` the host lacks, run through `loader`
/// with `library_dir` as the library path (`None`: none missing, or nothing could be told).
pub(crate) fn explain_with(loader: &Path, server: &Path, library_dir: &Path) -> Option<String> {
    let output = Command::new(loader)
        .arg("--list")
        .arg(server)
        .env_clear()
        .env("LD_LIBRARY_PATH", library_dir)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let missing = not_found(&String::from_utf8_lossy(&output.stdout));
    let first = missing.first()?;
    let name = server.file_name()?.to_string_lossy();
    Some(format!(
        "{name} needs {first}: {}{}",
        remedy(first),
        if missing.len() > 1 {
            format!(" (also missing: {})", missing[1..].join(", "))
        } else {
            String::new()
        }
    ))
}

/// [`explain_with`] with the host's dynamic loader.
pub(crate) fn explain(server: &Path, library_dir: &Path) -> Option<String> {
    let loader: PathBuf = LOADERS
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())?;
    explain_with(&loader, server, library_dir)
}

#[cfg(test)]
#[path = "libs_tests.rs"]
mod tests;
