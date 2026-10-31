//! Host namespace availability and the runner's start report.
use std::path::Path;

pub(crate) fn shell_word(text: &str) -> String {
    if !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+=:,@%".contains(c))
    {
        text.to_owned()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

pub(crate) fn apparmor_message(exe: &Path) -> String {
    let command = shell_word(&exe.to_string_lossy());
    format!(
        "Ubuntu's AppArmor restriction of unprivileged user namespaces may be why Mimik cannot create the private sandbox of the model server. An administrator can allow it once for this program: run `{command} assistant-apparmor-profile > mimik.apparmor && sudo install -m 0644 mimik.apparmor /etc/apparmor.d/mimik && sudo apparmor_parser -r /etc/apparmor.d/mimik`, then press Restart. The profile only gives this one program that permission; it does not confine or unconfine anything else."
    )
}

pub(crate) fn remedy(cause: Cause, exe: &Path) -> String {
    match cause {
        Cause::AppArmor => apparmor_message(exe),
        Cause::UsernsClone => "kernel.unprivileged_userns_clone = 0 may be why: it forbids the private sandbox of the model server. An administrator can allow it with `sudo sysctl kernel.unprivileged_userns_clone=1` (and keep it in a file under /etc/sysctl.d), then press Restart.".to_owned(),
    }
}

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
