//! Re-executing the test binary as a helper process (plan §1.4a: "the adversarial helper is
//! that same binary re-executed with a helper flag"). A helper is an ordinary test that does
//! nothing unless `MIMIK_TEST_HELPER` names it; re-executed with `--exact <its name>`, it runs
//! alone in a fresh process, prints one `HELPER <json>` line and exits.
//!
//! Also the shared harness of the secrets-sandbox live tests (the model-server's and the PDF
//! worker's): [`run_confined`] runs a plan through the real runner path and reports what the
//! runner said, [`plan_stays_away`] checks that neither a bind nor the plan's root can reach
//! the secrets file, and [`identity`] is the bind identity the launcher verifies.
use super::{
    Identity, Plan,
    runner::{self, Exec, Handover, RunnerCommand, Spec},
};
use serde_json::Value;
use std::os::fd::RawFd;
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

pub(crate) const VARIABLE: &str = "MIMIK_TEST_HELPER";

/// The helper mode this process was started for, if any.
pub(crate) fn mode() -> Option<String> {
    std::env::var(VARIABLE).ok()
}

/// The command that runs test `test` (its full path) as helper `mode`.
pub(crate) fn command(test: &str, mode: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().expect("the test binary"));
    command
        .args([
            test,
            "--exact",
            "--nocapture",
            "--test-threads=1",
            "--include-ignored",
        ])
        .env(VARIABLE, mode);
    command
}

/// Runs test `test` as helper `mode` and returns its output.
pub(crate) fn run(test: &str, mode: &str) -> Output {
    command(test, mode).output().expect("the helper starts")
}

/// The helper's `HELPER <json>` line.
pub(crate) fn report(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .find_map(|line| line.find("HELPER ").map(|at| &line[at + 7..]))
        .unwrap_or_else(|| {
            panic!(
                "no helper report; status {:?}, stdout {stdout}, stderr {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )
        });
    serde_json::from_str(line).expect("the helper report is JSON")
}

/// Prints the helper's report (on stdout, flushed: the helper may exec or exit next).
pub(crate) fn print(report: &Value) {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "HELPER {report}").expect("stdout");
    stdout.flush().expect("stdout");
}

/// The errno of the last failed call, or 0 when `result` is not negative.
pub(crate) fn errno(result: i64) -> i64 {
    if result < 0 {
        i64::from(std::io::Error::last_os_error().raw_os_error().unwrap_or(-1))
    } else {
        0
    }
}

/// The most bytes of a helper's output a failure message keeps.
pub(crate) const TAIL: usize = 4096;

/// Reads `read` to EOF, keeps only its last [`TAIL`] bytes and sends `(label, text)` on
/// `send`. The buffer never holds more than the tail plus one chunk, so a chatty helper
/// cannot grow it without bound.
pub(crate) fn tail(
    label: &'static str,
    mut read: impl std::io::Read,
    send: &std::sync::mpsc::Sender<(&'static str, String)>,
) {
    let mut kept: Vec<u8> = Vec::new();
    let mut chunk = [0_u8; TAIL];
    loop {
        match std::io::Read::read(&mut read, &mut chunk) {
            Ok(0) => break,
            Ok(count) => {
                kept.extend_from_slice(&chunk[..count]);
                if kept.len() > TAIL {
                    kept.drain(..kept.len() - TAIL);
                }
            }
            // An interrupted read is retried; any other error ends the drain.
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    let _ = send.send((label, String::from_utf8_lossy(&kept).into_owned()));
}

/// The descriptor numbers the level-1 handovers land on: the status, marker and
/// acknowledgement pipes (production hands them over at the same numbers).
pub(crate) const STATUS_FD: RawFd = 4;
pub(crate) const MARKER_FD: RawFd = 12;
pub(crate) const ACK_FD: RawFd = 13;

/// How long [`run_confined`] waits for the runner before killing it.
const RUN_TIMEOUT: Duration = Duration::from_secs(120);
/// How long [`run_confined`] waits for each drained report after the runner ended.
const REPORT_TIMEOUT: Duration = Duration::from_secs(5);

/// The `(device, inode)` of a host file: the identity the launcher verifies binds with.
pub(crate) fn identity(path: &Path) -> Identity {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).unwrap();
    (metadata.dev(), metadata.ino())
}

/// No bind of `plan`, and not its root either, covers `secret` or a folder above it:
/// through them the sandbox could reach the secrets file.
pub(crate) fn plan_stays_away(plan: &Plan, secret: &Path) {
    let root = Path::new(&plan.root);
    assert!(
        !secret.starts_with(root) && !root.starts_with(secret),
        "the plan root covers the secrets file: {}",
        plan.root
    );
    for bind in &plan.binds {
        let source = Path::new(&bind.source);
        assert!(
            !secret.starts_with(source) && !source.starts_with(secret),
            "a bind covers the secrets file: {}",
            bind.source
        );
    }
}

/// Runs `exec` (a level-1 plan) through the real runner path — `runner::launch` with
/// `runner` as the runner command — and returns the payload's exit code with the runner's
/// own report (its status line, stdout and stderr, each bounded to its last [`TAIL`]
/// bytes), so a failure states its reason. The acknowledgement byte is written up front,
/// as Mimik writes it once the started line arrives; the pipes' read ends stay open and
/// are drained for the runner's whole life, so its start report never fails with EPIPE
/// (that would end the run as a SPEC refusal, not the payload's exit).
pub(crate) fn run_confined(runner: &RunnerCommand, exec: Exec) -> (i32, String) {
    let (status, marker, ack) = match &exec {
        Exec::Hakoniwa {
            status,
            marker,
            ack,
            ..
        }
        | Exec::PdfWorker {
            status,
            marker,
            ack,
            ..
        } => (*status, *marker, *ack),
        Exec::Server { .. } => panic!("run_confined runs a level-1 exec"),
    };
    assert!(
        [status, marker, ack] == [STATUS_FD, MARKER_FD, ACK_FD],
        "the exec's handed-over descriptors are not the level-1 numbers"
    );
    // Close-on-exec, as production creates these pipes (assistant/runtime/launch.rs,
    // sources/worker/launch.rs): the runner receives them through launch's dup2, which
    // clears the flag on the target copy.
    let pipe = || rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
    let (status_read, status_write) = pipe();
    let (_marker_read, marker_write) = pipe();
    let (ack_read, ack_write) = pipe();
    rustix::io::write(&ack_write, b"k").unwrap();
    let spec = Spec {
        parent: std::process::id(),
        // The handed-over status, marker and acknowledgement descriptors stay open.
        keep: vec![STATUS_FD, MARKER_FD, ACK_FD],
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        chdir: None,
        exec,
    };
    let handover = [
        Handover {
            fd: status_write,
            target: STATUS_FD,
        },
        Handover {
            fd: marker_write,
            target: MARKER_FD,
        },
        Handover {
            fd: ack_read,
            target: ACK_FD,
        },
    ];
    let mut child = runner::launch(runner, &spec, &handover).unwrap();
    drop(handover);
    // The runner writes its start report on the status pipe and its refusal reasons on
    // stderr: their read ends stay open and are drained (bounded), so a write never fails
    // with EPIPE and a failure's message carries what the runner said.
    let (send, reported) = std::sync::mpsc::channel();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    std::thread::spawn({
        let send = send.clone();
        move || tail("stdout", stdout, &send)
    });
    std::thread::spawn({
        let send = send.clone();
        move || tail("stderr", stderr, &send)
    });
    std::thread::spawn(move || {
        tail("status", std::fs::File::from(status_read), &send);
    });
    let code = reap(&mut child);
    let mut parts = Vec::new();
    for _ in 0..3 {
        match reported.recv_timeout(REPORT_TIMEOUT) {
            Ok((label, text)) => parts.push(format!("{label} (last {TAIL} bytes): {text}")),
            Err(_) => break,
        }
    }
    (code, parts.join("\n"))
}

/// Waits for the runner child, killing it at [`RUN_TIMEOUT`]: only this thread reaps it
/// (through `try_wait`/`wait` below), and an unreaped child's pid is still ours, so the
/// kill cannot hit a recycled pid.
fn reap(child: &mut std::process::Child) -> i32 {
    let deadline = std::time::Instant::now() + RUN_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.code().unwrap_or(-1),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                return child.wait().unwrap().code().unwrap_or(-1);
            }
            Err(error) => panic!("waiting for the runner: {error}"),
        }
    }
}
