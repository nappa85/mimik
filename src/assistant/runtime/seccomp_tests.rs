//! The seccomp program, installed in a helper process (the test binary re-executed), so the
//! test process itself stays unfiltered.
use super::super::test_helper::{self, errno};
use super::*;
use serde_json::{Value, json};
use std::os::fd::AsRawFd;

const HELPER: &str = "assistant::runtime::seccomp::tests::helper";

#[test]
fn the_program_checks_the_architecture_then_x32_then_clone3() {
    let sandbox = program(Profile::Sandbox);
    assert_eq!(sandbox[0], stmt(BPF_LD_W_ABS, ARCH_OFFSET));
    assert_eq!(sandbox[1], jump(BPF_JEQ_K, AUDIT_ARCH_X86_64, 1, 0));
    assert_eq!(sandbox[2], stmt(BPF_RET_K, SECCOMP_RET_KILL_PROCESS));
    assert_eq!(sandbox[3], stmt(BPF_LD_W_ABS, NR_OFFSET));
    assert_eq!(sandbox[4], jump(BPF_JGE_K, X32_SYSCALL_BIT, 0, 1));
    assert_eq!(sandbox[5], stmt(BPF_RET_K, SECCOMP_RET_ERRNO | EPERM));
    assert_eq!(sandbox[7], stmt(BPF_RET_K, SECCOMP_RET_ERRNO | ENOSYS));
    // The kernel's limit is 4,096 instructions.
    assert!(sandbox.len() < 4096);
    assert!(program(Profile::Tcp { exec_fd: 1023 }).len() < 4096);
    assert_eq!(bytes(&sandbox).len(), sandbox.len() * 8);
    assert_eq!(&bytes(&sandbox)[..8], &[0x20, 0, 0, 0, 4, 0, 0, 0]);
}

fn raw(number: i64, args: [i64; 5]) -> i64 {
    // SAFETY: test-only raw system calls with integer or null arguments, in a helper process
    // that exists only to make them.
    unsafe { libc::syscall(number, args[0], args[1], args[2], args[3], args[4]) }
}

fn socket(domain: i32, kind: i32, protocol: i32) -> i64 {
    let fd = raw(
        libc::SYS_socket,
        [domain.into(), kind.into(), protocol.into(), 0, 0],
    );
    let error = errno(fd);
    if fd >= 0 {
        // SAFETY: closing a descriptor this call just created.
        unsafe { libc::close(i32::try_from(fd).unwrap()) };
    }
    error
}

/// A raw process-creating call that, if it ever succeeds, ends the child at once.
fn spawn(number: i64, args: [i64; 5]) -> i64 {
    let pid = raw(number, args);
    if pid == 0 {
        // SAFETY: the child of a successful raw fork-like call exits without unwinding.
        unsafe { libc::_exit(0) };
    }
    if pid > 0 {
        let mut status = 0;
        // SAFETY: reaping the child just created.
        unsafe { libc::waitpid(i32::try_from(pid).unwrap(), &raw mut status, 0) };
        return 0;
    }
    errno(pid)
}

fn probes() -> Value {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let address = libc::sockaddr_in {
        sin_family: libc::sa_family_t::try_from(libc::AF_INET).unwrap(),
        sin_port: port.to_be(),
        sin_addr: libc::in_addr {
            s_addr: u32::from(std::net::Ipv4Addr::LOCALHOST).to_be(),
        },
        sin_zero: [0; 8],
    };
    let tcp = raw(
        libc::SYS_socket,
        [libc::AF_INET.into(), libc::SOCK_STREAM.into(), 0, 0, 0],
    );
    let fast_open = errno(raw(
        libc::SYS_sendto,
        [
            tcp,
            c"x".as_ptr() as i64,
            1,
            i64::from(libc::MSG_FASTOPEN),
            std::ptr::from_ref(&address) as i64,
        ],
    ));
    let one: i32 = 1;
    let fast_open_connect = errno(
        // SAFETY: setsockopt on a descriptor this helper owns, with a pointer to a local int.
        i64::from(unsafe {
            libc::setsockopt(
                i32::try_from(tcp).unwrap(),
                libc::IPPROTO_TCP,
                libc::TCP_FASTOPEN_CONNECT,
                std::ptr::from_ref(&one).cast(),
                4,
            )
        }),
    );
    let message = libc::msghdr {
        msg_name: std::ptr::from_ref(&address).cast_mut().cast(),
        msg_namelen: u32::try_from(std::mem::size_of::<libc::sockaddr_in>()).unwrap(),
        msg_iov: std::ptr::null_mut(),
        msg_iovlen: 0,
        msg_control: std::ptr::null_mut(),
        msg_controllen: 0,
        msg_flags: 0,
    };
    let fast_open_message = errno(raw(
        libc::SYS_sendmsg,
        [
            tcp,
            std::ptr::from_ref(&message) as i64,
            i64::from(libc::MSG_FASTOPEN),
            0,
            0,
        ],
    ));
    let fast_open_messages = errno(raw(
        libc::SYS_sendmmsg,
        [tcp, 0, 0, i64::from(libc::MSG_FASTOPEN), 0],
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    let listener_saw = listener.accept().is_ok();
    let thread = std::thread::spawn(|| 7).join().unwrap_or(0);
    json!({
        "udp": socket(libc::AF_INET, libc::SOCK_DGRAM, 0),
        "udp6": socket(libc::AF_INET6, libc::SOCK_DGRAM, 0),
        "raw": socket(libc::AF_INET, libc::SOCK_RAW, libc::IPPROTO_ICMP),
        "sctp": socket(libc::AF_INET, libc::SOCK_STREAM, libc::IPPROTO_SCTP),
        "netlink": socket(libc::AF_NETLINK, libc::SOCK_RAW, 0),
        "packet": socket(libc::AF_PACKET, libc::SOCK_RAW, 0),
        "unix_dgram": socket(libc::AF_UNIX, libc::SOCK_DGRAM, 0),
        "unix_seqpacket": socket(libc::AF_UNIX, libc::SOCK_SEQPACKET, 0),
        "tcp": socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0),
        "tcp_explicit": socket(libc::AF_INET, libc::SOCK_STREAM, libc::IPPROTO_TCP),
        "tcp6": socket(libc::AF_INET6, libc::SOCK_STREAM | libc::SOCK_NONBLOCK, 0),
        "unix_stream": socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0),
        "socketpair": errno(raw(libc::SYS_socketpair, [libc::AF_UNIX.into(), libc::SOCK_STREAM.into(), 0, 0, 0])),
        "fork": spawn(libc::SYS_fork, [0; 5]),
        "vfork": spawn(libc::SYS_vfork, [0; 5]),
        "clone_process": spawn(libc::SYS_clone, [libc::SIGCHLD.into(), 0, 0, 0, 0]),
        "clone3": errno(raw(libc::SYS_clone3, [0, 0, 0, 0, 0])),
        "thread": thread,
        "x32_getpid": errno(raw(0x4000_0000 | libc::SYS_getpid, [0; 5])),
        "io_uring": errno(raw(libc::SYS_io_uring_setup, [1, 0, 0, 0, 0])),
        // An invalid request: unfiltered it fails with EIO or ESRCH, never by tracing anyone.
        "ptrace": errno(raw(libc::SYS_ptrace, [0x7fff_ffff, 0, 0, 0, 0])),
        "pidfd_open": errno(raw(libc::SYS_pidfd_open, [i64::from(std::process::id()), 0, 0, 0, 0])),
        "kcmp": errno(raw(libc::SYS_kcmp, [0, 0, 0, 0, 0])),
        "bpf": errno(raw(libc::SYS_bpf, [0, 0, 0, 0, 0])),
        "unshare": errno(raw(libc::SYS_unshare, [libc::CLONE_NEWUSER.into(), 0, 0, 0, 0])),
        "clear_pdeathsig": errno(raw(libc::SYS_prctl, [libc::PR_SET_PDEATHSIG.into(), 0, 0, 0, 0])),
        "set_name": errno(raw(libc::SYS_prctl, [libc::PR_SET_NAME.into(), c"helper".as_ptr() as i64, 0, 0, 0])),
        "fast_open_sendto": fast_open,
        "fast_open_connect": fast_open_connect,
        "fast_open_sendmsg": fast_open_message,
        "fast_open_sendmmsg": fast_open_messages,
        "listener_saw": listener_saw,
    })
}

fn exec_probes(exec_fd: i32) -> Value {
    let argv = [c"/bin/true".as_ptr(), std::ptr::null()];
    let envp = [std::ptr::null::<libc::c_char>()];
    let other = rustix::fs::open(
        "/bin/true",
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .unwrap();
    let duplicate = rustix::io::fcntl_dupfd_cloexec(
        // SAFETY: `exec_fd` is open in this helper for its whole run.
        unsafe { std::os::fd::BorrowedFd::borrow_raw(exec_fd) },
        0,
    )
    .unwrap();
    let execveat = |fd: i32, flags: i32| {
        errno(raw(
            libc::SYS_execveat,
            [
                fd.into(),
                c"".as_ptr() as i64,
                argv.as_ptr() as i64,
                envp.as_ptr() as i64,
                flags.into(),
            ],
        ))
    };
    json!({
        "execve": errno(raw(libc::SYS_execve, [c"/bin/true".as_ptr() as i64, argv.as_ptr() as i64, envp.as_ptr() as i64, 0, 0])),
        "execveat_other": execveat(other.as_raw_fd(), libc::AT_EMPTY_PATH),
        "execveat_duplicate": execveat(duplicate.as_raw_fd(), libc::AT_EMPTY_PATH),
        "execveat_no_empty_path": execveat(exec_fd, 0),
    })
}

/// The number the TCP profile allows exec on, and the descriptor limit the runner sets.
const EXEC_FD: i32 = 1000;

fn exec_descriptor() -> i32 {
    let exe = rustix::fs::open(
        "/proc/self/exe",
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .unwrap();
    // SAFETY: `dup3` onto a number this helper does not use; the result is kept open on
    // purpose (it is the runtime descriptor) and never closed by Rust.
    let fd = unsafe {
        libc::dup3(
            std::os::fd::AsRawFd::as_raw_fd(&exe),
            EXEC_FD,
            libc::O_CLOEXEC,
        )
    };
    assert_eq!(fd, EXEC_FD);
    fd
}

fn limit_descriptors() {
    let limit = rustix::process::Rlimit {
        current: Some(u64::try_from(EXEC_FD).unwrap()),
        maximum: Some(u64::try_from(EXEC_FD).unwrap()),
    };
    rustix::process::setrlimit(rustix::process::Resource::Nofile, limit).unwrap();
}

/// Installs the TCP profile and reports the refused execs, then makes the one allowed.
fn tcp_exec() {
    let fd = exec_descriptor();
    limit_descriptors();
    install(&program(Profile::Tcp {
        exec_fd: u32::try_from(fd).unwrap(),
    }))
    .unwrap();
    let mut report = exec_probes(fd);
    report["before_exec"] = json!(true);
    test_helper::print(&report);
    // The allowed exec: this same test binary, as the "after-exec" helper.
    let exe = c"helper-after-exec";
    let test = std::ffi::CString::new(HELPER).unwrap();
    let argv = [
        exe.as_ptr(),
        test.as_ptr(),
        c"--exact".as_ptr(),
        c"--nocapture".as_ptr(),
        c"--test-threads=1".as_ptr(),
        std::ptr::null(),
    ];
    let variable = std::ffi::CString::new(format!("{}=after-exec", test_helper::VARIABLE)).unwrap();
    let envp = [variable.as_ptr(), std::ptr::null()];
    let result = raw(
        libc::SYS_execveat,
        [
            fd.into(),
            c"".as_ptr() as i64,
            argv.as_ptr() as i64,
            envp.as_ptr() as i64,
            libc::AT_EMPTY_PATH.into(),
        ],
    );
    test_helper::print(&json!({"exec_failed": errno(result)}));
}

/// In the executed image: no descriptor can take the exec number any more.
fn after_exec() {
    // The runtime descriptor was close-on-exec, and no new descriptor can take its
    // number: fill every free number below the limit, then try once more.
    let mut held = Vec::new();
    let exhausted = loop {
        match rustix::fs::open(
            "/dev/null",
            rustix::fs::OFlags::RDONLY,
            rustix::fs::Mode::empty(),
        ) {
            Ok(fd) => held.push(fd),
            Err(error) => break error.raw_os_error(),
        }
        if held.len() > 4096 {
            break 0;
        }
    };
    let highest = held.iter().map(AsRawFd::as_raw_fd).max().unwrap_or(-1);
    drop(held);
    // SAFETY: dup2 of standard input onto a number at the limit; it must fail.
    let dup2 = errno(i64::from(unsafe { libc::dup2(0, EXEC_FD) }));
    let raise = rustix::process::setrlimit(
        rustix::process::Resource::Nofile,
        rustix::process::Rlimit {
            current: Some(4096),
            maximum: Some(4096),
        },
    )
    .err()
    .map_or(0, |error| i64::from(error.raw_os_error()));
    let argv = [c"x".as_ptr(), std::ptr::null()];
    let envp = [std::ptr::null::<libc::c_char>()];
    let again = errno(raw(
        libc::SYS_execveat,
        [
            EXEC_FD.into(),
            c"".as_ptr() as i64,
            argv.as_ptr() as i64,
            envp.as_ptr() as i64,
            libc::AT_EMPTY_PATH.into(),
        ],
    ));
    test_helper::print(&json!({
        "after_exec": true,
        "exhausted": exhausted,
        "highest": highest,
        "dup2": dup2,
        "raise": raise,
        "execveat_again": again,
    }));
}

#[test]
fn helper() {
    let Some(mode) = test_helper::mode() else {
        return;
    };
    match mode.as_str() {
        "unfiltered" => test_helper::print(&probes()),
        "sandbox" => {
            install(&program(Profile::Sandbox)).unwrap();
            test_helper::print(&probes());
        }
        "tcp" => {
            install(&program(Profile::Tcp {
                exec_fd: u32::try_from(EXEC_FD).unwrap(),
            }))
            .unwrap();
            test_helper::print(&probes());
        }
        "tcp-exec" => tcp_exec(),
        "after-exec" => after_exec(),
        other => panic!("unknown helper mode {other}"),
    }
}

fn eperm() -> Value {
    json!(libc::EPERM)
}

#[test]
fn the_sandbox_profile_refuses_what_the_plan_lists() {
    let report = test_helper::report(&test_helper::run(HELPER, "sandbox"));
    for refused in [
        "udp",
        "udp6",
        "raw",
        "sctp",
        "netlink",
        "packet",
        "unix_dgram",
        "unix_seqpacket",
        "socketpair",
        "fork",
        "vfork",
        "clone_process",
        "x32_getpid",
        "io_uring",
        "ptrace",
        "pidfd_open",
        "kcmp",
        "bpf",
        "unshare",
        "clear_pdeathsig",
        "fast_open_sendto",
        "fast_open_connect",
        "fast_open_sendmsg",
        "fast_open_sendmmsg",
    ] {
        assert_eq!(report[refused], eperm(), "{refused}: {report}");
    }
    assert_eq!(report["clone3"], json!(libc::ENOSYS), "{report}");
    for allowed in ["tcp", "tcp_explicit", "tcp6", "unix_stream", "set_name"] {
        assert_eq!(report[allowed], json!(0), "{allowed}: {report}");
    }
    // Threads still start; the fast-open sends reached no one.
    assert_eq!(report["thread"], json!(7));
    assert_eq!(report["listener_saw"], json!(false));
}

#[test]
fn without_the_program_the_probes_are_not_refused() {
    // The control: the same probes unfiltered get other answers, so the refusals above are
    // the program's.
    let report = test_helper::report(&test_helper::run(HELPER, "unfiltered"));
    assert_eq!(report["fork"], json!(0));
    assert_eq!(report["udp"], json!(0));
    assert_eq!(report["clear_pdeathsig"], json!(0));
    assert_ne!(report["ptrace"], eperm());
}

#[test]
fn the_tcp_profile_refuses_the_same_and_restricts_exec() {
    let report = test_helper::report(&test_helper::run(HELPER, "tcp"));
    assert_eq!(report["fork"], eperm());
    assert_eq!(report["udp"], eperm());
    assert_eq!(report["clear_pdeathsig"], eperm());
    assert_eq!(report["clone3"], json!(libc::ENOSYS));
    assert_eq!(report["thread"], json!(7));
}

#[test]
fn exec_is_allowed_once_on_the_runtime_descriptor_and_never_again() {
    let output = test_helper::run(HELPER, "tcp-exec");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let reports: Vec<Value> = stdout
        .lines()
        .filter_map(|line| line.find("HELPER ").map(|at| &line[at + 7..]))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        reports.len(),
        2,
        "{stdout} {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let before = &reports[0];
    for refused in [
        "execve",
        "execveat_other",
        "execveat_duplicate",
        "execveat_no_empty_path",
    ] {
        assert_eq!(before[refused], eperm(), "{refused}: {before}");
    }
    // The second report is from the executed image.
    let after = &reports[1];
    assert_eq!(after["after_exec"], json!(true), "{after}");
    assert_eq!(after["exhausted"], json!(libc::EMFILE));
    assert!(after["highest"].as_i64().unwrap() < i64::from(EXEC_FD));
    assert_eq!(after["dup2"], json!(libc::EBADF));
    assert_eq!(after["raise"], eperm());
    assert_eq!(after["execveat_again"], json!(libc::EBADF));
}
