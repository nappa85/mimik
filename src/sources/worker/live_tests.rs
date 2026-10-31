//! Host-only confinement tests; all adversarial behaviour is cfg(test), never shipped.
use super::{
    launch::{Options, execute},
    *,
};
use crate::sandbox::{process::Watched, runner::RunnerCommand};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const HELPER: &str = "sources::worker::live_tests::worker_helper";
const PARENT: &str = "sources::worker::live_tests::parent_helper";

fn enabled() -> bool {
    std::env::var_os("MIMIK_LIVE_RUNNER").is_some()
}
fn options(mode: &str) -> Options {
    let binary = std::env::current_exe().unwrap();
    Options {
        observed_exit: std::sync::Arc::default(),
        runner: RunnerCommand {
            program: std::env::var_os("MIMIK_LIVE_RUNNER").unwrap().into(),
            args: vec!["assistant-runner".into(), "--spec-fd".into(), "3".into()],
            env: vec![],
        },
        executable: binary,
        argv: vec![
            "/opt/mimik/mimik".into(),
            HELPER.into(),
            "--exact".into(),
            "--nocapture".into(),
            "--test-threads=1".into(),
            "--include-ignored".into(),
        ],
        env: vec![
            ("MIMIK_PDF_TEST".into(), mode.into()),
            ("HOME".into(), "/tmp".into()),
        ],
        wall: Some(Duration::from_secs(10)),
        cpu: None,
    }
}
fn pdf() -> (tempfile::TempDir, PathBuf) {
    use lopdf::{Document, Object, dictionary};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.pdf");
    let mut doc = Document::with_version("1.7");
    let pages = doc.new_object_id();
    let page = doc.add_object(dictionary!{"Type"=>"Page", "Parent"=>pages, "MediaBox"=>vec![0.into(),0.into(),100.into(),200.into()]});
    doc.objects.insert(
        pages,
        Object::Dictionary(dictionary! {"Type"=>"Pages", "Count"=>1, "Kids"=>vec![page.into()]}),
    );
    let root = doc.add_object(dictionary! {"Type"=>"Catalog", "Pages"=>pages});
    doc.trailer.set("Root", root);
    doc.save(&path).unwrap();
    (dir, path)
}
fn inspection(path: &Path, opts: &Options, cancel: &Cancel) -> Result<Inspection, Reason> {
    execute(
        path,
        &Spec {
            operation: records::Operation::Inspect,
            pages: vec![],
            settings: RenderSettings::default(),
            input: "/in/document.pdf".into(),
        },
        cancel,
        opts,
        |pipe| match reader(
            pipe,
            opts.env.iter().any(|(key, _)| key == "MIMIK_PDF_TEST"),
        ) {
            Ok(pipe) => records::read_inspection_tracked(pipe),
            Err(error) => (Err(error), None),
        },
    )
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn worker_helper() {
    use std::io::Write;
    if std::env::var_os("MIMIK_PDF_TEST").is_none() {
        return;
    }
    // libtest writes a prefix before entering the helper. The test parent strips only
    // this bounded prefix; production readers always receive the pristine worker pipe.
    writeln!(std::io::stdout(), "\nPDF_TEST_BEGIN").unwrap();
    std::io::stdout().flush().unwrap();
    std::process::exit(match run_entry(Path::new("/run/spec.json")) {
        Ok(()) => 0,
        Err(e) => e.exit_code(),
    });
}

fn reader(
    pipe: std::process::ChildStdout,
    helper: bool,
) -> Result<std::io::BufReader<std::process::ChildStdout>, Error> {
    use std::io::BufRead;
    let mut pipe = std::io::BufReader::new(pipe);
    if helper {
        let mut prefix = String::new();
        loop {
            let mut line = String::new();
            if (&mut pipe)
                .take(4097 - prefix.len() as u64)
                .read_line(&mut line)
                .map_err(|_| Error::Unreadable)?
                == 0
            {
                return Err(Error::Unreadable);
            }
            prefix.push_str(&line);
            if prefix.len() > 4096 {
                return Err(Error::Unreadable);
            }
            if line == "PDF_TEST_BEGIN\n" {
                break;
            }
        }
    }
    Ok(pipe)
}

pub(super) fn misbehave() -> Result<(), Error> {
    use std::io::Write;
    let mode = std::env::var("MIMIK_PDF_TEST").unwrap_or_default();
    match mode.as_str() {
        "socket" => {
            // SAFETY: cfg(test)-only adversarial call, expected to kill this disposable
            // process. If it unexpectedly returns, no pointer is passed to the kernel.
            unsafe {
                libc::syscall(libc::SYS_socket, libc::AF_INET, libc::SOCK_STREAM, 0);
            }
        }
        "exec" => {
            let argv = [c"/opt/mimik/mimik".as_ptr(), std::ptr::null()];
            // SAFETY: cfg(test)-only exec probe with valid NUL-terminated argv/envp.
            // A successful exec replaces this disposable helper; seccomp must kill it.
            unsafe {
                libc::syscall(libc::SYS_execve, argv[0], argv.as_ptr(), argv[1..].as_ptr());
            }
        }
        "clone" => {
            // SAFETY: cfg(test)-only fork-style clone (no shared memory or child stack),
            // expected to be killed by the second filter before a child is created.
            let result = unsafe { libc::syscall(libc::SYS_clone, libc::SIGCHLD, 0, 0, 0, 0) };
            if result == 0 {
                std::process::exit(0);
            }
        }
        "sleep" => std::thread::sleep(Duration::from_secs(60)),
        "cpu" => loop {
            std::hint::spin_loop();
        },
        "tmpfs" => {
            let bytes = vec![0_u8; 9 << 20];
            for name in ["/tmp/a", "/tmp/b"] {
                File::create(name)
                    .and_then(|mut f| f.write_all(&bytes))
                    .map_err(|_| Error::Unreadable)?;
            }
        }
        "fsize" => File::create("/tmp/oversize")
            .and_then(|f| f.set_len(17 << 20))
            .map_err(|_| Error::Unreadable)?,
        "denied" => {
            let home = std::env::var("MIMIK_PDF_HOST_HOME").unwrap();
            let data = std::env::var("MIMIK_PDF_HOST_DATA").unwrap();
            let output = std::env::var("MIMIK_PDF_HOST_WRITE").unwrap();
            if std::fs::read(home).is_ok()
                || std::fs::read(data).is_ok()
                || File::create(output).is_ok()
            {
                return Err(Error::Unsupported);
            }
        }
        _ => {}
    }
    Ok(())
}
use std::fs::File;

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_inspect_and_render_without_proc() {
    if !enabled() {
        return;
    }
    let (_dir, path) = pdf();
    // Production executable, production argv and production budgets.
    let mut opts = options("normal");
    opts.executable = opts.runner.program.clone();
    opts.argv = vec![
        "/opt/mimik/mimik".into(),
        "pdf-worker".into(),
        "--spec".into(),
        "/run/spec.json".into(),
    ];
    opts.env.retain(|(key, _)| key != "MIMIK_PDF_TEST");
    let inspected = inspection(&path, &opts, &Cancel::default()).unwrap();
    assert_eq!(inspected.document.pages, 1);
    let spec = Spec {
        operation: records::Operation::Render,
        pages: vec![1],
        settings: RenderSettings::default(),
        input: "/in/document.pdf".into(),
    };
    let pages = execute(&path, &spec, &Cancel::default(), &opts, |pipe| {
        let mut reader = records::RenderReader::new(pipe, inspected.pages).unwrap();
        let result = (|| {
            let p = reader.next_page()?.unwrap();
            assert_ne!(p.png, [] as [u8; 0]);
            assert!(reader.next_page()?.is_none());
            Ok(1)
        })();
        (result, reader.terminal_error())
    })
    .unwrap();
    assert_eq!(pages, 1);
}

fn killed(mode: &str) {
    if !enabled() {
        return;
    }
    let (_dir, path) = pdf();
    let opts = options(mode);
    assert_eq!(
        inspection(&path, &opts, &Cancel::default()).unwrap_err(),
        Reason::Document(Error::Unreadable)
    );
    let code = opts
        .observed_exit
        .load(std::sync::atomic::Ordering::Acquire);
    match mode {
        "socket" | "exec" | "clone" => assert_eq!(code, 128 + libc::SIGSYS),
        "filter-fail" => assert_eq!(code, super::filter::INSTALL_FAILED),
        "tmpfs" | "fsize" => assert_eq!(code, Error::Unreadable.exit_code()),
        _ => {}
    }
}
#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_socket_is_killed() {
    killed("socket");
}
#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_exec_is_killed() {
    killed("exec");
}
#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_clone_is_killed() {
    killed("clone");
}
#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_tmpfs_exhaustion_fails_without_hanging() {
    killed("tmpfs");
}
#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_file_size_limit_fails_without_hanging() {
    killed("fsize");
}
#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_second_filter_install_failure() {
    killed("filter-fail");
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_host_files_are_invisible_and_unwritable() {
    if !enabled() {
        return;
    }
    let (dir, path) = pdf();
    let mut opts = options("denied");
    for (variable, name) in [
        ("MIMIK_PDF_HOST_HOME", "home-canary"),
        ("MIMIK_PDF_HOST_DATA", "data-canary"),
    ] {
        let canary = dir.path().join(name);
        std::fs::write(&canary, b"synthetic-private-canary").unwrap();
        opts.env
            .push((variable.into(), canary.to_str().unwrap().into()));
    }
    let output = dir.path().join("forbidden");
    opts.env.push((
        "MIMIK_PDF_HOST_WRITE".into(),
        output.to_str().unwrap().into(),
    ));
    inspection(&path, &opts, &Cancel::default()).unwrap();
    assert!(!output.exists());
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_cpu_hard_limit_kills() {
    if !enabled() {
        return;
    }
    let (_dir, path) = pdf();
    let mut opts = options("cpu");
    opts.cpu = Some(1);
    let started = Instant::now();
    assert_eq!(
        inspection(&path, &opts, &Cancel::default()).unwrap_err(),
        Error::Unreadable.into()
    );
    assert!(started.elapsed() < Duration::from_secs(8));
    assert_eq!(
        opts.observed_exit
            .load(std::sync::atomic::Ordering::Acquire),
        128 + libc::SIGKILL
    );
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_wall_deadline_is_parent_timeout() {
    if !enabled() {
        return;
    }
    let (_dir, path) = pdf();
    let mut opts = options("sleep");
    opts.wall = Some(Duration::from_secs(2));
    let started = Instant::now();
    assert_eq!(
        inspection(&path, &opts, &Cancel::default()).unwrap_err(),
        Error::Timeout.into()
    );
    assert!(started.elapsed() < Duration::from_secs(8));
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_cancel_mid_render_reaps_tree() {
    if !enabled() {
        return;
    }
    let (_dir, path) = pdf();
    let inspected = inspection(&path, &options("normal"), &Cancel::default()).unwrap();
    let cancel = Cancel::default();
    let spec = Spec {
        operation: records::Operation::Render,
        pages: vec![1],
        settings: RenderSettings::default(),
        input: "/in/document.pdf".into(),
    };
    // The final page waits for done, so trigger cancellation once the image header arrives.
    let result = execute(&path, &spec, &cancel, &options("mid-render"), |pipe| {
        let mut pipe = reader(pipe, true).unwrap();
        let mut first = [0; 1];
        std::io::Read::read_exact(&mut pipe, &mut first).unwrap();
        cancel.cancel();
        let input = std::io::Cursor::new(first).chain(pipe);
        let mut reader = records::RenderReader::new(input, inspected.pages).unwrap();
        let result = reader.next_page().map(|_| ());
        (result, reader.terminal_error())
    });
    assert_eq!(result, Err(Reason::Cancelled));
    // execute returns only after every pinned process is dead and the runner is reaped.
}
use std::io::Read;

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn parent_helper() {
    if std::env::var_os("MIMIK_PDF_PARENT").is_none() {
        return;
    }
    let (_dir, path) = pdf();
    let _ = inspection(&path, &options("sleep"), &Cancel::default());
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_parent_sigkill_takes_worker_with_it() {
    if !enabled() {
        return;
    }
    let mut parent = std::process::Command::new(std::env::current_exe().unwrap())
        .args([PARENT, "--exact", "--include-ignored", "--test-threads=1"])
        .env("MIMIK_PDF_PARENT", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let parent_pin = Watched::open(i32::try_from(parent.id()).unwrap()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut pins = Vec::new();
    while pins.len() < 3 && Instant::now() < deadline {
        let mut ppid = parent_pin.pid;
        pins.clear();
        for _ in 0..3 {
            let found = std::fs::read_dir("/proc")
                .unwrap()
                .filter_map(Result::ok)
                .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
                .find(|pid| crate::sandbox::process::stat_of(*pid).is_ok_and(|s| s.0 == ppid));
            if let Some(pid) = found
                && let Ok(pin) = Watched::open(pid)
            {
                ppid = pid;
                pins.push(pin);
            } else {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = rustix::process::pidfd_send_signal(&parent_pin.pidfd, rustix::process::Signal::KILL);
    parent.wait().unwrap();
    assert_eq!(
        pins.len(),
        3,
        "runner/reaper/worker must be running before killing parent"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while pins.iter().any(Watched::alive) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        pins.iter().all(|p| !p.alive()),
        "parent death left a live worker process"
    );
}
