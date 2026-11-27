//! Sandbox-only test, run in a container with user namespaces (it fails where namespaces are
//! unavailable, e.g. an unprivileged CI sandbox): the PDF worker sandbox (its real profile,
//! `profile::build`) cannot read `<data>/secrets/reviewer.key`. The test builds the real
//! profile over a data folder that holds the key and runs the plan through the runner, the
//! way production launches it, with this test binary as the worker executable (loaded by
//! the bound loader, as in the live suite): inside the sandbox it re-runs the
//! `reader_helper` test, which reads the path named by `MIMIK_TEST_READ` and exits 0 when
//! the file can be read, 1 when it cannot. The key is a dummy; no environment key is read.
//!
//! Not covered: GPU binds and `extra` binds (the worker profile has neither), and a data
//! folder in an unusual place (the data folder is a tempdir).
use super::profile;
use crate::Store;
use crate::sandbox::{
    Plan,
    runner::{self, Exec, RunnerCommand},
    test_helper,
};
use crate::sources::reviewer::key::{DIRECTORY, KEY_FILE};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

const KEY: &str = "dummy-key-not-a-secret";
/// The environment variable naming the path the payload inside the sandbox reads.
const READ: &str = "MIMIK_TEST_READ";

/// An existing dynamic loader (the profile binds it at its loader target).
fn loader() -> &'static Path {
    for path in [
        "/lib64/ld-linux-x86-64.so.2",
        "/lib/ld-linux-aarch64.so.1",
        "/usr/lib/ld.so",
    ] {
        if Path::new(path).exists() {
            return Path::new(path);
        }
    }
    Path::new("/lib64/ld-linux-x86-64.so.2")
}

/// The runner helper: the test binary re-executed with this test's name and
/// `MIMIK_TEST_HELPER=worker-secret-runner` runs the runner.
#[test]
#[ignore = "live: sandbox (user namespaces)"]
fn runner_helper() {
    if let Some(mode) = test_helper::mode() {
        assert_eq!(mode, "worker-secret-runner");
        std::process::exit(runner::run(runner::SPEC_FD));
    }
}

/// The payload helper: the test binary re-executed inside the sandbox as the worker, with
/// `MIMIK_TEST_HELPER=worker-secret-reader`, reads `MIMIK_TEST_READ` and exits 0 when the
/// file can be read, 1 when it cannot.
#[test]
#[ignore = "live: sandbox (user namespaces)"]
fn reader_helper() {
    if let Some(mode) = test_helper::mode() {
        assert_eq!(mode, "worker-secret-reader");
        let path = std::env::var(READ).unwrap();
        std::process::exit(i32::from(std::fs::read(&path).is_err()));
    }
}

/// The arguments that re-execute this test binary as the reader inside the sandbox.
fn reader_args() -> Vec<String> {
    [
        "sources::worker::secret_tests::reader_helper",
        "--exact",
        "--nocapture",
        "--test-threads=1",
        "--include-ignored",
    ]
    .map(String::from)
    .to_vec()
}

fn runner_command() -> RunnerCommand {
    RunnerCommand {
        program: std::env::current_exe().unwrap(),
        args: [
            "sources::worker::secret_tests::runner_helper",
            "--exact",
            "--nocapture",
            "--test-threads=1",
            "--include-ignored",
        ]
        .into_iter()
        .map(OsString::from)
        .collect(),
        env: vec![(test_helper::VARIABLE.into(), "worker-secret-runner".into())],
    }
}

/// The level-1 exec the runner runs: the plan as a PDF worker, with the handed-over
/// descriptor numbers.
fn exec(plan: Plan) -> Exec {
    Exec::PdfWorker {
        plan: Box::new(plan),
        status: test_helper::STATUS_FD,
        marker: test_helper::MARKER_FD,
        ack: test_helper::ACK_FD,
        cpu_seconds: 10,
    }
}

/// The PDF worker's real profile over `data`, with this test binary as the worker
/// executable; the payload reads `read` (the input document's in-sandbox path for the
/// sanity run, the secrets file's host path for the other).
fn worker_plan(data: &Path, root: &Path, read: &str) -> Plan {
    let input = data.join("sources").join("document.pdf");
    std::fs::create_dir_all(input.parent().unwrap()).unwrap();
    std::fs::write(&input, b"%PDF-1.4 fake").unwrap();
    let executable = std::env::current_exe().unwrap();
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").unwrap();
    let inputs = profile::Inputs {
        root,
        executable: (&executable, test_helper::identity(&executable)),
        input: (&input, test_helper::identity(&input)),
        loader: loader(),
        spec: "{}".to_owned(),
        mountinfo: &mountinfo,
        argv: std::iter::once(profile::EXECUTABLE.to_owned())
            .chain(reader_args())
            .collect(),
        env: vec![
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("HOME".to_owned(), "/tmp".to_owned()),
            (
                test_helper::VARIABLE.to_owned(),
                "worker-secret-reader".to_owned(),
            ),
            (READ.to_owned(), read.to_owned()),
        ],
    };
    let plan = profile::build(inputs);
    plan.validate().unwrap();
    plan
}

/// The PDF worker sandbox cannot read `<data>/secrets/reviewer.key`. Needs user namespaces:
/// fails where they are unavailable.
#[test]
#[ignore = "live: sandbox (user namespaces)"]
fn the_pdf_worker_sandbox_cannot_read_the_secrets_folder() {
    let dir = tempfile::tempdir().unwrap();
    let data: PathBuf = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let store = Store::open(&data.join("private.sqlite3")).unwrap();
    store.secrets().write(KEY.as_bytes()).unwrap();
    let secret = data.join(DIRECTORY).join(KEY_FILE);
    assert!(secret.exists());
    let root = dir.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    // Sanity first: the payload can read the bound input document inside the sandbox.
    let plan = worker_plan(&data, &root, profile::INPUT);
    test_helper::plan_stays_away(&plan, &secret);
    let (code, report) = test_helper::run_confined(&runner_command(), exec(plan));
    assert_eq!(
        code, 0,
        "sanity: the bound input document is readable inside the worker sandbox\nrunner output:\n{report}"
    );
    // The secrets file is not: its host path is not bound and no rule grants it.
    let plan = worker_plan(&data, &root, &crate::sandbox::text(&secret));
    test_helper::plan_stays_away(&plan, &secret);
    let (code, report) = test_helper::run_confined(&runner_command(), exec(plan));
    assert_eq!(
        code, 1,
        "the secrets file is not readable inside the PDF worker sandbox\nrunner output:\n{report}"
    );
}
