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

/// The probe inside its level. Returns the exit code.
pub(crate) fn inner(args: &[String]) -> i32 {
    let mut out = std::io::stdout().lock();
    let pid = std::process::id();
    match args {
        [level] if level == "1" => {
            let model = model_ok(bwrap::MODEL);
            let token = token_ok(bwrap::TOKEN);
            let socket = format!("{}/{}", bwrap::SOCKET_DIR, endpoint::SOCKET);
            let listener = std::os::unix::net::UnixListener::bind(&socket);
            let bound = listener
                .as_ref()
                .map(|_| ())
                .map_err(|error| format!("the socket cannot be bound: {}", error.kind()));
            let ok = model.is_ok() && token.is_ok() && bound.is_ok();
            let _ = writeln!(
                out,
                "{}",
                json!({"ok": ok, "model": verdict(&model), "token": verdict(&token),
                    "socket": verdict(&bound), "pid": pid})
            );
            let _ = out.flush();
            if let Ok(listener) = listener {
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
            let _ = writeln!(
                out,
                "{}",
                json!({"ok": ok, "model": verdict(&model), "token": verdict(&token),
                    "port": verdict(&bound), "pid": pid})
            );
            let _ = out.flush();
            if let Ok(listener) = listener {
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
        let levels = launch.probe(Path::Cpu).await;
        let reached = levels
            .iter()
            .find(|(_, result)| result.is_ok())
            .map(|(level, _)| level.number());
        let detail: serde_json::Map<String, Value> = levels
            .iter()
            .map(|(level, result)| {
                (
                    level.number().to_string(),
                    match result {
                        Ok(()) => json!({"ok": true}),
                        Err(why) => json!({"ok": false, "reason": why}),
                    },
                )
            })
            .collect();
        json!({"level": reached, "levels": detail, "path": "cpu",
            "llama_cpp": super::manifest::MANIFEST.llama_cpp.tag})
    });
    let level_one = result["level"] == json!(Level::Bubblewrap.number());
    report(result);
    i32::from(!level_one)
}

/// `mimik assistant-runner …`. Returns the exit code.
pub fn main(args: &[String]) -> i32 {
    match args {
        [flag, fd] if flag == "--spec-fd" => match fd.parse() {
            Ok(fd) if fd == runner::SPEC_FD => runner::run(fd),
            _ => runner::exit::SPEC,
        },
        [flag, rest @ ..] if flag == "--probe-inner" => inner(rest),
        [flag, rest @ ..] if flag == "--probe" => cli_probe(rest),
        _ => 2,
    }
}
