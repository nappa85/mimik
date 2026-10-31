//! Runtime helper processes share the sandbox's re-execution protocol.
pub(crate) use crate::sandbox::test_helper::{VARIABLE, command, errno, mode, print, report};

/// Where scratch folders go: under the target directory, or, as root (the CI container, whose
/// bind-mounted workspace belongs to another user), under a root-owned folder, so every
/// ancestor passes the level-3 owner rule.
pub(crate) fn scratch_base() -> std::path::PathBuf {
    if rustix::process::getuid().is_root() {
        std::path::PathBuf::from("/var/lib/mimik-runtime-tests")
    } else {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/runtime-tests")
    }
}
