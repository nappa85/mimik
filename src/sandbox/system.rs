//! Shared system library discovery, without GPU or CPU-topology grants.
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Item {
    RoBind(PathBuf),
    Symlink { target: PathBuf, at: PathBuf },
    Dir(PathBuf),
}

pub(crate) const LIBRARY_DIRS: [&str; 5] = [
    "/usr/lib",
    "/usr/lib64",
    "/lib",
    "/lib64",
    "/usr/libexec/ld",
];

pub(crate) fn library_items() -> Vec<Item> {
    let mut items = Vec::new();
    for dir in LIBRARY_DIRS {
        let path = Path::new(dir);
        let Ok(metadata) = std::fs::symlink_metadata(path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            if let Ok(target) = std::fs::read_link(path) {
                items.push(Item::Symlink {
                    target,
                    at: path.to_path_buf(),
                });
            }
        } else if metadata.is_dir() {
            items.push(Item::RoBind(path.to_path_buf()));
        }
    }
    if Path::new("/etc/ld.so.cache").is_file() {
        items.push(Item::RoBind("/etc/ld.so.cache".into()));
    }
    items
}
