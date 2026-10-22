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
    let mode = std::fs::metadata(dir.path().join("run")).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
    std::fs::set_permissions(dir.path().join("run"), std::fs::Permissions::from_mode(0o755))
        .unwrap();
    assert!(matches!(open(dir.path()), Err(Refusal::Mode(_))));
}

#[test]
fn a_second_holder_cannot_take_the_pidfile() {
    let (_dir, run) = data();
    let first = Pidfile::acquire(run.as_fd()).unwrap().unwrap();
    assert_eq!(Pidfile::acquire(run.as_fd()).unwrap().unwrap_err(), Busy);
    drop(first);
    assert!(Pidfile::acquire(run.as_fd()).unwrap().is_ok());
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
    std::process::Command::new("sleep").arg("30").spawn().unwrap()
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
    let generation = Generation::create(&run, 1).unwrap();
    let path = dir.path().join("run/1");
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o700
    );
    std::fs::write(path.join("written-by-the-server"), b"x").unwrap();
    drop(generation);
    assert!(!path.exists());

    // Already there (any kind): refused.
    std::fs::create_dir(dir.path().join("run/2")).unwrap();
    assert!(Generation::create(&run, 2).is_err());
    std::os::unix::fs::symlink(dir.path(), dir.path().join("run/3")).unwrap();
    assert!(Generation::create(&run, 3).is_err());
    std::fs::write(dir.path().join("run/4"), b"").unwrap();
    assert!(Generation::create(&run, 4).is_err());

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
    assert!(Generation::create(&run, 2).is_ok());
}
