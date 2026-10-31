//! Host namespace availability and the runner's start report.
use std::path::Path;

/// Why the sandbox cannot be used, shown on the assistant page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Unusable {
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

pub(crate) fn namespaces(root: &Path) -> Result<(), Unusable> {
    if sysctl(root, "sys/user/max_user_namespaces") == Some(0) {
        return Err(Unusable::UserNamespaces("user.max_user_namespaces = 0"));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cause {
    UsernsClone,
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
    pub child_pid: i32,
    pub pid_namespace: u64,
    pub inner_pid: Option<i32>,
    pub inner_start: Option<u64>,
    pub ended: bool,
}

#[cfg(test)]
#[path = "sandbox_tests.rs"]
mod tests;
