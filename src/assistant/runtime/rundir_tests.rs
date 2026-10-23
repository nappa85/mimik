use super::*;
use std::os::unix::fs::PermissionsExt;

fn data() -> (tempfile::TempDir, OwnedFd) {
    let dir = tempfile::tempdir().unwrap();
    let run = open(dir.path()).unwrap();
    (dir, run)
}

#[test]
fn run_is_created_private_and_checked() {
    let (dir, _run) = data();
    let mode = std::fs::metadata(dir.path().join("run"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);
    std::fs::set_permissions(
        dir.path().join("run"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(matches!(open(dir.path()), Err(Refusal::Mode(_))));
}

#[test]
fn a_second_holder_cannot_take_the_pidfile() {
    let (_dir, run) = data();
    let first = Pidfile::acquire(run.as_fd()).unwrap().unwrap();
    assert_eq!(Pidfile::acquire(run.as_fd()).unwrap().unwrap_err(), Busy);
    drop(first);
    // A concurrent test's fork may hold the descriptor until its exec closes it.
    let mut again = false;
    for _ in 0..100 {
        if Pidfile::acquire(run.as_fd()).unwrap().is_ok() {
            again = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(again);
}

#[test]
fn entries_round_trip() {
    let (_dir, run) = data();
    let mut pidfile = Pidfile::acquire(run.as_fd()).unwrap().unwrap();
    assert_eq!(pidfile.leftover(), None);
    let entry = Entry {
        generation: 3,
        pid: 42,
        start_time: 99,
        executables: vec!["/usr/bin/bwrap".into(), "/opt/mimik/mimik".into()],
    };
    pidfile.record(&entry).unwrap();
    assert_eq!(pidfile.leftover(), Some(entry));
    pidfile.clear().unwrap();
    assert_eq!(pidfile.leftover(), None);
}

fn sleeper() -> std::process::Child {
    std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap()
}

fn gone(child: &mut std::process::Child) -> bool {
    for _ in 0..100 {
        if child.try_wait().unwrap().is_some() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

#[test]
fn a_leftover_is_killed_only_when_pid_start_time_and_path_match() {
    let (_dir, run) = data();
    let mut pidfile = Pidfile::acquire(run.as_fd()).unwrap().unwrap();
    let mut child = sleeper();
    let pid = i32::try_from(child.id()).unwrap();
    let exe = executable(pid).unwrap();
    // A reused PID: another start time.
    let mut entry = Entry::of(1, pid, vec![exe.clone()]).unwrap();
    entry.start_time += 1;
    pidfile.record(&entry).unwrap();
    assert_eq!(pidfile.sweep(), Swept::Nothing);
    assert!(child.try_wait().unwrap().is_none());
    // Another executable.
    let entry = Entry::of(1, pid, vec!["/usr/bin/bwrap".into()]).unwrap();
    pidfile.record(&entry).unwrap();
    assert_eq!(pidfile.sweep(), Swept::Nothing);
    assert!(child.try_wait().unwrap().is_none());
    // All three match.
    let entry = Entry::of(1, pid, vec!["/usr/bin/bwrap".into(), exe]).unwrap();
    pidfile.record(&entry).unwrap();
    assert_eq!(pidfile.sweep(), Swept::Killed);
    assert!(gone(&mut child));
    assert_eq!(pidfile.leftover(), None);
}

#[test]
fn generation_directories_are_fresh_private_and_removed() {
    let (dir, run) = data();
    let generation = Generation::create(&run, "1").unwrap();
    let path = dir.path().join("run/1");
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o700
    );
    std::fs::write(path.join("written-by-the-server"), b"x").unwrap();
    std::fs::create_dir_all(path.join("home/.cache/deeper")).unwrap();
    std::fs::write(path.join("home/.cache/deeper/file"), b"x").unwrap();
    std::os::unix::fs::symlink(dir.path(), path.join("home/link-out")).unwrap();
    drop(generation);
    // The link was removed, not followed.
    assert!(dir.path().join("run").exists());
    assert!(!path.exists());

    // Already there (any kind): refused.
    std::fs::create_dir(dir.path().join("run/2")).unwrap();
    assert!(Generation::create(&run, "2").is_err());
    std::os::unix::fs::symlink(dir.path(), dir.path().join("run/3")).unwrap();
    assert!(Generation::create(&run, "3").is_err());
    std::fs::write(dir.path().join("run/4"), b"").unwrap();
    assert!(Generation::create(&run, "4").is_err());

    // The sweep at start removes them, unlinking the link (not its target), and keeps the
    // pidfile.
    let _pidfile = Pidfile::acquire(run.as_fd()).unwrap().unwrap();
    sweep_generations(run.as_fd());
    for name in ["2", "3", "4"] {
        assert!(
            std::fs::symlink_metadata(dir.path().join("run").join(name)).is_err(),
            "{name}"
        );
    }
    assert!(dir.path().exists());
    assert!(dir.path().join("run").join(PIDFILE).exists());
    assert!(Generation::create(&run, "2").is_ok());
}

#[test]
fn a_leftover_that_exits_between_the_pidfd_and_the_check_is_never_signalled_twice() {
    let (_dir, run) = data();
    let mut pidfile = Pidfile::acquire(run.as_fd()).unwrap().unwrap();
    let mut leftover = sleeper();
    let pid = i32::try_from(leftover.id()).unwrap();
    let exe = executable(pid).unwrap();
    pidfile
        .record(&Entry::of(1, pid, vec![exe]).unwrap())
        .unwrap();
    let mut bystander = sleeper();
    // The leftover ends (and is reaped) after the pidfd is open, before the identity check:
    // nothing is killed, and no other process is touched.
    let killed = pidfile.sweep_with(
        || {
            leftover.kill().unwrap();
            leftover.wait().unwrap();
        },
        |_| true,
    );
    assert_eq!(killed, Swept::Nothing);
    assert!(bystander.try_wait().unwrap().is_none());
    assert_eq!(pidfile.leftover(), None);
    bystander.kill().unwrap();
    bystander.wait().unwrap();
}

#[test]
fn the_identity_is_checked_after_the_pidfd_is_open() {
    let (_dir, run) = data();
    let mut pidfile = Pidfile::acquire(run.as_fd()).unwrap().unwrap();
    let mut leftover = sleeper();
    let pid = i32::try_from(leftover.id()).unwrap();
    let exe = executable(pid).unwrap();
    pidfile
        .record(&Entry::of(1, pid, vec![exe]).unwrap())
        .unwrap();
    // The recorded process stops being the recorded program once the pidfd is open (here:
    // it ends, unreaped): the check made after that sees it, so nothing is signalled.
    let killed = pidfile.sweep_with(
        || {
            leftover.kill().unwrap();
            let stat = format!("/proc/{pid}/stat");
            while !std::fs::read_to_string(&stat).is_ok_and(|text| text.contains(") Z ")) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        },
        |_| true,
    );
    assert_eq!(killed, Swept::Nothing);
    leftover.wait().unwrap();
}

#[test]
fn a_killed_leftover_is_gone_when_the_sweep_returns() {
    let (_dir, run) = data();
    let mut pidfile = Pidfile::acquire(run.as_fd()).unwrap().unwrap();
    let mut leftover = sleeper();
    let pid = i32::try_from(leftover.id()).unwrap();
    let exe = executable(pid).unwrap();
    pidfile
        .record(&Entry::of(1, pid, vec![exe]).unwrap())
        .unwrap();
    assert_eq!(pidfile.sweep(), Swept::Killed);
    // Already ended: the status is there at once.
    assert!(leftover.try_wait().unwrap().is_some());
}

#[test]
fn a_killed_leftover_not_yet_ended_keeps_its_entry() {
    let (_dir, run) = data();
    let mut pidfile = Pidfile::acquire(run.as_fd()).unwrap().unwrap();
    let mut leftover = sleeper();
    let pid = i32::try_from(leftover.id()).unwrap();
    let entry = Entry::of(1, pid, vec![executable(pid).unwrap()]).unwrap();
    pidfile.record(&entry).unwrap();
    // The wait reports it still running (as a process slow to end would be).
    assert_eq!(pidfile.sweep_with(|| {}, |_| false), Swept::StillRunning);
    assert_eq!(pidfile.leftover(), Some(entry));
    leftover.wait().unwrap();
}

/// A model of the process table where a PID changes hands: each process has an id; a handle
/// pins the process that had the PID when it was opened.
struct Reused {
    /// The process holding the recorded PID now: 1 (the leftover) or 2 (a newcomer).
    holder: std::cell::Cell<u32>,
    signalled: std::cell::RefCell<Vec<u32>>,
}

impl Processes for Reused {
    type Handle = u32;

    fn open(&self, _: i32) -> std::io::Result<u32> {
        Ok(self.holder.get())
    }

    fn identity(&self, _: i32) -> (Option<u64>, Option<String>) {
        match self.holder.get() {
            1 => (Some(100), Some("/opt/mimik/mimik".into())),
            _ => (Some(200), Some("/usr/bin/other".into())),
        }
    }

    fn kill(&self, handle: &u32) -> std::io::Result<()> {
        self.signalled.borrow_mut().push(*handle);
        Ok(())
    }

    fn alive(&self, _: &u32) -> bool {
        false
    }
}

#[test]
fn a_pid_taken_over_by_another_process_is_never_signalled() {
    let (_dir, run) = data();
    let mut pidfile = Pidfile::acquire(run.as_fd()).unwrap().unwrap();
    let entry = Entry {
        generation: 1,
        pid: 4242,
        start_time: 100,
        executables: vec!["/opt/mimik/mimik".into()],
    };
    pidfile.record(&entry).unwrap();
    let processes = Reused {
        holder: std::cell::Cell::new(1),
        signalled: std::cell::RefCell::new(Vec::new()),
    };
    // The leftover ends and another process takes its PID, after the handle is opened.
    let outcome = pidfile.sweep_in(&processes, || processes.holder.set(2), |_| true);
    assert_eq!(outcome, Swept::Nothing);
    assert_eq!(processes.signalled.borrow().as_slice(), &[] as &[u32]);
    // Without a takeover the leftover itself is the one signalled.
    pidfile.record(&entry).unwrap();
    processes.holder.set(1);
    assert_eq!(pidfile.sweep_in(&processes, || {}, |_| true), Swept::Killed);
    assert_eq!(processes.signalled.borrow().as_slice(), &[1]);
}
