//! Sandbox-only tests, run in a container with user namespaces (they fail where namespaces
//! are unavailable, e.g. an unprivileged CI sandbox): the model-server sandbox cannot read
//! `<data>/secrets/reviewer.key`. The test builds the real plan of the sandbox (`hako::plan`)
//! over a data folder that holds the key and runs the plan through the runner, the way
//! production launches it, with this test binary bound as the plan's runtime: inside the
//! sandbox it re-runs the `reader_helper` test, which reads the path named by
//! `MIMIK_TEST_READ` and exits 0 when the file can be read, 1 when it cannot. The key is
//! a dummy; no environment key is read.
//!
//! Not covered: GPU binds and `extra` binds (the plan here is built with `gpu: None` and no
//! extras), and a data folder in an unusual place (the data folder is a tempdir).
use super::key::{DIRECTORY, KEY_FILE};
use crate::Store;
use crate::assistant::runtime::hako;
use crate::sandbox::{
    Plan, Secret,
    runner::{self, Exec, RunnerCommand},
    test_helper,
};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

const KEY: &str = "dummy-key-not-a-secret";
/// The environment variable naming the path the payload inside the sandbox reads.
const READ: &str = "MIMIK_TEST_READ";

/// The runner helper: the test binary re-executed with this test's name and
/// `MIMIK_TEST_HELPER=reviewer-sandbox-runner` runs the runner.
#[test]
#[ignore = "live: sandbox (user namespaces)"]
fn runner_helper() {
    if let Some(mode) = test_helper::mode() {
        assert_eq!(mode, "reviewer-sandbox-runner");
        std::process::exit(runner::run(runner::SPEC_FD));
    }
}

/// The payload helper: the test binary re-executed inside the sandbox as the plan's
/// program, with `MIMIK_TEST_HELPER=reviewer-sandbox-reader`, reads `MIMIK_TEST_READ` and
/// exits 0 when the file can be read, 1 when it cannot.
#[test]
#[ignore = "live: sandbox (user namespaces)"]
fn reader_helper() {
    if let Some(mode) = test_helper::mode() {
        assert_eq!(mode, "reviewer-sandbox-reader");
        let path = std::env::var(READ).unwrap();
        std::process::exit(i32::from(std::fs::read(&path).is_err()));
    }
}

fn helper_args() -> Vec<String> {
    [
        "sources::reviewer::sandbox_tests::runner_helper",
        "--exact",
        "--nocapture",
        "--test-threads=1",
        "--include-ignored",
    ]
    .map(String::from)
    .to_vec()
}

/// The arguments that re-execute this test binary as the reader inside the sandbox.
fn reader_args() -> Vec<String> {
    [
        "sources::reviewer::sandbox_tests::reader_helper",
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
        args: helper_args().into_iter().map(OsString::from).collect(),
        env: vec![(
            test_helper::VARIABLE.into(),
            "reviewer-sandbox-runner".into(),
        )],
    }
}

/// The level-1 exec the runner runs: the plan with the handed-over descriptor numbers.
fn exec(plan: Plan) -> Exec {
    Exec::Hakoniwa {
        plan: Box::new(plan),
        status: test_helper::STATUS_FD,
        marker: test_helper::MARKER_FD,
        ack: test_helper::ACK_FD,
    }
}

/// The model-server sandbox's real plan over `data` (its bind list), with this test binary
/// bound as the runtime the plan executes; the payload reads `read` (the model's
/// in-sandbox path for the sanity run, the secrets file's host path for the other).
fn model_server_plan(data: &Path, root: &Path, read: &str) -> Plan {
    let runtime = std::env::current_exe().unwrap();
    let model = data.join("models").join("model.gguf");
    std::fs::create_dir_all(model.parent().unwrap()).unwrap();
    std::fs::write(&model, b"fake model").unwrap();
    let generation = data.join("run").join("1");
    std::fs::create_dir_all(&generation).unwrap();
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").unwrap();
    let runtime_target = format!(
        "{}/llama-server",
        crate::assistant::runtime::sandbox::RUNTIME
    );
    let inputs = hako::Inputs {
        root: root.to_path_buf(),
        mountinfo,
        runtime: vec![hako::File {
            name: "llama-server".to_owned(),
            source: runtime.clone(),
            identity: test_helper::identity(&runtime),
        }],
        model: (model.clone(), test_helper::identity(&model)),
        generation: (generation.clone(), test_helper::identity(&generation)),
        token: Secret::new("token\n".to_owned()),
        extra: Vec::new(),
        gpu: None,
        env: vec![
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            (
                test_helper::VARIABLE.to_owned(),
                "reviewer-sandbox-reader".to_owned(),
            ),
            (READ.to_owned(), read.to_owned()),
        ],
        argv: std::iter::once(runtime_target)
            .chain(reader_args())
            .collect(),
    };
    let plan = hako::plan(&inputs);
    plan.validate().unwrap();
    plan
}

/// The model-server sandbox (its real bind list, `hako::plan`) cannot read
/// `<data>/secrets/reviewer.key`. Needs user namespaces: fails where they are unavailable.
#[test]
#[ignore = "live: sandbox (user namespaces)"]
fn the_model_server_sandbox_cannot_read_the_secrets_folder() {
    let dir = tempfile::tempdir().unwrap();
    let data: PathBuf = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let store = Store::open(&data.join("private.sqlite3")).unwrap();
    store.secrets().write(KEY.as_bytes()).unwrap();
    let secret = data.join(DIRECTORY).join(KEY_FILE);
    assert!(secret.exists());
    let root = dir.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    // Sanity first: the payload can read a file the plan really binds (the model).
    let plan = model_server_plan(&data, &root, crate::assistant::runtime::sandbox::MODEL);
    test_helper::plan_stays_away(&plan, &secret);
    let (code, report) = test_helper::run_confined(&runner_command(), exec(plan));
    assert_eq!(
        code, 0,
        "sanity: the bound model file is readable inside the sandbox\nrunner output:\n{report}"
    );
    // The secrets file is not: its host path is not bound and no rule grants it.
    let plan = model_server_plan(&data, &root, &crate::sandbox::text(&secret));
    test_helper::plan_stays_away(&plan, &secret);
    let (code, report) = test_helper::run_confined(&runner_command(), exec(plan));
    assert_eq!(
        code, 1,
        "the secrets file is not readable inside the model-server sandbox\nrunner output:\n{report}"
    );
}
