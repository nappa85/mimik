//! The endpoint check against processes Mimik does not control: helper processes (the test
//! binary re-executed) as squatters, second listeners and impostor peers. The accepted
//! level-1 shape needs a real bubblewrap sandbox and is a live test (`live_tests.rs`).
use super::super::{files, test_helper};
use super::*;
use serde_json::{Value, json};
use std::io::{BufRead, Read, Write};
use std::os::fd::AsFd;
use std::process::{Child, Stdio};

const HELPER: &str = "assistant::runtime::endpoint::tests::helper";

fn reuse_port_listener(port: u16) -> std::net::TcpListener {
    let socket = rustix::net::socket(
        rustix::net::AddressFamily::INET,
        rustix::net::SocketType::STREAM,
        None,
    )
    .unwrap();
    rustix::net::sockopt::set_socket_reuseport(&socket, true).unwrap();
    rustix::net::bind(&socket, &SocketAddr::from((Ipv4Addr::LOCALHOST, port))).unwrap();
    rustix::net::listen(&socket, 16).unwrap();
    std::net::TcpListener::from(socket)
}

/// Helper modes: `tcp:<port>:<reuse 0|1>` listens on TCP, `unix:<path>` on a Unix socket;
/// both report the port, then wait for standard input to close and report how many
/// connections arrived.
#[test]
fn helper() {
    let Some(mode) = test_helper::mode() else {
        return;
    };
    let parts: Vec<&str> = mode.split(':').collect();
    let accepted = match parts.as_slice() {
        ["tcp", port, reuse] => {
            let port: u16 = port.parse().unwrap();
            let listener = if *reuse == "1" {
                reuse_port_listener(port)
            } else {
                std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap()
            };
            listener.set_nonblocking(true).unwrap();
            test_helper::print(&json!({"port": listener.local_addr().unwrap().port()}));
            let mut line = String::new();
            let _ = std::io::stdin().lock().read_line(&mut line);
            std::iter::from_fn(|| listener.accept().ok()).count()
        }
        ["unix", path] => {
            let listener = std::os::unix::net::UnixListener::bind(path).unwrap();
            listener.set_nonblocking(true).unwrap();
            test_helper::print(&json!({"port": 0}));
            let mut line = String::new();
            let _ = std::io::stdin().lock().read_line(&mut line);
            std::iter::from_fn(|| listener.accept().ok()).count()
        }
        _ => panic!("unknown helper mode {mode}"),
    };
    test_helper::print(&json!({"accepted": accepted}));
}

struct Helper {
    child: Child,
    port: u16,
}

impl Helper {
    fn start(mode: &str) -> Self {
        let mut child = test_helper::command(HELPER, mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut reader = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        while !line.contains("HELPER ") {
            line.clear();
            assert!(
                reader.read_line(&mut line).unwrap() > 0,
                "the helper stopped"
            );
        }
        let report: Value =
            serde_json::from_str(&line[line.find("HELPER ").unwrap() + 7..]).unwrap();
        child.stdout = Some(reader.into_inner());
        Self {
            port: u16::try_from(report["port"].as_u64().unwrap()).unwrap(),
            child,
        }
    }

    /// Ends the helper and returns how many connections it accepted.
    fn accepted(mut self) -> u64 {
        drop(self.child.stdin.take());
        let mut rest = String::new();
        self.child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut rest)
            .unwrap();
        self.child.wait().unwrap();
        let line = rest.lines().find(|line| line.contains("HELPER ")).unwrap();
        let report: Value =
            serde_json::from_str(&line[line.find("HELPER ").unwrap() + 7..]).unwrap();
        report["accepted"].as_u64().unwrap()
    }
}

fn myself() -> Arc<Watched> {
    Arc::new(Watched::open(i32::try_from(std::process::id()).unwrap()).unwrap())
}

#[tokio::test]
async fn the_childs_own_listener_is_reached_even_if_it_accepts_late() {
    let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let connector = Loopback {
        port,
        child: myself(),
    };
    assert_eq!(connector.authority(), format!("127.0.0.1:{port}"));
    // Nothing accepts yet: the connection completes on the listen backlog.
    let stream = connector.connect().await.unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));
    let (mut accepted, _) = listener.accept().unwrap();
    let Stream::Tcp(mut stream) = stream else {
        panic!("a TCP stream")
    };
    tokio::io::AsyncWriteExt::write_all(&mut stream, b"x")
        .await
        .unwrap();
    let mut byte = [0];
    accepted.read_exact(&mut byte).unwrap();
    assert_eq!(&byte, b"x");
}

#[tokio::test]
async fn no_listener_yet_is_not_ready() {
    let port = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let error = Loopback {
        port,
        child: myself(),
    }
    .connect()
    .await
    .err()
    .unwrap();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionRefused);
}

#[tokio::test]
async fn a_squatter_on_the_port_gets_nothing() {
    let squatter = Helper::start("tcp:0:0");
    let error = Loopback {
        port: squatter.port,
        child: myself(),
    }
    .connect()
    .await
    .err()
    .unwrap();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(squatter.accepted(), 0);
}

#[tokio::test]
async fn a_second_reuseport_listener_fails_the_attempt() {
    let ours = reuse_port_listener(0);
    let port = ours.local_addr().unwrap().port();
    let second = Helper::start(&format!("tcp:{port}:1"));
    let error = Loopback {
        port,
        child: myself(),
    }
    .connect()
    .await
    .err()
    .unwrap();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(second.accepted(), 0);
    ours.set_nonblocking(true).unwrap();
    assert!(ours.accept().is_err());
}

#[tokio::test]
async fn a_child_that_has_exited_is_not_reached() {
    let helper = Helper::start("tcp:0:0");
    let child = Arc::new(Watched::open(i32::try_from(helper.child.id()).unwrap()).unwrap());
    let port = helper.port;
    assert_eq!(helper.accepted(), 0);
    let error = Loopback { port, child }.connect().await.err().unwrap();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
}

fn own_namespace() -> u64 {
    rustix::fs::stat("/proc/self/ns/pid").unwrap().st_ino
}

#[tokio::test]
async fn a_peer_outside_the_sandbox_is_refused_even_with_mimiks_uid() {
    let directory = tempfile::tempdir().unwrap();
    let dir = files::open_dir(directory.path(), files::Owners::Mine, false).unwrap();
    // An impostor socket swapped into the directory, served by this very process.
    let _listener = std::os::unix::net::UnixListener::bind(directory.path().join(SOCKET)).unwrap();
    let reaper = Helper::start("tcp:0:0");
    let sandbox = Arc::new(Sandbox {
        reaper: Watched::open(i32::try_from(reaper.child.id()).unwrap()).unwrap(),
        pid_namespace: own_namespace(),
    });
    let connector = SandboxSocket {
        directory: Arc::new(dir),
        sandbox,
    };
    let error = connector.connect().await.err().unwrap();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(
        error
            .to_string()
            .contains("not in this generation's sandbox"),
        "{error}"
    );
    let _ = reaper.accepted();
}

#[tokio::test]
async fn a_child_of_the_reaper_outside_a_pid_namespace_is_refused() {
    // UID and parent match (the helper is this process's child, and this process plays the
    // reaper) but the peer is not one PID namespace deep: never accepted.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(SOCKET);
    let peer = Helper::start(&format!("unix:{}", path.display()));
    let dir = files::open_dir(directory.path(), files::Owners::Mine, false).unwrap();
    let connector = SandboxSocket {
        directory: Arc::new(dir),
        sandbox: Arc::new(Sandbox {
            reaper: Watched::open(i32::try_from(std::process::id()).unwrap()).unwrap(),
            pid_namespace: own_namespace(),
        }),
    };
    let error = connector.connect().await.err().unwrap();
    assert!(
        error.to_string().contains("not one namespace deep"),
        "{error}"
    );
    // The connection was made (the check needs the peer), but nothing was written to it.
    let _ = peer.accepted();
}

#[tokio::test]
async fn the_directory_descriptor_path_works_past_the_sun_path_limit() {
    let directory = tempfile::tempdir().unwrap();
    let mut deep = directory.path().to_path_buf();
    while deep.as_os_str().len() < 140 {
        deep.push("a-long-directory-name");
    }
    std::fs::create_dir_all(&deep).unwrap();
    let path = deep.join(SOCKET);
    assert!(path.as_os_str().len() > 108);
    // Binding by that path fails; the server side binds through a descriptor path too.
    let dir = files::open_dir(&deep, files::Owners::Mine, false).unwrap();
    let listener = std::os::unix::net::UnixListener::bind(format!(
        "/proc/self/fd/{}/{SOCKET}",
        dir.as_raw_fd()
    ))
    .unwrap();
    assert!(tokio::net::UnixStream::connect(&path).await.is_err());
    let mut stream = connect_in(&dir).await.unwrap();
    tokio::io::AsyncWriteExt::write_all(&mut stream, b"y")
        .await
        .unwrap();
    let (mut accepted, _) = listener.accept().unwrap();
    let mut byte = [0];
    accepted.read_exact(&mut byte).unwrap();
    assert_eq!(&byte, b"y");
    let _ = accepted.write_all(b"z");
    let _ = stream.as_fd();
}

#[tokio::test]
async fn an_entry_that_is_not_a_socket_is_never_connected() {
    // A link (to a real socket elsewhere) or a plain file where the socket should be.
    let directory = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let _real = std::os::unix::net::UnixListener::bind(elsewhere.path().join("real.sock")).unwrap();
    std::os::unix::fs::symlink(
        elsewhere.path().join("real.sock"),
        directory.path().join(SOCKET),
    )
    .unwrap();
    let dir = files::open_dir(directory.path(), files::Owners::Mine, false).unwrap();
    let error = connect_in(&dir).await.err().unwrap();
    assert!(error.to_string().contains("not a socket"), "{error}");
    std::fs::remove_file(directory.path().join(SOCKET)).unwrap();
    std::fs::write(directory.path().join(SOCKET), b"").unwrap();
    let error = connect_in(&dir).await.err().unwrap();
    assert!(error.to_string().contains("not a socket"), "{error}");
}

#[test]
fn the_reported_reaper_must_be_the_runners_child_in_the_reported_namespace() {
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let pid = i32::try_from(child.id()).unwrap();
    let me = i32::try_from(std::process::id()).unwrap();
    let namespace = rustix::fs::stat(format!("/proc/{pid}/ns/pid"))
        .unwrap()
        .st_ino;
    assert!(open_reaper(pid, me, namespace).is_ok());
    let error = open_reaper(pid, 1, namespace).unwrap_err();
    assert!(
        error.to_string().contains("not bubblewrap's child"),
        "{error}"
    );
    let error = open_reaper(pid, me, namespace + 1).unwrap_err();
    assert!(
        error.to_string().contains("another PID namespace"),
        "{error}"
    );
    child.kill().unwrap();
    child.wait().unwrap();
}
