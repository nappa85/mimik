//! The start-time probe (assisted sheet plan §1.4a, "Preflight"): Mimik's own binary
//! re-executed under the full level profile (the same binds, descriptors, seccomp program and
//! token channel as a real start) exercises each resource once: it opens the model through
//! its descriptor path, reads the token channel and binds the socket (level 1) or the port
//! (level 3), reports one JSON line and answers one connection, through which the outer side
//! runs the endpoint check (on level 1 that asserts the process shape). It does not prove
//! llama-server's own system calls; the real start does.
//!
//! Also the `mimik assistant-runner` entry: `--spec-fd 3` (the runner), `--probe-inner
//! <level> …` (inside the level), and `--probe …` (the CI and owner check, which prints one
//! JSON line and exits 0 only at level 1).
use super::bwrap;
use super::endpoint;
use super::launch::{InnerCommand, Launch, Real, Settings};
use super::manager::Launcher;
use super::runner::{self, RunnerCommand};
use super::{Level, Path};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::time::Duration;

fn model_ok(path: &str) -> Result<(), String> {
    let mut magic = [0_u8; 4];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .map_err(|error| format!("the model cannot be opened: {}", error.kind()))?;
    if &magic == b"GGUF" {
        Ok(())
    } else {
        Err("the model is not a GGUF file".into())
    }
}

fn token_ok(path: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("the token cannot be read: {}", error.kind()))?;
    let line = text.strip_suffix('\n').unwrap_or(&text);
    if line.len() == 43 && !line.contains('\n') {
        Ok(())
    } else {
        Err("the token channel holds no token".into())
    }
}

fn verdict(result: &Result<(), String>) -> Value {
    match result {
        Ok(()) => json!("ok"),
        Err(why) => json!(why),
    }
}

/// A connection the probe answers.
trait Peer: Read + Write {
    fn blocking(&self);
}

impl Peer for std::os::unix::net::UnixStream {
    fn blocking(&self) {
        let _ = self.set_nonblocking(false);
        let _ = self.set_read_timeout(Some(Duration::from_secs(10)));
    }
}

impl Peer for std::net::TcpStream {
    fn blocking(&self) {
        let _ = self.set_nonblocking(false);
        let _ = self.set_read_timeout(Some(Duration::from_secs(10)));
    }
}

/// Accepts one connection within 10 s, answers it, and keeps its listener until the outer
/// side has closed it (so the endpoint check never sees a listener that is going away).
fn answer(accept: impl Fn() -> std::io::Result<Box<dyn Peer>>) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        match accept() {
            Ok(mut stream) => {
                stream.blocking();
                let _ = stream.write_all(b"ok\n");
                let mut rest = Vec::new();
                let _ = stream.take(4096).read_to_end(&mut rest);
                return;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return,
        }
    }
}

/// The probe's option naming the render node to open (the GPU path at level 1).
pub(crate) const RENDER: &str = "--render";

/// Opens the render node read-write, as the driver does.
fn render_ok(path: &str) -> Result<(), String> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map(|_| ())
        .map_err(|error| format!("the render node cannot be opened: {}", error.kind()))
}

/// The probe inside its level. Returns the exit code.
pub(crate) fn inner(args: &[String]) -> i32 {
    inner_to(args, &mut std::io::stdout().lock())
}

/// [`inner`], reporting to `out`.
fn inner_to(args: &[String], out: &mut dyn Write) -> i32 {
    let pid = std::process::id();
    match args {
        [level, rest @ ..] if level == "1" && matches!(rest, [] | [_, _]) => {
            let render = match rest {
                [flag, node] if flag == RENDER => Some(render_ok(node)),
                [] => None,
                _ => return 2,
            };
            let model = model_ok(bwrap::MODEL);
            let token = token_ok(bwrap::TOKEN);
            let socket = format!("{}/{}", bwrap::SOCKET_DIR, endpoint::SOCKET);
            let listener = std::os::unix::net::UnixListener::bind(&socket);
            let bound = listener
                .as_ref()
                .map(|_| ())
                .map_err(|error| format!("the socket cannot be bound: {}", error.kind()));
            let ok = model.is_ok()
                && token.is_ok()
                && bound.is_ok()
                && render.as_ref().is_none_or(Result::is_ok);
            let mut report = json!({"ok": ok, "model": verdict(&model), "token": verdict(&token),
                "socket": verdict(&bound), "pid": pid});
            if let Some(render) = &render {
                report["render"] = verdict(render);
            }
            let _ = writeln!(out, "{report}");
            let _ = out.flush();
            if ok && let Ok(listener) = listener {
                let _ = listener.set_nonblocking(true);
                answer(|| {
                    listener
                        .accept()
                        .map(|(stream, _)| Box::new(stream) as Box<dyn Peer>)
                });
            }
            i32::from(!ok)
        }
        [level, port, token] if level == "3" => {
            let model = model_ok("/proc/self/fd/8");
            let token = token_ok(token);
            let port: u16 = port.parse().unwrap_or(0);
            let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port));
            let bound = listener
                .as_ref()
                .map(|_| ())
                .map_err(|error| format!("the port cannot be bound: {}", error.kind()));
            let ok = model.is_ok() && token.is_ok() && bound.is_ok();
            // Only a port taken by someone else (after the launcher picked it) is worth
            // another try.
            let retry = model.is_ok()
                && token.is_ok()
                && listener
                    .as_ref()
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::AddrInUse);
            let _ = writeln!(
                out,
                "{}",
                json!({"ok": ok, "model": verdict(&model), "token": verdict(&token),
                    "port": verdict(&bound), "retry": retry, "pid": pid})
            );
            let _ = out.flush();
            if ok && let Ok(listener) = listener {
                let _ = listener.set_nonblocking(true);
                answer(|| {
                    listener
                        .accept()
                        .map(|(stream, _)| Box::new(stream) as Box<dyn Peer>)
                });
            }
            i32::from(!ok)
        }
        _ => 2,
    }
}

/// `--probe`'s options.
#[derive(Debug, Default)]
struct ProbeArgs {
    runtime: Option<String>,
    models: Option<String>,
    model: Option<String>,
    sha256: Option<String>,
    data: Option<String>,
    sandbox: Option<String>,
}

fn parse(args: &[String]) -> Result<ProbeArgs, String> {
    let mut parsed = ProbeArgs::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let value = iter
            .next()
            .cloned()
            .ok_or_else(|| format!("{arg} needs a value"))?;
        match arg.as_str() {
            "--runtime" => parsed.runtime = Some(value),
            "--models-dir" => parsed.models = Some(value),
            "--model" => parsed.model = Some(value),
            "--model-sha256" => parsed.sha256 = Some(value),
            "--data-dir" => parsed.data = Some(value),
            "--sandbox" => parsed.sandbox = Some(value),
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(parsed)
}

/// `mimik assistant-runner --probe …`: one JSON line, exit 0 only at level 1.
fn cli_probe(args: &[String]) -> i32 {
    let report = |value: Value| {
        println!("{value}");
    };
    let parsed = match parse(args) {
        Ok(parsed) => parsed,
        Err(why) => {
            report(json!({"level": null, "error": why}));
            return 2;
        }
    };
    let (Some(runtime), Some(models), Some(model)) = (parsed.runtime, parsed.models, parsed.model)
    else {
        report(json!({"level": null, "error": "--runtime, --models-dir and --model are required"}));
        return 2;
    };
    let Some(name) = model.strip_prefix("local:") else {
        report(json!({"level": null, "error": "--model must be local:<name>.gguf"}));
        return 2;
    };
    let sha256 = parsed
        .sha256
        .unwrap_or_else(|| crate::assistant::DEFAULT_MODEL_SHA256.to_owned());
    let data = parsed.data.map_or_else(
        || {
            std::path::PathBuf::from(&models)
                .join("..")
                .join("probe-data")
        },
        std::path::PathBuf::from,
    );
    let backend = match parsed.sandbox.as_deref().map(super::Backend::parse) {
        None => super::Backend::default(),
        Some(Ok(backend)) => backend,
        Some(Err(why)) => {
            report(json!({"level": null, "error": why}));
            return 2;
        }
    };
    let (Ok(runner), Ok(inner)) = (RunnerCommand::mimik(), InnerCommand::mimik()) else {
        report(json!({"level": null, "error": "Mimik's own binary cannot be found"}));
        return 2;
    };
    let settings = Settings {
        data_dir: data,
        models_dir: models.into(),
        runtime_dir: runtime.into(),
        model: name.to_owned(),
        model_sha256: sha256,
        alias: "probe".into(),
        threads: 1,
        ctx_size: 256,
        runner,
        inner,
        device: super::device::Choice::Cpu,
        roots: super::device::Roots::default(),
        backend,
    };
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return 2;
    };
    let launch = Launch(std::sync::Arc::new(Real::new(settings)));
    let result = runtime.block_on(async {
        if let Err(why) = launch.preflight(Path::Cpu).await {
            return json!({"level": null, "error": why});
        }
        probe_and_settle(&launch).await
    });
    let level_one = result["level"] == json!(Level::Bubblewrap.number());
    report(result);
    i32::from(!level_one)
}

/// Every level's probe, then a settle: the probes' cleanup (killing them, clearing their
/// pidfile entries) ends before the command exits.
pub(crate) async fn probe_and_settle(launch: &Launch) -> Value {
    let real = std::sync::Arc::clone(&launch.0);
    let ticket = real.work().enter();
    let levels = tokio::task::spawn_blocking(move || real.probe_levels(Path::Cpu, &ticket))
        .await
        .unwrap_or_default();
    launch.0.work().settle().await;
    probe_report(&levels)
}

/// `--probe`'s report: the level reached is the first that actually passed (a level kept
/// on inconclusive probes is not "ok").
pub(crate) fn probe_report(levels: &[(Level, super::launch::Probed)]) -> Value {
    use super::launch::Probed;
    // Only level 1 is ever used (ASSIST-43): the level reached is 1 when its probe passed, and
    // none otherwise, whatever level 3's probe says.
    let reached = levels
        .iter()
        .find(|(level, probed)| *level == Level::Bubblewrap && *probed == Probed::Passed)
        .map(|(level, _)| level.number());
    let detail: serde_json::Map<String, Value> = levels
        .iter()
        .map(|(level, probed)| {
            (level.number().to_string(), {
                let mut detail = match probed {
                    Probed::Passed => json!({"ok": true}),
                    Probed::Kept(why) => {
                        json!({"ok": false, "inconclusive": true, "reason": why})
                    }
                    Probed::Failed(why) => json!({"ok": false, "reason": why}),
                };
                if *level == Level::Unconfined {
                    detail["used"] = json!(false);
                    detail["note"] = json!("not used in this release (ASSIST-43)");
                }
                detail
            })
        })
        .collect();
    json!({"level": reached, "levels": detail, "path": "cpu",
        "llama_cpp": super::manifest::MANIFEST.llama_cpp.tag})
}

/// The variable that carries the shim's spec into the sandbox.
pub(crate) const SHIM_VARIABLE: &str = "MIMIK_SHIM";

/// What the shim executes, and where it reports that it reached the exec.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct ShimSpec {
    /// The inherited descriptor the marker is written to.
    pub marker: i32,
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// The marker written just before the exec.
pub(crate) const MARKER: &[u8] = b"exec\n";

/// Inside the level-1 sandbox, in place of the command: writes the marker on its
/// descriptor, closes every descriptor but 0, 1 and 2 (bwrap's status, seccomp, token and
/// directory descriptors included), then executes the command with exactly its environment.
/// The marker is the only proof that the sandbox was set up: whatever the command prints or
/// does later, a marked start is never a sandbox failure (it cannot unsend the marker).
pub(crate) fn shim() -> i32 {
    use std::os::unix::process::CommandExt;
    let Some(spec) = std::env::var(SHIM_VARIABLE)
        .ok()
        .and_then(|text| serde_json::from_str::<ShimSpec>(&text).ok())
    else {
        return 2;
    };
    if spec.argv.is_empty() {
        return 2;
    }
    // SAFETY: the marker descriptor is inherited and used only here, by number.
    let marker = unsafe { std::os::fd::BorrowedFd::borrow_raw(spec.marker) };
    if rustix::io::write(marker, MARKER).ok() != Some(MARKER.len()) {
        return 3;
    }
    if !close_inherited(close_range, fd_listing) {
        eprintln!("mimik shim: the inherited descriptors cannot be closed; not executing");
        return 4;
    }
    let error = std::process::Command::new(&spec.argv[0])
        .args(&spec.argv[1..])
        .env_clear()
        .envs(spec.env)
        .exec();
    // The marker went out already: this failure is counted at the same level, never a
    // sandbox failure.
    eprintln!("mimik shim: cannot execute {}: {error}", spec.argv[0]);
    127
}

/// `close_range(3, ~0, 0)`: every descriptor above the standard ones.
pub(crate) fn close_range() -> bool {
    // SAFETY: a plain system call closing descriptors this process no longer uses.
    unsafe { libc::syscall(libc::SYS_close_range, 3_u32, u32::MAX, 0_u32) == 0 }
}

/// The open descriptors, from `/proc/self/fd`.
pub(crate) fn fd_listing() -> std::io::Result<Vec<i32>> {
    descriptors(
        std::fs::read_dir("/proc/self/fd")?
            .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned())),
    )
}

/// Descriptor numbers from directory entry names: any error or other name fails the whole
/// listing (a partial one could leave a descriptor open).
pub(crate) fn descriptors(
    names: impl Iterator<Item = std::io::Result<String>>,
) -> std::io::Result<Vec<i32>> {
    names
        .map(|name| {
            name.and_then(|name| {
                name.parse()
                    .map_err(|_| std::io::Error::other("not a descriptor number"))
            })
        })
        .collect()
}

/// Closes every descriptor but 0, 1 and 2: `close_range`, else each one `/proc/self/fd`
/// lists. False when neither works: the shim then never executes anything.
pub(crate) fn close_inherited(
    close_range: impl FnOnce() -> bool,
    listing: impl FnOnce() -> std::io::Result<Vec<i32>>,
) -> bool {
    if close_range() {
        return true;
    }
    let Ok(listing) = listing() else {
        return false;
    };
    for fd in listing {
        if fd > 2 {
            // SAFETY: closing every inherited descriptor but the standard ones before the exec;
            // nothing in this process uses them afterwards.
            unsafe { libc::close(fd) };
        }
    }
    true
}

/// `mimik assistant-runner …`. Returns the exit code.
pub fn main(args: &[String]) -> i32 {
    match args {
        [flag, fd] if flag == "--spec-fd" => match fd.parse() {
            Ok(fd) if fd == runner::SPEC_FD => runner::run(fd),
            _ => runner::exit::SPEC,
        },
        [flag, rest @ ..] if flag == "--probe-inner" => inner(rest),
        [flag] if flag == "--shim" => shim(),
        [flag, rest @ ..] if flag == "--probe" => cli_probe(rest),
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cli_reports_level_1_only_when_its_probe_passed() {
        use super::super::launch::Probed;
        let kept = probe_report(&[
            (Level::Bubblewrap, Probed::Kept("no status line".into())),
            (Level::Landlock, Probed::Failed("not built".into())),
            (Level::Unconfined, Probed::Passed),
        ]);
        // Level 3 passing is reported, labelled not used; the level reached stays none.
        assert_eq!(kept["level"], Value::Null);
        assert_eq!(kept["levels"]["1"]["ok"], json!(false));
        assert_eq!(kept["levels"]["1"]["inconclusive"], json!(true));
        assert_eq!(kept["levels"]["3"]["ok"], json!(true));
        assert_eq!(kept["levels"]["3"]["used"], json!(false));
        let none = probe_report(&[(Level::Bubblewrap, Probed::Kept("timeout".into()))]);
        assert_eq!(none["level"], Value::Null);
        let passed = probe_report(&[(Level::Bubblewrap, Probed::Passed)]);
        assert_eq!(passed["level"], json!(1));
    }

    #[test]
    fn a_partial_descriptor_listing_is_no_listing() {
        let partial = [
            Ok("3".to_owned()),
            Err(std::io::Error::from_raw_os_error(libc::EIO)),
            Ok("7".to_owned()),
        ];
        assert!(descriptors(partial.into_iter()).is_err());
        assert!(descriptors([Ok("x".to_owned())].into_iter()).is_err());
        assert_eq!(
            descriptors([Ok("3".to_owned()), Ok("12".to_owned())].into_iter()).unwrap(),
            [3, 12]
        );
        // So the shim executes nothing.
        let listing = [
            Ok("3".to_owned()),
            Err(std::io::Error::from_raw_os_error(libc::EIO)),
        ];
        assert!(!close_inherited(
            || false,
            || descriptors(listing.into_iter())
        ));
    }

    #[test]
    fn a_shim_that_cannot_close_its_descriptors_never_executes() {
        assert!(!close_inherited(
            || false,
            || Err(std::io::Error::other("no /proc"))
        ));
        assert!(close_inherited(|| false, || Ok(Vec::new())));
        assert!(close_inherited(
            || true,
            || unreachable!("close_range worked")
        ));
    }

    #[test]
    fn outside_its_level_the_probe_names_every_missing_resource_and_fails() {
        // No sandbox here: no /models/model.gguf, no /run/token, no /run/llama.
        let mut out = Vec::new();
        assert_eq!(inner_to(&["1".into()], &mut out), 1);
        let report: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(report["ok"], json!(false));
        assert!(
            report["model"]
                .as_str()
                .unwrap()
                .starts_with("the model cannot be opened")
        );
        assert!(
            report["token"]
                .as_str()
                .unwrap()
                .starts_with("the token cannot be read")
        );
        assert!(
            report["socket"]
                .as_str()
                .unwrap()
                .starts_with("the socket cannot be bound")
        );
        let mut out = Vec::new();
        assert_eq!(
            inner_to(
                &["3".into(), "0".into(), "/nonexistent/token".into()],
                &mut out
            ),
            1
        );
        let report: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(report["ok"], json!(false));
        assert!(
            report["token"]
                .as_str()
                .unwrap()
                .starts_with("the token cannot be read")
        );
        assert_eq!(inner(&["2".into()]), 2);
    }

    #[test]
    fn on_the_gpu_path_the_probe_opens_the_render_node_read_write() {
        let mut out = Vec::new();
        assert_eq!(
            inner_to(&["1".into(), RENDER.into(), "/dev/null".into()], &mut out),
            1
        );
        let report: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(report["render"], json!("ok"));
        let mut out = Vec::new();
        inner_to(
            &["1".into(), RENDER.into(), "/nonexistent/renderD128".into()],
            &mut out,
        );
        let report: Value = serde_json::from_slice(&out).unwrap();
        assert!(
            report["render"]
                .as_str()
                .unwrap()
                .starts_with("the render node cannot be opened")
        );
        // The CPU path's report has no render entry; anything else is refused.
        let mut out = Vec::new();
        inner_to(&["1".into()], &mut out);
        let report: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(report.get("render"), None);
        assert_eq!(
            inner_to(&["1".into(), "--other".into(), "x".into()], &mut Vec::new()),
            2
        );
    }
}
