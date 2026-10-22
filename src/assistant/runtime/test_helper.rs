//! Re-executing the test binary as a helper process (plan §1.4a: "the adversarial helper is
//! that same binary re-executed with a helper flag"). A helper is an ordinary test that does
//! nothing unless `MIMIK_TEST_HELPER` names it; re-executed with `--exact <its name>`, it runs
//! alone in a fresh process, prints one `HELPER <json>` line and exits.
use serde_json::Value;
use std::process::{Command, Output};

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
