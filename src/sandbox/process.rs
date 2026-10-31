//! Process/descriptor observations shared by sandbox enforcement and its callers.
use std::os::fd::OwnedFd;

/// The open descriptors, from `/proc/self/fd`.
pub(crate) fn fd_listing() -> std::io::Result<Vec<i32>> {
    descriptors(
        std::fs::read_dir("/proc/self/fd")?
            .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned())),
    )
}

/// A partial listing must never leave a descriptor open.
pub(crate) fn descriptors(
    names: impl Iterator<Item = std::io::Result<String>>,
) -> std::io::Result<Vec<i32>> {
    names
        .map(|name| {
            name.and_then(|name| {
                name.parse()
                    .map_err(|_| std::io::Error::other("not a descriptor number"))
            })
        })
        .collect()
}

/// Parent and start time from a single observation of the process.
pub(crate) fn stat_of(pid: i32) -> std::io::Result<(i32, u64)> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    parse_stat(&text).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "an unreadable stat line")
    })
}

pub(crate) fn parse_stat(text: &str) -> Option<(i32, u64)> {
    let rest = text.rsplit(')').next()?;
    let mut fields = rest.split_whitespace();
    let parent = fields.nth(1)?.parse().ok()?;
    let start = fields.nth(17)?.parse().ok()?;
    Some((parent, start))
}

/// Whether a pidfd reports its process gone (it polls readable then).
pub(crate) fn exited(pidfd: &OwnedFd) -> bool {
    let mut fds = [rustix::event::PollFd::new(
        pidfd,
        rustix::event::PollFlags::IN,
    )];
    rustix::event::poll(&mut fds, Some(&rustix::time::Timespec::default()))
        .is_ok_and(|ready| ready > 0)
}
