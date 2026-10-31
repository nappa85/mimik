//! Live tests: the pinned official llama-server (CPU build) under the real profiles, with the
//! CI test model (`stories260K.gguf`). Ignored in ordinary runs; run with `--ignored` where
//! the sandbox works (the dev host, the `llama-runtime` CI job) and with:
//!
//! - `MIMIK_LIVE_RUNTIME`: the runtime folder (holding `cpu/` with exactly the pinned files);
//! - `MIMIK_LIVE_MODELS`: a 0700 models folder holding `stories260K.gguf`;
//! - `MIMIK_LIVE_SCRATCH`: a folder whose ancestors are not writable by others;
//! - `MIMIK_LIVE_FAKE_SERVER`: the `fake_llama_server` example binary (the forged-output and
//!   post-spawn-failure tests run it in place of llama-server).
//!
//! A missing variable fails the test (never a silent skip). The test binary plays the runner,
//! the adversarial helper and "Mimik" for the SIGKILL check.
use super::super::manager::Child;
use super::super::manifest::MANIFEST;
use super::super::server::Health;
use super::super::test_helper;
use super::*;
use crate::assistant::transport::Bounded;
use rig_core::http_client::HttpClientExt;
use serde_json::{Value, json};

const HELPER: &str = "assistant::runtime::launch::live_tests::helper";
const RUNNER: &str = "sandbox::runner::tests::helper";

fn env(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("{name} is not set")))
}

fn helper_args(test: &str) -> Vec<String> {
    [
        test,
        "--exact",
        "--nocapture",
        "--test-threads=1",
        "--include-ignored",
    ]
    .map(String::from)
    .to_vec()
}

/// Settings over the live runtime and test model; `inner` is the helper mode run in place
/// of the probe (or `probe-inner`).
fn settings(scratch: &std::path::Path, inner: &str, inner_env: Vec<(String, String)>) -> Settings {
    let exe = std::env::current_exe().unwrap();
    let mut args = helper_args(HELPER);
    args.push("probe-inner".into());
    let mut env_vars = vec![(test_helper::VARIABLE.to_owned(), inner.to_owned())];
    env_vars.extend(inner_env);
    let runner_args: Vec<std::ffi::OsString> =
        helper_args(RUNNER).into_iter().map(Into::into).collect();
    // `MIMIK_LIVE_RUNNER`: Mimik's own binary (built with the feature under test) as the
    // runner, as shipped: single-threaded, its guards active, instead of the test binary.
    let runner_program = std::env::var_os("MIMIK_LIVE_RUNNER").map(PathBuf::from);
    let runner = match runner_program {
        Some(program) => RunnerCommand {
            program,
            args: vec!["assistant-runner".into(), "--spec-fd".into(), "3".into()],
            env: Vec::new(),
        },
        None => RunnerCommand {
            program: exe.clone(),
            args: runner_args,
            env: vec![(test_helper::VARIABLE.into(), "runner".into())],
        },
    };
    Settings {
        data_dir: scratch.join("data"),
        models_dir: env("MIMIK_LIVE_MODELS"),
        runtime_dir: env("MIMIK_LIVE_RUNTIME"),
        model: MANIFEST.test_model.name.clone(),
        model_sha256: MANIFEST.test_model.sha256.clone(),
        alias: "stories".into(),
        threads: 2,
        ctx_size: MANIFEST.test_model.ctx_size,
        runner,
        inner: InnerCommand {
            executable: exe,
            probe_args: args,
            probe_env: env_vars,
        },
        device: device::Choice::Cpu,
        roots: device::Roots::default(),
    }
}

fn scratch() -> tempfile::TempDir {
    tempfile::Builder::new()
        .tempdir_in(env("MIMIK_LIVE_SCRATCH"))
        .unwrap()
}

/// The live launcher, level 3 allowed (the live level-3 tests keep its switched-off code
/// working; Mimik's own launcher never allows it, ASSIST-43).
fn real(settings: Settings) -> Arc<Real> {
    Arc::new(Real::new(settings).allowing_unconfined())
}

#[test]
fn helper() {
    let Some(mode) = test_helper::mode() else {
        return;
    };
    match mode.as_str() {
        "probe-inner" => {
            let args: Vec<String> = std::env::args().collect();
            let at = args.iter().position(|arg| arg == "probe-inner").unwrap();
            std::process::exit(super::super::probe::inner(&args[at + 1..]));
        }
        // A probe that reports and exits at once (before its reaper can be pinned).
        "probe-negative-fast" => {
            println!(r#"{{"ok":false,"render":"the render node cannot be opened: test","pid":1}}"#);
            std::process::exit(0);
        }
        "probe-ok-fast" => {
            println!(r#"{{"ok":true,"pid":1}}"#);
            std::process::exit(0);
        }
        "adversary" => adversary(),
        "landlock-scope" => landlock_scope(),
        "mimik" => play_mimik(),
        "cwd-probe" => play_cwd_probe(),
        other => panic!("unknown helper mode {other}"),
    }
}

fn errno_of<T>(result: std::io::Result<T>) -> Value {
    match result {
        Ok(_) => json!(0),
        Err(error) => json!(error.raw_os_error().unwrap_or(-1)),
    }
}

/// What the adversary finds out about its own process: capabilities, `no_new_privs`,
/// seccomp, terminals, `/dev` and standard input.
fn process_facts() -> Value {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |name: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .map_or_else(String::new, |value| value.trim().to_owned())
    };
    // `TIOCSTI` pushes a byte into a terminal's input queue; on each standard descriptor it
    // must fail (they are pipes or `/dev/null`, never a terminal).
    let tiocsti: Vec<i64> = (0..3)
        .map(|fd| {
            let byte = b'x';
            // SAFETY: an ioctl on a standard descriptor with a pointer to a live byte; the
            // request is refused (ENOTTY) unless the descriptor were a terminal.
            let result = unsafe { libc::ioctl(fd, libc::TIOCSTI, &byte) };
            i64::from(result == 0)
        })
        .collect();
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    // Field 7 of /proc/self/stat is the controlling terminal (after the parenthesised name).
    let tty_nr = stat
        .rsplit(')')
        .next()
        .and_then(|rest| rest.split_whitespace().nth(4))
        .map_or_else(String::new, str::to_owned);
    let dev: Vec<String> = std::fs::read_dir("/dev")
        .map(|listing| {
            listing
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    // Every mount with its options (field 5 of /proc/self/mountinfo is the mount point and
    // field 6 its per-mount options).
    let mounts: Vec<Value> = std::fs::read_to_string("/proc/self/mountinfo")
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let point = fields.nth(4)?;
            let options = fields.next()?;
            Some(json!([point, options]))
        })
        .collect();
    json!({
        "mounts": mounts,
        "caps": {"inh": field("CapInh:"), "prm": field("CapPrm:"), "eff": field("CapEff:"),
            "bnd": field("CapBnd:"), "amb": field("CapAmb:")},
        "no_new_privs": field("NoNewPrivs:"),
        "umask": field("Umask:"),
        "seccomp": field("Seccomp:"),
        "dev": dev,
        "dev_tty": errno_of(std::fs::File::open("/dev/tty")),
        "stdin": std::fs::read_link("/proc/self/fd/0")
            .map_or_else(|error| error.to_string(), |path| path.display().to_string()),
        "tiocsti_succeeded": tiocsti,
        "controlling_tty": tty_nr,
    })
}

/// Inside the level-1 sandbox in place of llama-server: try what a compromised server
/// would, report each outcome, then answer the endpoint check like the probe.
fn adversary() {
    use std::io::Write;
    let port: u16 = std::env::var("MIMIK_ADV_PORT").unwrap().parse().unwrap();
    let lan = std::env::var("MIMIK_ADV_LAN").unwrap_or_default();
    let abstract_name = std::env::var("MIMIK_ADV_ABSTRACT").unwrap();
    let host_socket = std::env::var("MIMIK_ADV_SOCKET").unwrap();
    let connect = |address: &str| {
        errno_of(std::net::TcpStream::connect_timeout(
            &address.parse().unwrap(),
            std::time::Duration::from_secs(2),
        ))
    };
    let udp = errno_of(std::net::UdpSocket::bind("0.0.0.0:0"));
    let abstract_connect = {
        use std::os::linux::net::SocketAddrExt;
        let address =
            std::os::unix::net::SocketAddr::from_abstract_name(abstract_name.as_bytes()).unwrap();
        errno_of(std::os::unix::net::UnixStream::connect_addr(&address))
    };
    let root: Vec<String> = std::fs::read_dir("/")
        .map(|listing| {
            listing
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    // SAFETY: a raw fork in a single-purpose helper; a child would exit at once.
    let fork = unsafe { libc::syscall(libc::SYS_fork) };
    if fork == 0 {
        // SAFETY: the child of a fork that should have been refused.
        unsafe { libc::_exit(0) };
    }
    let fork = test_helper::errno(fork);
    let fill = |dir: &str| {
        let mut total = 0_u64;
        let chunk = vec![7_u8; 1 << 20];
        for n in 0..1024 {
            let Ok(mut file) = std::fs::File::create(format!("{dir}/fill-{n}")) else {
                break;
            };
            if file.write_all(&chunk).is_err() {
                break;
            }
            total += chunk.len() as u64;
        }
        for n in 0..1024 {
            let _ = std::fs::remove_file(format!("{dir}/fill-{n}"));
        }
        total
    };
    // What the command inherits: the closure closed every descriptor above 2 before its exec.
    let descriptors: Vec<i64> = std::fs::read_dir("/proc/self/fd")
        .map(|listing| {
            listing
                .filter_map(Result::ok)
                .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
                .collect()
        })
        .unwrap_or_default();
    // What the namespaces' mounts allow and the Landlock ruleset does not: a write in the
    // writable `/dev` tmpfs and in `/proc`, listings of `/etc`, `/sys` and `/`, and a TCP
    // bind (the empty network namespace alone fails it differently). The controls are
    // what the ruleset allows.
    let landlock = landlock_facts();
    let report = json!({
        "ok": true,
        "descriptors": descriptors,
        "tcp_host_listener": connect(&format!("127.0.0.1:{port}")),
        "tcp_lan": if lan.is_empty() { json!(null) } else { connect(&format!("{lan}:{port}")) },
        "tcp_public": connect("1.1.1.1:443"),
        "udp": udp,
        "abstract_socket": abstract_connect,
        "host_socket": errno_of(std::os::unix::net::UnixStream::connect(&host_socket)),
        "home_visible": std::path::Path::new("/home").exists(),
        "root": root,
        "fork": fork,
        "tmp_filled": fill("/tmp"),
        "shm_filled": fill("/dev/shm"),
    });
    let mut report = report;
    report
        .as_object_mut()
        .unwrap()
        .extend(process_facts().as_object().unwrap().clone());
    report
        .as_object_mut()
        .unwrap()
        .extend(landlock.as_object().unwrap().clone());
    println!("{report}");
    super::super::probe::inner(&["1".into()]);
}

fn landlock_facts() -> Value {
    let abi = landlock_abi();
    let write_open = |path: &str| {
        errno_of(
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(path),
        )
    };
    json!({
        "landlock_abi": abi,
        "dev_create": write_open("/dev/mimik-landlock-probe"),
        "proc_write": errno_of(std::fs::OpenOptions::new().write(true).open("/proc/self/oom_score_adj")),
        "etc_list": errno_of(std::fs::read_dir("/etc")),
        "sys_list": errno_of(std::fs::read_dir("/sys")),
        "root_list": errno_of(std::fs::read_dir("/")),
        "tcp_bind": errno_of(std::net::TcpListener::bind("127.0.0.1:0")),
        "control_tmp_write": write_open("/tmp/mimik-landlock-probe"),
        "control_shm_write": write_open("/dev/shm/mimik-landlock-probe"),
        "control_null_write": write_open("/dev/null"),
        "control_cpu_read": errno_of(std::fs::read_to_string("/sys/devices/system/cpu/online")),
        "control_proc_read": errno_of(std::fs::read_to_string("/proc/self/status")),
        "control_dev_list": errno_of(std::fs::read_dir("/dev")),
    })
}

/// Independent capability query: do not decide which assertions to run from the report
/// being tested. VERSION creates no ruleset and does not alter this process.
fn landlock_abi() -> i64 {
    // SAFETY: VERSION takes a null pointer and zero length, dereferencing no memory.
    unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<u8>(),
            0_usize,
            1_u32,
        )
    }
}

/// Plays Mimik for the SIGKILL check: starts llama-server at level 1, reports the PIDs, waits.
fn play_mimik() {
    let scratch = env("MIMIK_LIVE_CHILD_SCRATCH");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let launch = Launch(real(settings(&scratch, "probe-inner", Vec::new())));
        launch.preflight(Path::Cpu).await.unwrap();
        let probing = Arc::clone(&launch.0);
        let probed = tokio::task::spawn_blocking(move || {
            probing.probe_level(Path::Cpu, Level::Sandbox, &probing.work().enter())
        })
        .await
        .unwrap();
        assert_eq!(probed, Probe::Passed);
        let child = launch.start(1, Level::Sandbox, Path::Cpu).await.unwrap();
        // "Ready" waits for /health; "starting" reports at once, while the server loads.
        if std::env::var("MIMIK_LIVE_KILL_AT").as_deref() != Ok("starting") {
            while child.server().health().await != Health::Ready {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        let pidfile =
            std::fs::read_to_string(scratch.join("data/run").join(rundir::PIDFILE)).unwrap();
        let runner: i32 = pidfile.lines().nth(1).unwrap().parse().unwrap();
        test_helper::print(&json!({"runner": runner}));
        tokio::time::sleep(Duration::from_secs(120)).await;
    });
}

/// A level-3 server, started from whatever cwd this process was given, held at Ready; prints
/// its server PID so the parent can inspect its cwd and whether a planted backend loaded.
fn play_cwd_probe() {
    let scratch = env("MIMIK_LIVE_CHILD_SCRATCH");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let launch = Launch(real(settings(&scratch, "probe-inner", Vec::new())));
        launch.preflight(Path::Cpu).await.unwrap();
        let child = launch.start(1, Level::Unconfined, Path::Cpu).await.unwrap();
        while child.server().health().await != Health::Ready {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let pidfile =
            std::fs::read_to_string(scratch.join("data/run").join(rundir::PIDFILE)).unwrap();
        let server: i32 = pidfile.lines().nth(1).unwrap().parse().unwrap();
        test_helper::print(&json!({"server": server}));
        tokio::time::sleep(Duration::from_secs(120)).await;
    });
}

fn descendants(pid: i32) -> Vec<i32> {
    let mut all = Vec::new();
    let mut frontier = vec![pid];
    while let Some(parent) = frontier.pop() {
        for entry in std::fs::read_dir("/proc").unwrap().flatten() {
            let Ok(child) = entry.file_name().to_string_lossy().parse::<i32>() else {
                continue;
            };
            let Ok(status) = std::fs::read_to_string(format!("/proc/{child}/status")) else {
                continue;
            };
            let parent_of = status
                .lines()
                .find_map(|line| line.strip_prefix("PPid:"))
                .and_then(|value| value.trim().parse::<i32>().ok());
            if parent_of == Some(parent) {
                all.push(child);
                frontier.push(child);
            }
        }
    }
    all
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
#[ignore = "live: pinned llama-server, sandbox, test model (MIMIK_LIVE_*)"]
fn live_the_probe_reaches_level_1_and_level_3() {
    let scratch = scratch();
    let real = real(settings(scratch.path(), "probe-inner", Vec::new()));
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime
        .block_on(Launch(Arc::clone(&real)).preflight(Path::Cpu))
        .unwrap();
    assert_eq!(
        real.probe_level(Path::Cpu, Level::Sandbox, &real.work().enter()),
        Probe::Passed
    );
    assert_eq!(
        real.probe_level(Path::Cpu, Level::Unconfined, &real.work().enter()),
        Probe::Passed
    );
}

/// Probes level 1 with the helper `mode` as the probe, held at the reaper step until the
/// probe has exited, so the sandbox is gone before its reaper is pinned.
fn probe_after_exit(mode: &str) -> Probe {
    use super::super::launch::{THREAD_PAUSE, ThreadPause};
    let scratch = scratch();
    let real = real(settings(scratch.path(), mode, Vec::new()));
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime
        .block_on(Launch(Arc::clone(&real)).preflight(Path::Cpu))
        .unwrap();
    let (arrived, reached) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let worker = {
        let real = Arc::clone(&real);
        std::thread::spawn(move || {
            THREAD_PAUSE.lock().unwrap().push(ThreadPause {
                thread: std::thread::current().id(),
                step: "before-reaper",
                arrived,
                release: wait,
            });
            real.probe_level(Path::Cpu, Level::Sandbox, &real.work().enter())
        })
    };
    reached.recv_timeout(Duration::from_secs(60)).unwrap();
    // The probe prints and exits at once; give it ample time to be gone.
    std::thread::sleep(Duration::from_millis(1500));
    release.send(()).unwrap();
    worker.join().unwrap()
}

#[test]
#[ignore = "live: sandbox, pinned runtime (MIMIK_LIVE_*)"]
fn live_a_failing_probe_that_ended_before_its_reaper_was_pinned_still_reports_its_verdict() {
    match probe_after_exit("probe-negative-fast") {
        Probe::Failed(why) => assert!(why.contains("the render node cannot be opened"), "{why}"),
        other => panic!("the negative report did not decide: {other:?}"),
    }
}

#[test]
#[ignore = "live: sandbox, pinned runtime (MIMIK_LIVE_*)"]
fn live_an_ok_probe_without_a_verified_reaper_can_never_pass() {
    match probe_after_exit("probe-ok-fast") {
        Probe::Inconclusive(why) => {
            assert!(why.contains("sandbox ended before its check"), "{why}");
        }
        other => panic!("an ok report without a reaper must stay inconclusive: {other:?}"),
    }
}

async fn raw(child: &RealChild, path: &str, token: Option<&str>, body: &Value) -> (u16, Value) {
    let transport = Bounded::new(child.connector());
    let mut request = http::Request::post(format!("{}{path}", transport.base()))
        .header(http::header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        request = request.header(http::header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let request = request
        .body(bytes::Bytes::from(serde_json::to_vec(body).unwrap()))
        .unwrap();
    match transport.send::<_, bytes::Bytes>(request).await {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response.into_body().await.unwrap();
            (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
        }
        Err(rig_core::http_client::Error::InvalidStatusCodeWithDetails { status, .. }) => {
            (status.as_u16(), Value::Null)
        }
        Err(error) => panic!("{error}"),
    }
}

fn short_request(max_tokens: u32) -> Value {
    json!({"messages": [{"role": "user", "content": "Once upon a time"}],
        "max_tokens": max_tokens, "ignore_eos": true, "temperature": 0.0, "top_k": 1,
        "seed": 0, "stream": false})
}

/// The real server at `level`, ready.
async fn started(scratch: &std::path::Path, level: Level) -> (Launch, Arc<RealChild>) {
    started_with(scratch, level, |_| {}).await
}

/// [`started`] with `tweak` applied to the settings first.
async fn started_with(
    scratch: &std::path::Path,
    level: Level,
    tweak: impl FnOnce(&mut Settings),
) -> (Launch, Arc<RealChild>) {
    let mut settings = settings(scratch, "probe-inner", Vec::new());
    tweak(&mut settings);
    let launch = Launch(real(settings));
    launch.preflight(Path::Cpu).await.unwrap();
    let probing = Arc::clone(&launch.0);
    let probed = tokio::task::spawn_blocking(move || {
        probing.probe_level(Path::Cpu, level, &probing.work().enter())
    })
    .await
    .unwrap();
    assert_eq!(probed, Probe::Passed);
    let child = launch.start_real(1, level, Path::Cpu).await.unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        match child.server().health().await {
            Health::Ready => break,
            Health::Refused(why) => panic!("{why}"),
            _ => {}
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no /health; the server said {:?}",
            child.capture.lock().unwrap()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    child.answered_health();
    child.loaded().await.unwrap();
    (launch, child)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: pinned llama-server, sandbox, test model (MIMIK_LIVE_*)"]
async fn live_level_1_serves_over_the_checked_socket_with_the_token_only() {
    let scratch = scratch();
    let (_launch, child) = started(scratch.path(), Level::Sandbox).await;
    let real = &*child;
    // /health answered without a token (above); every other route needs it.
    let (status, _) = raw(real, "/v1/chat/completions", None, &short_request(8)).await;
    assert_eq!(status, 401);
    let (status, _) = raw(
        real,
        "/v1/chat/completions",
        Some("wrong-token"),
        &short_request(8),
    )
    .await;
    assert_eq!(status, 401);
    let server = child.server();
    // The right token: the per-request ignore_eos generates exactly max_tokens (the test
    // model's context holds no fixed prefix, so the request is a short one of its own).
    let marker = "zebra-marker-7f3a";
    let mut request = short_request(16);
    request["messages"][0]["content"] = json!(format!("Once upon a {marker}"));
    let (status, answer) = raw(real, "/v1/chat/completions", Some(child.token()), &request).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["timings"]["predicted_n"], json!(16));
    assert!(server.tokens("hello there").await.unwrap() > 0);
    // No token file is left in the generation's directory (the sandbox holds the token by a
    // descriptor or, with Hakoniwa, by the inode of a bind mount whose name was removed).
    assert!(
        !scratch
            .path()
            .join("data/run/1")
            .join(super::super::token::FILE)
            .exists()
    );
    // The token is in no file under the data and scratch directories (the sandbox root's mount
    // points, the generation's directory, the pidfile): with Hakoniwa it is in the sandbox's
    // own memory-backed /run only.
    assert_eq!(files_holding(scratch.path(), child.token().as_bytes()), 0);
    // The token is in no command line of the sandbox's processes.
    let pidfile =
        std::fs::read_to_string(scratch.path().join("data/run").join(rundir::PIDFILE)).unwrap();
    let runner: i32 = pidfile.lines().nth(1).unwrap().parse().unwrap();
    for pid in std::iter::once(runner).chain(descendants(runner)) {
        let line = String::from_utf8_lossy(
            &std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default(),
        )
        .replace('\0', " ");
        assert!(!line.contains("--api-key "), "{line}");
    }
    // A socket swapped into the directory by a process outside the sandbox gets nothing.
    let socket = scratch.path().join("data/run/1").join(endpoint::SOCKET);
    std::fs::remove_file(&socket).unwrap();
    let _impostor = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let health = child.server().health().await;
    assert!(matches!(health, Health::Refused(_)), "{health:?}");
    child.stop().await;
    assert!(!scratch.path().join("data/run/1").exists());
    // Nor does a sandbox root (Hakoniwa's mount points) outlive it.
    assert!(!scratch.path().join("data/run/1.root").exists());
    // The marker reached neither stream.
    assert!(child.readers.all_ended(READERS_WITHIN).await);
    let capture = child.capture.lock().unwrap();
    assert!(!capture.overflow, "output capture overflowed");
    assert!(capture.bytes.iter().any(|bytes| !bytes.is_empty()));
    assert!(!capture.bytes.iter().any(|stream| {
        stream
            .windows(marker.len())
            .any(|bytes| bytes == marker.as_bytes())
    }));
    for pid in descendants(runner) {
        assert!(!alive(pid), "{pid} outlived the stop");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: pinned llama-server, sandbox, test model (MIMIK_LIVE_*)"]
async fn live_level_3_loads_the_model_through_proc_self_fd() {
    let scratch = scratch();
    let (_launch, child) = started(scratch.path(), Level::Unconfined).await;
    let real = &*child;
    let line =
        String::from_utf8_lossy(&std::fs::read(format!("/proc/{}/cmdline", real.pid())).unwrap())
            .replace('\0', " ");
    assert!(line.contains("--model /proc/self/fd/8"), "{line}");
    assert!(!line.contains("--api-key "));
    assert!(!scratch.path().join("data/run/1").join(token::FILE).exists());
    let (status, _) = raw(real, "/v1/chat/completions", None, &short_request(8)).await;
    assert_eq!(status, 401);
    let (status, answer) = raw(
        real,
        "/v1/chat/completions",
        Some(child.token()),
        &short_request(8),
    )
    .await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["timings"]["predicted_n"], json!(8));
    child.stop().await;
}

/// Every mount is read-only (the root and each bind of host files) but the few the server
/// writes to: its socket directory, the temporary space (Hakoniwa's 64 KiB `/run` for the token included) and the device nodes.
fn assert_only_the_writable_mounts_are_writable(report: &Value) {
    for mount in report["mounts"].as_array().unwrap() {
        let (point, options) = (mount[0].as_str().unwrap(), mount[1].as_str().unwrap());
        let writable = ["/run/llama", "/run", "/tmp", "/dev/shm", "/proc"].contains(&point)
            || point == "/dev"
            || point.starts_with("/dev/");
        let read_only = options.split(',').any(|option| option == "ro");
        assert!(
            writable || read_only,
            "{point} is mounted writable ({options}): {report}"
        );
        // No mount may carry set-user-ID files, and only device mounts may carry devices.
        let has = |flag: &str| options.split(',').any(|option| option == flag);
        assert!(has("nosuid") || point == "/", "{point}: {options}");
        if !point.starts_with("/dev") && point != "/" {
            assert!(has("nodev"), "{point}: {options}");
        }
    }
}

/// No capability in any set, `no_new_privs` and seccomp in force, no terminal, and with
/// Hakoniwa a `/dev` of the five safe nodes.
fn assert_no_privilege_or_terminal(report: &Value) {
    let refused = |value: &Value| value.as_i64().is_some_and(|errno| errno != 0);
    // No capability left in any set (the bounding set too: the closure empties it), `no_new_privs` and the seccomp filter in force.
    let zero = "0000000000000000";
    for set in ["inh", "prm", "eff", "bnd", "amb"] {
        assert_eq!(report["caps"][set], json!(zero), "{set}: {report}");
    }
    assert_eq!(report["no_new_privs"], json!("1"), "{report}");
    assert_eq!(report["seccomp"], json!("2"), "{report}");
    // No terminal: /dev/tty cannot be opened, no standard descriptor takes TIOCSTI, and the
    // process has no controlling terminal.
    assert!(refused(&report["dev_tty"]), "{report}");
    assert_eq!(report["tiocsti_succeeded"], json!([0, 0, 0]), "{report}");
    assert_eq!(report["controlling_tty"], json!("0"), "{report}");
    // Standard input is `/dev/null`: with Hakoniwa it was the marker pipe until the exec.
    assert_eq!(report["stdin"], json!("/dev/null"), "{report}");
    // The runner's own umask (027, set around the run) is back before the exec (it ran with
    // 077 while it built the sandbox).
    assert_eq!(report["umask"], json!("0027"), "{report}");
    {
        // The sandbox's /dev is built here: the five safe nodes and the links, no terminal node,
        // no pts, no ptmx.
        let dev: Vec<&str> = report["dev"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        for entry in &dev {
            assert!(
                [
                    "null", "zero", "full", "random", "urandom", "fd", "stdin", "stdout", "stderr",
                    "shm"
                ]
                .contains(entry),
                "{entry} is in /dev: {dev:?}"
            );
        }
    }
}

/// Whether the run demands ABI 6 (`MIMIK_LIVE_LANDLOCK=require`: CI on a capable runner, where
/// skipping the confinement assertions would hide a regression). Without it the assertions
/// that need ABI 6 skip, with their reason, on a kernel below it (DEP-25).
fn landlock_demanded() -> bool {
    std::env::var("MIMIK_LIVE_LANDLOCK").as_deref() == Ok("require")
}

/// Whether the confinement assertions apply: on a kernel of ABI 6 or more. A
/// demanded run on an older kernel fails; otherwise it skips and says why.
fn landlock_required(abi: i64) -> bool {
    assert!(
        abi >= 6 || !landlock_demanded(),
        "this run requires Landlock ABI >=6, found {abi}"
    );
    if abi < 6 {
        eprintln!("SKIPPED: the Landlock confinement assertions need kernel ABI 6, found {abi}");
    }
    abi >= 6
}

/// Under Hakoniwa with Landlock in force the ruleset refuses, with `EACCES`, what the mounts
/// alone would allow, and still allows what the server needs (the controls). Containment
/// checks only, like the others.
fn assert_landlock_confines(report: &Value, lan: bool) {
    if !landlock_required(report["landlock_abi"].as_i64().unwrap()) {
        return;
    }
    let denied = json!(libc::EACCES);
    for fact in [
        // A writable mount (the sandbox's `/dev` tmpfs, `/proc`) that is not in the ruleset.
        "dev_create",
        "proc_write",
        // Directories the mounts show but no rule lists.
        "etc_list",
        "sys_list",
        "root_list",
        // TCP: the empty network namespace is not what refuses these, Landlock is.
        "tcp_host_listener",
        "tcp_public",
        "tcp_bind",
    ] {
        assert_eq!(report[fact], denied, "{fact}: {report}");
    }
    if lan {
        assert_eq!(report["tcp_lan"], denied, "{report}");
    }
    // The server's own needs stay open.
    for fact in [
        "control_tmp_write",
        "control_shm_write",
        "control_null_write",
        "control_cpu_read",
        "control_proc_read",
        "control_dev_list",
    ] {
        assert_eq!(report[fact], json!(0), "{fact}: {report}");
    }
}

#[test]
#[ignore = "live: pinned llama-server, sandbox, test model (MIMIK_LIVE_*)"]
fn live_an_adversary_in_the_level_1_sandbox_reaches_nothing() {
    let scratch = scratch();
    let listener = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let lan = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .find(|iface| !iface.is_loopback() && iface.ip().is_ipv4())
        .map(|iface| iface.ip().to_string())
        .unwrap_or_default();
    let abstract_name = format!("mimik-live-{}", std::process::id());
    let abstract_listener = {
        use std::os::linux::net::SocketAddrExt;
        std::os::unix::net::UnixListener::bind_addr(
            &std::os::unix::net::SocketAddr::from_abstract_name(abstract_name.as_bytes()).unwrap(),
        )
        .unwrap()
    };
    abstract_listener.set_nonblocking(true).unwrap();
    let host_socket = scratch.path().join("host.sock");
    let host_listener = std::os::unix::net::UnixListener::bind(&host_socket).unwrap();
    host_listener.set_nonblocking(true).unwrap();
    let real = real(settings(
        scratch.path(),
        "adversary",
        vec![
            ("MIMIK_ADV_PORT".into(), port.to_string()),
            ("MIMIK_ADV_LAN".into(), lan.clone()),
            ("MIMIK_ADV_ABSTRACT".into(), abstract_name),
            ("MIMIK_ADV_SOCKET".into(), host_socket.display().to_string()),
        ],
    ));
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(Launch(Arc::clone(&real)).preflight(Path::Cpu))
        .unwrap();
    // The runner inherits this process's umask: an explicit one, restored at once.
    let before = rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o027));
    let reported = real.inner_report(Path::Cpu, Level::Sandbox, &real.work().enter());
    rustix::process::umask(before);
    let (report, verdict) = reported.unwrap();
    assert_eq!(verdict, Probe::Passed, "{report}");
    let refused = |value: &Value| value.as_i64().is_some_and(|errno| errno != 0);
    assert!(refused(&report["tcp_host_listener"]), "{report}");
    if !lan.is_empty() {
        assert!(refused(&report["tcp_lan"]), "{report}");
    }
    assert!(refused(&report["tcp_public"]), "{report}");
    assert_eq!(report["udp"], json!(libc::EPERM), "{report}");
    assert!(refused(&report["abstract_socket"]), "{report}");
    assert!(refused(&report["host_socket"]), "{report}");
    assert_eq!(report["home_visible"], json!(false));
    assert_eq!(report["fork"], json!(libc::EPERM));
    // Only 0, 1, 2 and the listing's own descriptor.
    let descriptors: Vec<i64> = report["descriptors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|fd| fd.as_i64().unwrap())
        .collect();
    assert!(
        descriptors.iter().filter(|fd| **fd > 2).count() <= 1,
        "{descriptors:?}"
    );
    for inherited in [4, 5, 6, 7, 8, 10, 12, 16] {
        assert!(
            !descriptors.contains(&inherited),
            "{inherited} is open: {descriptors:?}"
        );
    }
    // Sized temporary space.
    {
        assert!(
            report["tmp_filled"].as_u64().unwrap() <= sandbox::TMP_BYTES,
            "{report}"
        );
        assert!(
            report["shm_filled"].as_u64().unwrap() <= sandbox::SHM_BYTES,
            "{report}"
        );
    }
    let root: Vec<&str> = report["root"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for entry in &root {
        assert!(
            [
                "usr", "lib", "lib64", "etc", "sys", "dev", "proc", "tmp", "run", "opt", "models",
                "bin", "sbin"
            ]
            .contains(entry),
            "{entry} is visible: {root:?}"
        );
    }
    assert_no_privilege_or_terminal(&report);
    assert_only_the_writable_mounts_are_writable(&report);
    assert_landlock_confines(&report, !lan.is_empty());
    // Nothing reached the host's listeners.
    assert!(listener.accept().is_err());
    assert!(abstract_listener.accept().is_err());
    assert!(host_listener.accept().is_err());
}

fn sigkill_of_mimik_ends_the_sandbox(kill_at: &str) {
    let scratch = scratch();
    let mut mimik = test_helper::command(HELPER, "mimik")
        .env("MIMIK_LIVE_CHILD_SCRATCH", scratch.path())
        .env("MIMIK_LIVE_KILL_AT", kill_at)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = std::io::BufReader::new(mimik.stdout.take().unwrap());
    let runner = loop {
        let mut line = String::new();
        assert!(
            std::io::BufRead::read_line(&mut reader, &mut line).unwrap() > 0,
            "mimik stopped"
        );
        if let Some(at) = line.find("HELPER ") {
            let value: Value = serde_json::from_str(&line[at + 7..]).unwrap();
            break i32::try_from(value["runner"].as_i64().unwrap()).unwrap();
        }
    };
    let tree: Vec<i32> = std::iter::once(runner).chain(descendants(runner)).collect();
    assert_ne!(tree, Vec::<i32>::new());
    mimik.kill().unwrap();
    mimik.wait().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while tree.iter().any(|pid| alive(*pid)) {
        assert!(
            std::time::Instant::now() < deadline,
            "the sandbox outlived Mimik: {tree:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "live: pinned llama-server, sandbox, test model (MIMIK_LIVE_*)"]
fn live_sigkill_of_mimik_kills_the_whole_sandbox() {
    sigkill_of_mimik_ends_the_sandbox("ready");
}

#[test]
#[ignore = "live: pinned llama-server, sandbox, test model (MIMIK_LIVE_*)"]
fn live_sigkill_of_mimik_during_startup_kills_the_whole_sandbox() {
    sigkill_of_mimik_ends_the_sandbox("starting");
}

/// A fake build in `scratch`: the fake llama-server (`MIMIK_LIVE_FAKE_SERVER`) as the only
/// executable, a fake model, the test binary as runner.
fn fake_build(scratch: &std::path::Path, alias: &str) -> Arc<Real> {
    let server = std::fs::read(env("MIMIK_LIVE_FAKE_SERVER")).unwrap();
    fake_build_with(scratch, alias, &server)
}

/// [`fake_build`] with `server` as the runtime's one executable.
fn fake_build_with(scratch: &std::path::Path, alias: &str, server: &[u8]) -> Arc<Real> {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;
    let cpu = scratch.join("runtime/cpu");
    std::fs::create_dir_all(&cpu).unwrap();
    std::fs::write(cpu.join("llama-server"), server).unwrap();
    std::fs::set_permissions(
        cpu.join("llama-server"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let models = scratch.join("models");
    std::fs::create_dir(&models).unwrap();
    std::fs::set_permissions(&models, std::fs::Permissions::from_mode(0o700)).unwrap();
    let model = b"GGUF a fake model";
    std::fs::write(models.join("m.gguf"), model).unwrap();
    let mut settings = settings(scratch, "probe-inner", Vec::new());
    settings.runtime_dir = scratch.join("runtime");
    settings.models_dir = models;
    settings.model = "m.gguf".into();
    settings.model_sha256 = hex::encode(Sha256::digest(model));
    settings.alias = alias.into();
    let build = super::super::manifest::Build {
        path: "cpu".into(),
        directory: "cpu".into(),
        server: "llama-server".into(),
        files: vec![super::super::manifest::File {
            name: "llama-server".into(),
            sha256: hex::encode(Sha256::digest(server)),
            size: server.len() as u64,
            mode: "0755".into(),
        }],
    };
    Arc::new(Real::with_build(settings, Some(build)))
}

/// How many regular files under `dir` (not followed through links) contain `needle`.
fn files_holding(dir: &std::path::Path, needle: &[u8]) -> usize {
    let mut found = 0;
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            found += files_holding(&entry.path(), needle);
        } else if kind.is_file()
            && std::fs::read(entry.path())
                .is_ok_and(|bytes| bytes.windows(needle.len()).any(|window| window == needle))
        {
            found += 1;
        }
    }
    found
}

/// A server (the fake one) that prints sandbox-failure lines and exits with the code the
/// alias names: the failure is counted as the model server's, the level kept, and nothing
/// starts unconfined.
async fn forged_failure_keeps_level_1(alias: &str) {
    let scratch = scratch();
    let real = fake_build(scratch.path(), alias);
    failed_start_keeps_level_1(real, &["the model server exited"]).await;
}

/// The manager over `real` until its first failed start: counted as the model server's, the
/// level kept.
async fn failed_start_keeps_level_1(real: Arc<Real>, reasons: &[&str]) {
    use super::super::manager::{self, State};
    use crate::assistant::jobs::Assistant;
    let assistant = Assistant::new();
    let handle = manager::spawn(
        assistant,
        Arc::new(Launch(Arc::clone(&real))),
        manager::Config {
            path: Path::Cpu,
            levels: manager::USED_LEVELS,
        },
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    while handle.status().failures == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "{:?}",
            handle.status()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let status = handle.status();
    // The forged lines did not count as a sandbox failure: the level is kept, the failure
    // counted, no unconfined start.
    assert_eq!(status.level, Some(Level::Sandbox), "{status:?}");
    assert!(
        !status
            .skipped
            .iter()
            .any(|(level, _)| *level == Level::Sandbox),
        "{status:?}"
    );
    assert!(matches!(&status.state, State::Starting(_)), "{status:?}");
    assert!(
        status
            .last_failure
            .as_deref()
            .is_some_and(|why| reasons.iter().any(|reason| why.starts_with(reason))),
        "{status:?}"
    );
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: pinned llama-server, sandbox, test model (MIMIK_LIVE_*)"]
async fn live_a_server_that_forges_sandbox_errors_keeps_level_1() {
    forged_failure_keeps_level_1("forge-sandbox").await;
}

/// A server file that cannot be executed (not an executable format) fails at the exec itself,
/// after the marker: the server's failure, counted, never a sandbox failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: pinned llama-server, sandbox, test model (MIMIK_LIVE_*)"]
async fn live_a_server_that_cannot_be_executed_is_the_servers_failure() {
    let scratch = scratch();
    let real = fake_build_with(scratch.path(), "no-exec", b"this is not a program\n");
    // It ends within moments of the start: its sandbox may be gone before it can be pinned
    // (then it says so), else the exit is the server's.
    failed_start_keeps_level_1(
        real,
        &[
            "the model server exited",
            "the sandbox's reaper: No such process",
        ],
    )
    .await;
}

/// What a start of the Hakoniwa sandbox does when the object at a bound path is no longer the
/// one that was verified (the path binds' (device, inode) check).
async fn swapped_before_the_spawn(swap: impl FnOnce(&std::path::Path)) -> Fail {
    let scratch = scratch();
    let real = fake_build(scratch.path(), "swapped");
    let settings = real.settings.clone();
    let build = real.cpu.clone();
    let real = Arc::new(Real::with_build(settings, build));
    let launch = Launch(Arc::clone(&real));
    launch.preflight(Path::Cpu).await.unwrap();
    let probing = Arc::clone(&real);
    let probed = tokio::task::spawn_blocking(move || {
        probing.probe_level(Path::Cpu, Level::Sandbox, &probing.work().enter())
    })
    .await
    .unwrap();
    assert_eq!(probed, Probe::Passed);
    let (arrived, reached) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    PAUSE.lock().unwrap().push(Pause {
        dir: real.settings.runtime_dir.clone(),
        step: "before-spawn",
        arrived,
        release: wait,
    });
    let starting = tokio::spawn({
        let launch = Launch(Arc::clone(&real));
        async move { launch.start_real(1, Level::Sandbox, Path::Cpu).await }
    });
    tokio::task::spawn_blocking(move || reached.recv_timeout(Duration::from_secs(60)))
        .await
        .unwrap()
        .unwrap();
    swap(scratch.path());
    release.send(()).unwrap();
    let child = starting.await.unwrap().expect("the runner starts");
    let failure = child.exited().await;
    // Nothing of the server ran: it would have created its socket.
    assert!(
        !scratch.path().join("data/run/1/llama.sock").exists(),
        "the server ran"
    );
    child.stop().await;
    failure
}

fn replace_file(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let bytes = std::fs::read(path).unwrap();
    let other = path.with_extension("swapped");
    std::fs::write(&other, bytes).unwrap();
    std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(&other, path).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: pinned llama-server, sandbox, test model (MIMIK_LIVE_*)"]
async fn live_a_model_swapped_after_verification_is_never_run_by_hakoniwa() {
    let failure = swapped_before_the_spawn(|scratch| {
        // The same bytes under a new inode: only the identity check can tell.
        replace_file(&scratch.join("models/m.gguf"));
    })
    .await;
    assert!(
        matches!(&failure, Fail::Sandbox(why) if why.starts_with("hakoniwa could not set up")),
        "{failure:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: pinned llama-server, sandbox, test model (MIMIK_LIVE_*)"]
async fn live_a_server_swapped_after_verification_is_never_run_by_hakoniwa() {
    let failure = swapped_before_the_spawn(|scratch| {
        replace_file(&scratch.join("runtime/cpu/llama-server"));
    })
    .await;
    assert!(
        matches!(&failure, Fail::Sandbox(why) if why.starts_with("hakoniwa could not set up")),
        "{failure:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: pinned llama-server, sandbox, test model (MIMIK_LIVE_*)"]
async fn live_a_model_replaced_by_a_link_to_another_file_is_never_run_by_hakoniwa() {
    let failure = swapped_before_the_spawn(|scratch| {
        let model = scratch.join("models/m.gguf");
        let elsewhere = scratch.join("models/other.gguf");
        std::fs::copy(&model, &elsewhere).unwrap();
        std::fs::remove_file(&model).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &model).unwrap();
    })
    .await;
    assert!(
        matches!(&failure, Fail::Sandbox(why) if why.starts_with("hakoniwa could not set up")),
        "{failure:?}"
    );
}

/// Nothing the executed server exits with can count as a sandbox failure, whatever code the
/// runner or its closure use for one (or the signal-like codes above 128).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: pinned llama-server, sandbox, test model (MIMIK_LIVE_*)"]
async fn live_no_exit_code_of_the_server_counts_as_a_sandbox_failure() {
    for code in [110, 111, 112, 113, 114, 115, 116, 120, 125, 126, 127, 137] {
        forged_failure_keeps_level_1(&format!("forge-exit-{code}")).await;
    }
}

#[tokio::test]
#[ignore = "live: pinned llama-server, sandbox, test model (MIMIK_LIVE_*)"]
async fn live_a_failure_after_the_level_1_spawn_leaves_no_process_behind() {
    let scratch = scratch();
    // The alias is in the command lines of the runner, its reaper and the server: a unique one
    // finds this test's processes and no other's.
    let alias = format!(
        "fake-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let real = fake_build(scratch.path(), &alias);
    let launch = Launch(Arc::clone(&real));
    launch.preflight(Path::Cpu).await.unwrap();
    let probing = Arc::clone(&real);
    let probed = tokio::task::spawn_blocking(move || {
        probing.probe_level(Path::Cpu, Level::Sandbox, &probing.work().enter())
    })
    .await
    .unwrap();
    assert_eq!(probed, Probe::Passed);
    let needle = alias;
    for (generation, step) in [
        (1, "spawned"),
        (2, "recorded"),
        (3, "started"),
        (4, "connector"),
    ] {
        *FAULT.lock().unwrap() = Some((real.settings.runtime_dir.clone(), step));
        let result = launch.start(generation, Level::Sandbox, Path::Cpu).await;
        assert!(result.is_err(), "{step}");
        launch.settle().await;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let left: Vec<i32> = std::fs::read_dir("/proc")
                .unwrap()
                .flatten()
                .filter_map(|entry| entry.file_name().to_string_lossy().parse::<i32>().ok())
                .filter(|pid| {
                    alive(*pid)
                        && std::fs::read(format!("/proc/{pid}/cmdline"))
                            .is_ok_and(|line| String::from_utf8_lossy(&line).contains(&needle))
                })
                .collect();
            if left.is_empty() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{step}: {left:?} survived"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// Builds `libggml-cpu-zzevil.so` in `dir`, whose constructor writes `marker`; `None` when no
/// C compiler is available.
fn marker_backend(dir: &std::path::Path, marker: &std::path::Path) -> Option<()> {
    let source = dir.join("evil.c");
    std::fs::write(
        &source,
        format!(
            "#include <stdio.h>\n__attribute__((constructor)) static void run(void) {{\n  FILE *f = fopen(\"{}\", \"w\"); if (f) {{ fputs(\"loaded\", f); fclose(f); }}\n}}\n",
            marker.display()
        ),
    )
    .unwrap();
    let out = dir.join("libggml-cpu-zzevil.so");
    let compiler = ["cc", "gcc"].into_iter().find(|cc| {
        std::process::Command::new(cc)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })?;
    let status = std::process::Command::new(compiler)
        .args(["-shared", "-fPIC", "-o"])
        .arg(&out)
        .arg(&source)
        .status()
        .ok()?;
    status.success().then_some(())
}

#[test]
#[ignore = "live: pinned llama-server, sandbox, test model (MIMIK_LIVE_*)"]
fn live_level_3_does_not_load_a_backend_from_an_attacker_writable_cwd() {
    let scratch = scratch();
    let attacker = scratch.path().join("attacker-cwd");
    std::fs::create_dir(&attacker).unwrap();
    std::fs::set_permissions(
        &attacker,
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o777),
    )
    .unwrap();
    let marker = attacker.join("loaded.marker");
    // The marker backend must build (cc is present in CI): a silent skip would hide the test.
    marker_backend(&attacker, &marker).expect("a C compiler to build the marker backend");

    // Positive control: the pinned server run straight from the attacker directory (no chdir
    // away) loads the planted backend, so its constructor writes the marker.
    let server = env("MIMIK_LIVE_RUNTIME").join("cpu/llama-server");
    let model = env("MIMIK_LIVE_MODELS").join("stories260K.gguf");
    let mut control = std::process::Command::new(&server)
        .current_dir(&attacker)
        .args([
            "--model".as_ref(),
            model.as_os_str(),
            "--host".as_ref(),
            "127.0.0.1".as_ref(),
            "--port".as_ref(),
            "0".as_ref(),
            "--no-webui".as_ref(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !marker.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    control.kill().unwrap();
    control.wait().unwrap();
    assert!(
        marker.exists(),
        "the control did not load the planted backend; the test proves nothing"
    );
    std::fs::remove_file(&marker).unwrap();

    // Mimik, run from the attacker directory in a subprocess (never set_current_dir here),
    // chdirs the server into the verified build folder, so the planted backend never loads.
    let mut mimik = test_helper::command(HELPER, "cwd-probe")
        .current_dir(&attacker)
        .env("MIMIK_LIVE_CHILD_SCRATCH", scratch.path())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = std::io::BufReader::new(mimik.stdout.take().unwrap());
    let pid = loop {
        let mut line = String::new();
        assert!(
            std::io::BufRead::read_line(&mut reader, &mut line).unwrap() > 0,
            "the child stopped"
        );
        if let Some(at) = line.find("HELPER ") {
            let value: Value = serde_json::from_str(&line[at + 7..]).unwrap();
            break i32::try_from(value["server"].as_i64().unwrap()).unwrap();
        }
    };
    let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).unwrap();
    assert_ne!(cwd, attacker, "the server kept Mimik's cwd");
    assert!(cwd.ends_with("cpu"), "{}", cwd.display());
    // Ready means the model (and its backends) have loaded; the planted one did not.
    assert!(!marker.exists(), "a backend from the attacker cwd loaded");
    mimik.kill().unwrap();
    mimik.wait().unwrap();
}

/// A watcher that records every descendant of `root` it sees, by pid and start time (a
/// reused pid is another process), at short intervals until stopped. The run's own processes
/// are found by their ancestry, not by anything in their command lines, so a runner, a
/// reaper and a program before its exec are all seen.
struct Sweep {
    seen: Arc<Mutex<std::collections::BTreeMap<(i32, u64), String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Sweep {
    fn watch(root: i32) -> Self {
        let seen = Arc::new(Mutex::new(std::collections::BTreeMap::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (seen, stop) = (Arc::clone(&seen), Arc::clone(&stop));
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    // The root, and every process already recorded (still the same process):
                    // what is reparented after the root's death is still found by them.
                    let mut roots = vec![root];
                    roots.extend(
                        seen.lock()
                            .unwrap()
                            .keys()
                            .filter(|(pid, start)| rundir::start_time(*pid) == Some(*start))
                            .map(|(pid, _)| *pid),
                    );
                    let found: std::collections::BTreeSet<i32> =
                        roots.into_iter().flat_map(descendants).collect();
                    for pid in found {
                        if let Some(start) = rundir::start_time(pid) {
                            let line = std::fs::read(format!("/proc/{pid}/cmdline"))
                                .map(|bytes| String::from_utf8_lossy(&bytes).replace('\0', " "))
                                .unwrap_or_default();
                            // A process caught between its fork and exec shows no command line
                            // yet: the first one that shows is kept.
                            let mut seen = seen.lock().unwrap();
                            let known: &mut String = seen.entry((pid, start)).or_default();
                            if known.is_empty() {
                                *known = line;
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_micros(300));
                }
            })
        };
        Self {
            seen,
            stop,
            thread: Some(thread),
        }
    }

    /// The processes seen so far with the command line each had when first seen.
    fn snapshot(&self) -> std::collections::BTreeMap<(i32, u64), String> {
        self.seen.lock().unwrap().clone()
    }

    fn finish(mut self) -> std::collections::BTreeMap<(i32, u64), String> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
        self.snapshot()
    }
}

/// The recorded processes still there: same pid with the same start time, zombies included
/// (a zombie is not gone until its `/proc` entry is).
fn survivors(seen: &std::collections::BTreeMap<(i32, u64), String>) -> Vec<i32> {
    seen.keys()
        .filter(|(pid, start)| rundir::start_time(*pid) == Some(*start))
        .map(|(pid, _)| *pid)
        .collect()
}

/// Waits (10 s) for every recorded process to be gone; the survivors if some are not.
fn wait_all_gone(seen: &std::collections::BTreeMap<(i32, u64), String>) -> Vec<i32> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let left = survivors(seen);
        if left.is_empty() || std::time::Instant::now() >= deadline {
            return left;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Mimik (a helper process) is killed at `delay` after the first process of its run (the
/// runner) appeared, at each moment of a start in turn; every process of the run seen from
/// then on (the runner, Hakoniwa's reaper, the program before and after its exec) must be
/// gone for real: same pid and start time no longer in `/proc`, zombies included.
fn killed_after(delay: Duration) {
    let scratch = scratch();
    let mut mimik = test_helper::command(HELPER, "mimik")
        .env("MIMIK_LIVE_CHILD_SCRATCH", scratch.path())
        .env("MIMIK_LIVE_KILL_AT", "starting")
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let sweep = Sweep::watch(i32::try_from(mimik.id()).unwrap());
    let appeared = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        if sweep.snapshot().values().any(|line| !line.is_empty()) {
            break;
        }
        assert!(
            std::time::Instant::now() < appeared,
            "no process of the run"
        );
        std::thread::sleep(Duration::from_micros(100));
    }
    // The earliest process of the run that shows a command line (the smallest start time) is
    // the runner, before any server's exec. A process caught between its exit and its reaping
    // shows none, and a `--help` probe at the start is not the run's.
    let line = loop {
        let seen = sweep.snapshot();
        let earliest = seen
            .iter()
            .filter(|(_, line)| !line.is_empty() && !line.contains(" --help"))
            .min_by_key(|((_, start), _)| *start)
            .map(|(_, line)| line.clone());
        if let Some(line) = earliest {
            break line;
        }
        assert!(
            std::time::Instant::now() < appeared,
            "no command line seen: {seen:?}"
        );
        std::thread::sleep(Duration::from_micros(200));
    };
    assert!(
        line.contains("runner::tests::helper") || line.contains("assistant-runner"),
        "the earliest process seen is not the runner: {line}"
    );
    // Not the server's own command line.
    assert!(!line.contains("--api-key-file"), "{line}");
    std::thread::sleep(delay);
    mimik.kill().unwrap();
    mimik.wait().unwrap();
    let seen = sweep.finish();
    let left = wait_all_gone(&seen);
    assert!(
        left.is_empty(),
        "{delay:?}: {left:?} survived Mimik ({seen:?})"
    );
}

/// The detector itself: a child that survives its parent is reported, and one that does not
/// is not.
#[test]
#[ignore = "live: pinned llama-server, sandbox, test model (MIMIK_LIVE_*)"]
fn live_the_survivor_detector_sees_a_child_that_outlives_its_parent() {
    let mut parent = std::process::Command::new("sh")
        .args(["-c", "sleep 60 & wait"])
        .spawn()
        .unwrap();
    let sweep = Sweep::watch(i32::try_from(parent.id()).unwrap());
    while sweep.snapshot().is_empty() {
        std::thread::sleep(Duration::from_millis(1));
    }
    parent.kill().unwrap();
    parent.wait().unwrap();
    let seen = sweep.finish();
    std::thread::sleep(Duration::from_millis(300));
    let left = survivors(&seen);
    assert_eq!(left.len(), 1, "{seen:?}");
    for pid in left {
        rustix::process::kill_process(
            rustix::process::Pid::from_raw(pid).unwrap(),
            rustix::process::Signal::KILL,
        )
        .unwrap();
    }
    assert_eq!(wait_all_gone(&seen), Vec::<i32>::new());
}

#[test]
#[ignore = "live: pinned llama-server, sandbox, test model (MIMIK_LIVE_*)"]
fn live_killing_mimik_at_any_moment_of_a_start_leaves_no_process_of_the_run() {
    // Hakoniwa sets the parent-death signal itself in its two children after their fork and
    // does not re-check its parent, so a parent that dies between the fork and that call
    // leaves a child behind (HAKO-L5); the delays sweep that window.
    let delays_ms = [0_u64, 1, 2, 3, 5, 8, 12, 20, 35, 60];
    for round in 0..3 {
        for (n, delay) in delays_ms.iter().enumerate() {
            let _ = (round, n);
            killed_after(Duration::from_millis(*delay));
        }
    }
}

/// With Hakoniwa the reaper is outside the namespace: a forced stop is over only when the
/// server (PID 1 inside) is gone, sampled at the instant the exit is published.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: pinned llama-server, sandbox, test model (MIMIK_LIVE_*)"]
async fn live_the_exit_is_published_only_once_hakoniwas_server_is_gone() {
    let scratch = scratch();
    let settings = settings(scratch.path(), "probe-inner", Vec::new());
    let launch = Launch(real(settings));
    launch.preflight(Path::Cpu).await.unwrap();
    let probing = Arc::clone(&launch.0);
    let probed = tokio::task::spawn_blocking(move || {
        probing.probe_level(Path::Cpu, Level::Sandbox, &probing.work().enter())
    })
    .await
    .unwrap();
    assert_eq!(probed, Probe::Passed);
    let child = launch
        .start_real(1, Level::Sandbox, Path::Cpu)
        .await
        .unwrap();
    let runner = child.pid;
    let tree: Vec<i32> = descendants(runner);
    assert!(tree.len() >= 2, "the reaper and the server: {tree:?}");
    // The runner is killed as a crash would; the watcher takes it from there.
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(runner).unwrap(),
        rustix::process::Signal::KILL,
    )
    .unwrap();
    child.exited().await;
    for pid in &tree {
        assert!(
            !std::path::Path::new(&format!("/proc/{pid}")).exists() || !alive(*pid),
            "{pid} was alive when the exit was published: {tree:?}"
        );
    }
}

#[path = "live_gpu_tests.rs"]
mod gpu;

/// The Landlock report of a ready server: full on a kernel with Landlock ABI 6 or newer, which a run with
/// `MIMIK_LIVE_LANDLOCK=require` demands.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: pinned llama-server, sandbox, test model (MIMIK_LIVE_*)"]
async fn live_the_hakoniwa_closure_reports_landlock_in_full_force() {
    let scratch = scratch();
    let (_launch, child) = started_with(scratch.path(), Level::Sandbox, |_| {}).await;
    assert_landlock_report(child.landlock(), landlock_abi(), landlock_demanded());
    child.stop().await;
}

fn assert_landlock_report(
    reported: Option<super::super::hako::Landlock>,
    abi: i64,
    demanded: bool,
) {
    if demanded {
        assert!(abi >= 6, "this run requires Landlock ABI >=6, found {abi}");
    }
    assert_eq!(reported, Some(expected_landlock(abi)));
}

fn expected_landlock(abi: i64) -> super::super::hako::Landlock {
    use super::super::hako::Landlock;
    if abi <= 0 {
        Landlock::Unavailable
    } else {
        let number = u8::try_from(abi).expect("kernel ABI fits the report");
        if abi >= 6 {
            Landlock::Full { abi: number }
        } else {
            Landlock::Partial { abi: number }
        }
    }
}

#[test]
fn compatibility_mode_cannot_accept_an_incorrect_landlock_report_on_a_capable_kernel() {
    use super::super::hako::Landlock;
    for wrong in [
        None,
        Some(Landlock::Unavailable),
        Some(Landlock::Partial { abi: 11 }),
        Some(Landlock::Full { abi: 6 }),
    ] {
        assert!(
            std::panic::catch_unwind(|| assert_landlock_report(wrong, 11, false)).is_err(),
            "accepted {wrong:?}"
        );
    }
    for abi in [-1, 0, 1, 3, 5, 6, 11] {
        assert_landlock_report(Some(expected_landlock(abi)), abi, false);
    }
    assert_landlock_report(Some(Landlock::Full { abi: 11 }), 11, true);
    // A demanded run on an older kernel fails, whatever was reported.
    assert!(
        std::panic::catch_unwind(|| {
            assert_landlock_report(Some(Landlock::Partial { abi: 4 }), 4, true);
        })
        .is_err()
    );
}

/// A kernel without Landlock, simulated by the runner's seccomp filter answering `errno` to
/// the three Landlock calls (inherited by the sandbox's closure): the version probe fails,
/// the start goes on, the server serves and the report says "not available". The libtest
/// runner is used whatever `MIMIK_LIVE_RUNNER` says (the filter is its helper's).
async fn without_landlock_still_starts(errno: &str) {
    let scratch = scratch();
    let (_launch, child) = started_with(scratch.path(), Level::Sandbox, |settings| {
        settings.runner = RunnerCommand {
            program: std::env::current_exe().unwrap(),
            args: helper_args(RUNNER).into_iter().map(Into::into).collect(),
            env: vec![
                (test_helper::VARIABLE.into(), "runner".into()),
                ("MIMIK_TEST_NO_LANDLOCK".into(), errno.into()),
            ],
        };
    })
    .await;
    assert_eq!(
        child.landlock(),
        Some(super::super::hako::Landlock::Unavailable)
    );
    // It serves: the token-checked route answers.
    let (status, _) = raw(&child, "/v1/chat/completions", None, &short_request(4)).await;
    assert_eq!(status, 401);
    child.stop().await;
}

/// `ENOSYS`: a kernel built without Landlock.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: pinned llama-server, sandbox, test model (MIMIK_LIVE_*)"]
async fn live_a_kernel_without_landlock_still_starts_and_is_reported_as_not_available() {
    without_landlock_still_starts("ENOSYS").await;
}

/// `EOPNOTSUPP`: Landlock disabled at boot.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: pinned llama-server, sandbox, test model (MIMIK_LIVE_*)"]
async fn live_landlock_disabled_at_boot_still_starts_and_is_reported_as_not_available() {
    without_landlock_still_starts("EOPNOTSUPP").await;
}

/// `EPERM`: a seccomp or service-manager filter around Mimik refuses the Landlock calls.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: pinned llama-server, sandbox, test model (MIMIK_LIVE_*)"]
async fn live_landlock_refused_by_a_filter_still_starts_and_is_reported_as_not_available() {
    without_landlock_still_starts("EPERM").await;
}

/// Scoping, with a peer outside the domain in the same network namespace (which the sandbox
/// itself cannot offer: its only other process is Hakoniwa's reaper): this process listens on
/// an abstract Unix socket and is the signal target; a re-executed copy of the test binary
/// applies the very ruleset code the closure runs and tries both.
#[test]
#[ignore = "live: Landlock in the kernel"]
fn live_landlock_refuses_abstract_sockets_and_signals_outside_its_domain() {
    use std::os::linux::net::SocketAddrExt;
    let name = format!("mimik-landlock-scope-{}", std::process::id());
    let listener = std::os::unix::net::UnixListener::bind_addr(
        &std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).unwrap(),
    )
    .unwrap();
    listener.set_nonblocking(true).unwrap();
    let output = test_helper::command(HELPER, "landlock-scope")
        .env("MIMIK_SCOPE_ABSTRACT", &name)
        .output()
        .unwrap();
    let report = test_helper::report(&output);
    let status = report["status"].as_str().unwrap();
    let abi = landlock_abi();
    assert_eq!(status, expected_landlock(abi).describe());
    if abi < 6 {
        assert!(
            !landlock_demanded(),
            "this run requires Landlock ABI >=6, found {abi}"
        );
        eprintln!("SKIPPED: Landlock is {status} on this kernel: the scope assertions need ABI 6");
        return;
    }
    assert!(status.starts_with("full"), "{report}");
    assert_eq!(report["abstract_outside"], json!(libc::EPERM), "{report}");
    assert_eq!(report["signal_outside"], json!(libc::EPERM), "{report}");
    // Controls: a socket made inside the domain and the process itself stay reachable.
    assert_eq!(report["abstract_inside"], json!(0), "{report}");
    assert_eq!(report["signal_self"], json!(0), "{report}");
    // Nothing reached this process's listener.
    assert!(listener.accept().is_err());
}

/// The scope helper: applies the closure's ruleset to itself, then reports.
fn landlock_scope() {
    use super::super::hako::{Grant, Rule, apply_landlock};
    use std::os::linux::net::SocketAddrExt;
    let rules = [
        Rule {
            path: "/proc".into(),
            grant: Grant::Read,
        },
        Rule {
            path: "/tmp".into(),
            grant: Grant::ReadWrite,
        },
    ];
    let applied = apply_landlock(&rules).unwrap();
    let outside = std::env::var("MIMIK_SCOPE_ABSTRACT").unwrap();
    let connect = |name: &str| {
        let address = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).unwrap();
        errno_of(std::os::unix::net::UnixStream::connect_addr(&address))
    };
    let inside_name = format!("{outside}-inside");
    let _inside = std::os::unix::net::UnixListener::bind_addr(
        &std::os::unix::net::SocketAddr::from_abstract_name(inside_name.as_bytes()).unwrap(),
    )
    .unwrap();
    // SAFETY: signal 0 only checks that the signal could be sent.
    let signal = |pid: i32| test_helper::errno(i64::from(unsafe { libc::kill(pid, 0) }));
    test_helper::print(&json!({
        "status": applied.describe(),
        "abstract_outside": connect(&outside),
        "abstract_inside": connect(&inside_name),
        "signal_outside": signal(rustix::process::getppid().map_or(1, |pid| pid.as_raw_nonzero().get())),
        "signal_self": signal(std::process::id().try_into().unwrap()),
    }));
}
