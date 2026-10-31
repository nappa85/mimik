//! Host namespace availability and the runner's start report.
use std::path::Path;

/// Why the sandbox cannot be used, shown on the assistant page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Unusable {
    /// A sysctl certainly disables unprivileged user namespaces; its name and value.
    UserNamespaces(&'static str),
}

impl std::fmt::Display for Unusable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UserNamespaces(setting) => {
                write!(f, "unprivileged user namespaces are disabled ({setting})")
            }
        }
    }
}

fn sysctl(root: &Path, name: &str) -> Option<u64> {
    std::fs::read_to_string(root.join(name))
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
}

/// Checks the one setting that certainly rules out the sandbox before its real probe.
/// `root` is `/proc` on the host, injectable for tests. Missing settings do not veto it.
///
/// # Errors
///
/// User namespaces are disabled by `user.max_user_namespaces = 0`.
pub(crate) fn namespaces(root: &Path) -> Result<(), Unusable> {
    if sysctl(root, "sys/user/max_user_namespaces") == Some(0) {
        return Err(Unusable::UserNamespaces("user.max_user_namespaces = 0"));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cause {
    /// Debian's switch forbids unprivileged user-namespace creation.
    UsernsClone,
    /// Ubuntu's `AppArmor` restriction requires permission for this executable.
    AppArmor,
}

pub(crate) fn namespace_cause(root: &Path) -> Option<Cause> {
    if sysctl(root, "sys/kernel/unprivileged_userns_clone") == Some(0) {
        return Some(Cause::UsernsClone);
    }
    if sysctl(root, "sys/kernel/apparmor_restrict_unprivileged_userns") == Some(1) {
        return Some(Cause::AppArmor);
    }
    None
}

/// What the runner's status descriptor reports first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Started {
    /// Host PID of the reaper outside the sandbox's PID namespace.
    pub child_pid: i32,
    /// PID namespace identity, or the protocol's unreported sentinel.
    pub pid_namespace: u64,
    /// Host PID of the server, the reaper's child (PID 1 inside the namespace).
    pub inner_pid: Option<i32>,
    /// Server start time in clock ticks since boot, read with its parent from one `stat`;
    /// paired with `inner_pid` so a reused PID cannot identify a different process.
    pub inner_start: Option<u64>,
    /// The server has already ended and the reaper was reaped: the launcher must not try
    /// to pin these PIDs. Its exit status still distinguishes setup from program failure.
    pub ended: bool,
}

#[cfg(test)]
#[path = "sandbox_tests.rs"]
mod tests;
