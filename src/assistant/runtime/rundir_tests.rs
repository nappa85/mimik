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
    assert!(!pidfile.sweep());
    assert!(child.try_wait().unwrap().is_none());
    // Another executable.
    let entry = Entry::of(1, pid, vec!["/usr/bin/bwrap".into()]).unwrap();
    pidfile.record(&entry).unwrap();
    assert!(!pidfile.sweep());
    assert!(child.try_wait().unwrap().is_none());
    // All three match.
    let entry = Entry::of(1, pid, vec!["/usr/bin/bwrap".into(), exe]).unwrap();
    pidfile.record(&entry).unwrap();
    assert!(pidfile.sweep());
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
    let killed = pidfile.sweep_with(|| {
        leftover.kill().unwrap();
        leftover.wait().unwrap();
    });
    assert!(!killed);
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
    let killed = pidfile.sweep_with(|| {
        leftover.kill().unwrap();
        let stat = format!("/proc/{pid}/stat");
        while !std::fs::read_to_string(&stat).is_ok_and(|text| text.contains(") Z ")) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    });
    assert!(!killed);
    leftover.wait().unwrap();
}

const HEAVY: &str = "assistant::runtime::rundir::tests::heavy_helper";

/// A leftover whose exit takes a while: 128 MiB of touched memory to tear down.
#[test]
fn heavy_helper() {
    if super::super::test_helper::mode().as_deref() != Some("heavy") {
        return;
    }
    let mut memory = vec![0_u8; 128 << 20];
    for page in memory.chunks_mut(4096) {
        page[0] = 1;
    }
    println!("HELPER {{\"ready\":{}}}", memory.len());
    std::thread::sleep(std::time::Duration::from_secs(30));
    std::hint::black_box(&memory);
}

#[test]
fn a_killed_leftover_is_gone_when_the_sweep_returns() {
    let (_dir, run) = data();
    let mut pidfile = Pidfile::acquire(run.as_fd()).unwrap().unwrap();
    let mut leftover = super::super::test_helper::command(HEAVY, "heavy")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Its memory is touched once it reports.
    let stdout = std::io::BufReader::new(leftover.stdout.take().unwrap());
    assert!(
        std::io::BufRead::lines(stdout)
            .map_while(Result::ok)
            .any(|line| line.contains("HELPER"))
    );
    let pid = i32::try_from(leftover.id()).unwrap();
    let exe = executable(pid).unwrap();
    pidfile
        .record(&Entry::of(1, pid, vec![exe]).unwrap())
        .unwrap();
    assert!(pidfile.sweep());
    // Already ended: the status is there at once.
    assert!(leftover.try_wait().unwrap().is_some());
}
