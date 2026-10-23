//! The runner on the TCP level's exec path (level 1 runs bwrap and is a live test): the test
//! binary plays the runner and the executed server.
use super::super::test_helper;
use super::*;
use serde_json::{Value, json};
use std::os::fd::AsFd;

const HELPER: &str = "assistant::runtime::runner::tests::helper";

fn helper_args() -> Vec<String> {
    [
        HELPER,
        "--exact",
        "--nocapture",
        "--test-threads=1",
        "--include-ignored",
    ]
    .map(String::from)
    .to_vec()
}

/// The test binary as the runner.
fn runner() -> RunnerCommand {
    RunnerCommand {
        program: std::env::current_exe().unwrap(),
        args: helper_args().into_iter().map(OsString::from).collect(),
        env: vec![(test_helper::VARIABLE.into(), "runner".into())],
    }
}

/// The test binary's own descriptor, as the "server" to execute.
fn own_binary() -> OwnedFd {
    rustix::fs::open(
        "/proc/self/exe",
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .unwrap()
}

const SERVER_FD: RawFd = 9;
const KEPT_FD: RawFd = 8;

/// A spec executing the test binary as helper `mode` with the minimal environment.
fn spec(parent: u32, mode: &str) -> Spec {
    let mut argv = vec!["llama-server".to_owned()];
    argv.extend(helper_args());
    Spec {
        parent,
        keep: vec![KEPT_FD],
        env: vec![
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("LANG".into(), "C.UTF-8".into()),
            ("HOME".into(), "/nonexistent-private-home".into()),
            (test_helper::VARIABLE.into(), mode.into()),
        ],
        exec: Exec::Server {
            fd: SERVER_FD,
            argv,
        },
    }
}

fn handover() -> Vec<Handover> {
    let kept = rustix::fs::open(
        "/dev/null",
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .unwrap();
    vec![
        Handover {
            fd: kept,
            target: KEPT_FD,
        },
        Handover {
            fd: own_binary(),
            target: SERVER_FD,
        },
    ]
}

fn own_pid() -> u32 {
    std::process::id()
}

fn limit(resource: rustix::process::Resource) -> Value {
    let limit = rustix::process::getrlimit(resource);
    json!([limit.current, limit.maximum])
}

fn report() -> Value {
    let env: Vec<String> = std::env::vars_os()
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .collect();
    let fds: Vec<i64> = std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().to_str()?.parse().ok())
        .collect();
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let field = |name: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .map(|value| value.trim().to_owned())
    };
    let death = rustix::process::parent_process_death_signal()
        .ok()
        .flatten()
        .map(rustix::process::Signal::as_raw);
    // SAFETY: prctl with integer arguments; under the seccomp program it is refused.
    let clear = unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, 0, 0, 0, 0) };
    let clear = test_helper::errno(i64::from(clear));
    // A thread that tries to exec (it would become the leader without the parent-death
    // signal): refused.
    let exec_from_thread = std::thread::spawn(|| {
        let argv = [c"/bin/true".as_ptr(), std::ptr::null()];
        let envp = [std::ptr::null::<libc::c_char>()];
        // SAFETY: execve with NUL-terminated arrays; refused by the program.
        let result = unsafe { libc::execve(c"/bin/true".as_ptr(), argv.as_ptr(), envp.as_ptr()) };
        test_helper::errno(i64::from(result))
    })
    .join()
    .unwrap();
    json!({
        "env": env,
        "fds": fds,
        "core": limit(rustix::process::Resource::Core),
        "fsize": limit(rustix::process::Resource::Fsize),
        "nofile": limit(rustix::process::Resource::Nofile),
        "death_signal": death,
        "seccomp": field("Seccomp:"),
        "no_new_privs": field("NoNewPrivs:"),
        "clear_death_signal": clear,
        "exec_from_thread": exec_from_thread,
        "pid": own_pid(),
    })
}

#[test]
fn helper() {
    let Some(mode) = test_helper::mode() else {
        return;
    };
    match mode.as_str() {
        "runner" => std::process::exit(run(SPEC_FD)),
        "report" => test_helper::print(&report()),
        "hold" => {
            test_helper::print(&report());
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
        "fsize" => {
            let mut file = std::fs::File::create(
                std::env::temp_dir().join(format!("mimik-fsize-{}", own_pid())),
            )
            .unwrap();
            let chunk = vec![0_u8; 1 << 20];
            for _ in 0..80 {
                std::io::Write::write_all(&mut file, &chunk).unwrap();
            }
            test_helper::print(&json!({"wrote": 80}));
        }
        // Plays Mimik: launches the runner (holding), reports its PID, then waits.
        "mimik" => {
            let mut child = launch(&runner(), &spec(own_pid(), "hold"), &handover()).unwrap();
            let mut reader = std::io::BufReader::new(child.stdout.take().unwrap());
            let mut line = String::new();
            while !line.contains("HELPER ") {
                line.clear();
                if std::io::BufRead::read_line(&mut reader, &mut line).unwrap() == 0 {
                    break;
                }
            }
            let report: Value = line.find("HELPER ").map_or(Value::Null, |at| {
                serde_json::from_str(&line[at + 7..]).unwrap()
            });
            test_helper::print(&json!({"runner": child.id(), "report": report}));
            // The server holds for 60 s; this process is killed long before.
            let _ = child.wait();
        }
        other => panic!("unknown helper mode {other}"),
    }
}

fn wait(mut child: Child) -> (std::process::ExitStatus, String) {
    let mut out = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    (child.wait().unwrap(), out)
}

fn parsed(out: &str) -> Value {
    let line = out
        .lines()
        .find(|line| line.contains("HELPER "))
        .unwrap_or_else(|| panic!("{out}"));
    serde_json::from_str(&line[line.find("HELPER ").unwrap() + 7..]).unwrap()
}

#[test]
fn the_executed_server_has_only_the_allowed_environment_descriptors_and_limits() {
    let child = launch(&runner(), &spec(own_pid(), "report"), &handover()).unwrap();
    let (status, out) = wait(child);
    assert!(status.success(), "{status:?} {out}");
    let report = parsed(&out);
    let mut env: Vec<&str> = report["env"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    env.sort_unstable();
    assert_eq!(env, ["HOME", "LANG", test_helper::VARIABLE, "PATH"]);
    // 0, 1, 2, the kept descriptor, and the listing's own; the server's descriptor was
    // close-on-exec and the spec's was closed.
    let fds: Vec<i64> = report["fds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();
    // (The listing's own takes the lowest free number, 3, the spec's old number.)
    for fd in [0, 1, 2, 8] {
        assert!(fds.contains(&fd), "{fds:?}");
    }
    assert!(!fds.contains(&9));
    assert!(!fds.contains(&i64::from(EXEC_FD)));
    assert!(
        fds.iter().filter(|fd| **fd > 2 && **fd != 8).count() <= 1,
        "{fds:?}"
    );
    assert_eq!(report["core"], json!([0, 0]));
    assert_eq!(report["fsize"], json!([FSIZE, FSIZE]));
    assert_eq!(report["nofile"], json!([EXEC_FD, EXEC_FD]));
    // The parent-death signal is per thread and this report runs on libtest's test thread,
    // so it is not read here; `sigkill_of_mimik_kills_the_server` shows its effect.
    assert_eq!(report["seccomp"], json!("2"));
    assert_eq!(report["no_new_privs"], json!("1"));
    assert_eq!(report["clear_death_signal"], json!(libc::EPERM));
    assert_eq!(report["exec_from_thread"], json!(libc::EPERM));
}

#[test]
fn a_write_past_the_file_size_limit_kills_the_server_with_sigxfsz() {
    use std::os::unix::process::ExitStatusExt;
    let child = launch(&runner(), &spec(own_pid(), "fsize"), &handover()).unwrap();
    let pid = child.id();
    let (status, out) = wait(child);
    let _ = std::fs::remove_file(std::env::temp_dir().join(format!("mimik-fsize-{pid}")));
    assert_eq!(status.signal(), Some(libc::SIGXFSZ), "{status:?} {out}");
    assert!(!out.contains("\"wrote\""));
}

#[test]
fn a_runner_whose_parent_is_not_mimik_exits_before_its_exec() {
    let child = launch(&runner(), &spec(1, "report"), &handover()).unwrap();
    let (status, out) = wait(child);
    assert_eq!(status.code(), Some(exit::PARENT));
    assert!(!out.contains("HELPER "));
}

fn alive(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        !stat
            .rsplit(')')
            .next()
            .unwrap_or("")
            .trim_start()
            .starts_with('Z')
    })
}

#[test]
fn sigkill_of_mimik_kills_the_server() {
    let mut mimik = test_helper::command(HELPER, "mimik")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = std::io::BufReader::new(mimik.stdout.take().unwrap());
    let mut line = String::new();
    let runner = loop {
        line.clear();
        assert!(std::io::BufRead::read_line(&mut reader, &mut line).unwrap() > 0);
        if let Some(at) = line.find("HELPER ") {
            let value: Value = serde_json::from_str(&line[at + 7..]).unwrap();
            // The server is running: it reported from inside its exec.
            assert_eq!(value["report"]["seccomp"], json!("2"), "{value}");
            assert_eq!(value["report"]["clear_death_signal"], json!(libc::EPERM));
            break i32::try_from(value["runner"].as_i64().unwrap()).unwrap();
        }
    };
    assert!(alive(runner));
    mimik.kill().unwrap();
    mimik.wait().unwrap();
    let mut gone = false;
    for _ in 0..300 {
        if !alive(runner) {
            gone = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(gone, "the server outlived Mimik");
    let _ = reader.get_ref().as_fd();
}

#[test]
fn a_spec_larger_than_a_pipe_starts_the_runner() {
    let mut big = spec(own_pid(), "report");
    big.env.push(("PADDING".into(), "x".repeat(100 << 10)));
    let (sender, started) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(launch(&runner(), &big, &handover()));
    });
    let child = started
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("the launch blocked on the spec")
        .unwrap();
    let (status, out) = wait(child);
    assert!(status.success(), "{status:?} {out}");
}

#[test]
fn a_spec_or_handover_past_the_runners_bounds_is_refused() {
    let mut huge = spec(own_pid(), "report");
    huge.env.push(("PADDING".into(), "x".repeat(2 << 20)));
    let error = launch(&runner(), &huge, &handover()).unwrap_err();
    assert!(
        error.to_string().contains("more than the 1048576"),
        "{error}"
    );
    let many: Vec<Handover> = (0..MAX_HANDOVER)
        .map(|n| Handover {
            fd: own_binary(),
            target: 100 + i32::try_from(n).unwrap(),
        })
        .collect();
    let error = launch(&runner(), &spec(own_pid(), "report"), &many).unwrap_err();
    assert!(
        error.to_string().contains("more than the runner's 64"),
        "{error}"
    );
}
