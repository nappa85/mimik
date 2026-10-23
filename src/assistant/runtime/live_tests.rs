//! Live tests: the pinned official llama-server (CPU build) under the real profiles, with the
//! CI test model (`stories260K.gguf`). Ignored in ordinary runs; run with `--ignored` where
//! bubblewrap works (the dev host, the `llama-runtime` CI job) and with:
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
const RUNNER: &str = "assistant::runtime::runner::tests::helper";

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
    Settings {
        data_dir: scratch.join("data"),
        models_dir: env("MIMIK_LIVE_MODELS"),
        runtime_dir: env("MIMIK_LIVE_RUNTIME"),
        model: MANIFEST.test_model.name.clone(),
        model_sha256: MANIFEST.test_model.sha256.clone(),
        alias: "stories".into(),
        threads: 2,
        ctx_size: MANIFEST.test_model.ctx_size,
        runner: RunnerCommand {
            program: exe.clone(),
            args: helper_args(RUNNER).into_iter().map(Into::into).collect(),
            env: vec![(test_helper::VARIABLE.into(), "runner".into())],
        },
        inner: InnerCommand {
            executable: exe,
            probe_args: args,
            probe_env: env_vars,
            shim_args: {
                let mut args = helper_args(HELPER);
                args.push("shim".into());
                args
            },
            shim_env: vec![(test_helper::VARIABLE.into(), "shim".into())],
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
        "adversary" => adversary(),
        "shim" => std::process::exit(super::super::probe::shim()),
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
    // What the command inherits: the shim closed bwrap's status, seccomp, token, directory and
    // marker descriptors before its exec.
    let descriptors: Vec<i64> = std::fs::read_dir("/proc/self/fd")
        .map(|listing| {
            listing
                .filter_map(Result::ok)
                .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
                .collect()
        })
        .unwrap_or_default();
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
    println!("{report}");
    // Answer the endpoint check like the probe does.
    super::super::probe::inner(&["1".into()]);
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
            probing.probe_level(Path::Cpu, Level::Bubblewrap, &probing.work().enter())
        })
        .await
        .unwrap();
        assert_eq!(probed, Probe::Passed);
        let child = launch.start(1, Level::Bubblewrap, Path::Cpu).await.unwrap();
        // "Ready" waits for /health; "starting" reports at once, while the server loads.
        if std::env::var("MIMIK_LIVE_KILL_AT").as_deref() != Ok("starting") {
            while child.server().health().await != Health::Ready {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        let pidfile =
            std::fs::read_to_string(scratch.join("data/run").join(rundir::PIDFILE)).unwrap();
        let bwrap: i32 = pidfile.lines().nth(1).unwrap().parse().unwrap();
        test_helper::print(&json!({"bwrap": bwrap}));
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
#[ignore = "live: pinned llama-server, bubblewrap, test model (MIMIK_LIVE_*)"]
fn live_the_probe_reaches_level_1_and_level_3() {
    let scratch = scratch();
    let real = real(settings(scratch.path(), "probe-inner", Vec::new()));
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime
        .block_on(Launch(Arc::clone(&real)).preflight(Path::Cpu))
        .unwrap();
    assert_eq!(
        real.probe_level(Path::Cpu, Level::Bubblewrap, &real.work().enter()),
        Probe::Passed
    );
    assert_eq!(
        real.probe_level(Path::Cpu, Level::Unconfined, &real.work().enter()),
        Probe::Passed
    );
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
    let launch = Launch(real(settings(scratch, "probe-inner", Vec::new())));
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
        assert!(std::time::Instant::now() < deadline, "no /health");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    child.answered_health();
    (launch, child)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: pinned llama-server, bubblewrap, test model (MIMIK_LIVE_*)"]
async fn live_level_1_serves_over_the_checked_socket_with_the_token_only() {
    let scratch = scratch();
    *drain::TAP.lock().unwrap() = Some(Vec::new());
    let (_launch, child) = started(scratch.path(), Level::Bubblewrap).await;
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
    // The token is in no command line of the sandbox's processes.
    let pidfile =
        std::fs::read_to_string(scratch.path().join("data/run").join(rundir::PIDFILE)).unwrap();
    let bwrap: i32 = pidfile.lines().nth(1).unwrap().parse().unwrap();
    for pid in std::iter::once(bwrap).chain(descendants(bwrap)) {
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
    // The marker reached neither stream.
    let lines = drain::TAP.lock().unwrap().take().unwrap();
    assert_ne!(lines, Vec::<String>::new());
    assert!(!lines.iter().any(|line| line.contains(marker)));
    for pid in descendants(bwrap) {
        assert!(!alive(pid), "{pid} outlived the stop");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: pinned llama-server, bubblewrap, test model (MIMIK_LIVE_*)"]
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

#[test]
#[ignore = "live: pinned llama-server, bubblewrap, test model (MIMIK_LIVE_*)"]
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
    let (report, verdict) = real
        .inner_report(Path::Cpu, Level::Bubblewrap, &real.work().enter())
        .unwrap();
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
    // Sized temporary space, when this bubblewrap has --size.
    if real.held().bwrap.as_ref().is_some_and(|bwrap| bwrap.size) {
        assert!(
            report["tmp_filled"].as_u64().unwrap() <= bwrap::TMP_BYTES,
            "{report}"
        );
        assert!(
            report["shm_filled"].as_u64().unwrap() <= bwrap::SHM_BYTES,
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
    let bwrap = loop {
        let mut line = String::new();
        assert!(
            std::io::BufRead::read_line(&mut reader, &mut line).unwrap() > 0,
            "mimik stopped"
        );
        if let Some(at) = line.find("HELPER ") {
            let value: Value = serde_json::from_str(&line[at + 7..]).unwrap();
            break i32::try_from(value["bwrap"].as_i64().unwrap()).unwrap();
        }
    };
    let tree: Vec<i32> = std::iter::once(bwrap).chain(descendants(bwrap)).collect();
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
#[ignore = "live: pinned llama-server, bubblewrap, test model (MIMIK_LIVE_*)"]
fn live_sigkill_of_mimik_kills_the_whole_sandbox() {
    sigkill_of_mimik_ends_the_sandbox("ready");
}

#[test]
#[ignore = "live: pinned llama-server, bubblewrap, test model (MIMIK_LIVE_*)"]
fn live_sigkill_of_mimik_during_startup_kills_the_whole_sandbox() {
    sigkill_of_mimik_ends_the_sandbox("starting");
}

/// A fake build in `scratch`: the fake llama-server (`MIMIK_LIVE_FAKE_SERVER`) as the only
/// executable, a fake model, the test binary as runner and shim.
fn fake_build(scratch: &std::path::Path, alias: &str) -> Arc<Real> {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;
    let server = std::fs::read(env("MIMIK_LIVE_FAKE_SERVER")).unwrap();
    let cpu = scratch.join("runtime/cpu");
    std::fs::create_dir_all(&cpu).unwrap();
    std::fs::write(cpu.join("llama-server"), &server).unwrap();
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
            sha256: hex::encode(Sha256::digest(&server)),
            size: server.len() as u64,
            mode: "0755".into(),
        }],
    };
    Arc::new(Real::with_build(settings, Some(build)))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: pinned llama-server, bubblewrap, test model (MIMIK_LIVE_*)"]
async fn live_a_server_that_forges_bubblewraps_errors_keeps_level_1() {
    use super::super::manager::{self, State};
    use crate::assistant::jobs::Assistant;
    let scratch = scratch();
    let real = fake_build(scratch.path(), "forge-bwrap");
    let assistant = Assistant::new();
    let handle = manager::spawn(
        assistant,
        Arc::new(Launch(Arc::clone(&real))),
        manager::Config {
            path: Path::Cpu,
            max_tokens: 64,
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
    assert_eq!(status.level, Some(Level::Bubblewrap), "{status:?}");
    assert!(
        !status
            .skipped
            .iter()
            .any(|(level, _)| *level == Level::Bubblewrap),
        "{status:?}"
    );
    assert!(matches!(&status.state, State::Starting(_)), "{status:?}");
    assert!(
        status
            .last_failure
            .as_deref()
            .is_some_and(|why| why.starts_with("the model server exited")),
        "{status:?}"
    );
    handle.shutdown().await;
}

#[tokio::test]
#[ignore = "live: pinned llama-server, bubblewrap, test model (MIMIK_LIVE_*)"]
async fn live_a_failure_after_the_level_1_spawn_leaves_no_process_behind() {
    let scratch = scratch();
    // The alias is in the command lines of bwrap, its reaper and the server: a unique one
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
        probing.probe_level(Path::Cpu, Level::Bubblewrap, &probing.work().enter())
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
        let result = launch.start(generation, Level::Bubblewrap, Path::Cpu).await;
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
#[ignore = "live: pinned llama-server, bubblewrap, test model (MIMIK_LIVE_*)"]
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

#[path = "live_gpu_tests.rs"]
mod gpu;
