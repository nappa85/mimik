//! Live tests: the pinned official llama-server (CPU build) under the real profiles, with the
//! CI test model (`stories260K.gguf`). Ignored in ordinary runs; run with `--ignored` where
//! bubblewrap works (the dev host, the `llama-runtime` CI job) and with:
//!
//! - `MIMIK_LIVE_RUNTIME`: the runtime folder (holding `cpu/` with exactly the pinned files);
//! - `MIMIK_LIVE_MODELS`: a 0700 models folder holding `stories260K.gguf`;
//! - `MIMIK_LIVE_SCRATCH`: a folder whose ancestors are not writable by others.
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
            args,
            env: env_vars,
        },
    }
}

fn scratch() -> tempfile::TempDir {
    tempfile::Builder::new()
        .tempdir_in(env("MIMIK_LIVE_SCRATCH"))
        .unwrap()
}

fn real(settings: Settings) -> Arc<Real> {
    Arc::new(Real::new(settings))
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
        "mimik" => play_mimik(),
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
    let report = json!({
        "ok": true,
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
        let probed =
            tokio::task::spawn_blocking(move || probing.probe_level(Path::Cpu, Level::Bubblewrap))
                .await
                .unwrap();
        assert_eq!(probed, Ok(()));
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
    assert_eq!(real.probe_level(Path::Cpu, Level::Bubblewrap), Ok(()));
    assert_eq!(real.probe_level(Path::Cpu, Level::Unconfined), Ok(()));
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
    let probed = tokio::task::spawn_blocking(move || probing.probe_level(Path::Cpu, level))
        .await
        .unwrap();
    assert_eq!(probed, Ok(()));
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
    let (report, verdict) = real.inner_report(Path::Cpu, Level::Bubblewrap).unwrap();
    assert_eq!(verdict, Ok(()), "{report}");
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
