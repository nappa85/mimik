//! Live CPU check of the prompt cache across prompts (plan 3c-1b): the pinned llama-server
//! b12110, started natively (ASSIST-40) with the launcher's own flag vector, answers an
//! interpretation, a reminder and an interpretation through Mimik's real client (the rig
//! adapter over the bounded transport), and the second interpretation's completion must carry
//! `cached_tokens` for at least 75 % of the prefix. Ignored in ordinary runs; run only as the
//! weekday rule allows (2 threads, bounded time) with:
//!
//! - `MIMIK_LIVE_CACHE_RUNTIME`: the pinned runtime folder (holding `llama/cpu/llama-server`);
//! - `MIMIK_LIVE_CACHE_MODEL`: the Granite 4.2 3B `Q4_K_M` file (its SHA-256 is checked).
//!
//! A missing variable fails the test. The server is killed by its PID when the test ends.
use super::*;
use crate::assistant::interpreter::{Completion, Interpreter, RequestKind, RigInterpreter};
use crate::assistant::model::DEFAULT_MODEL_SHA256;
use crate::assistant::transport::{Bounded, Endpoint};
use std::io::Read;
use std::process::{Child, Command, Stdio};

fn env(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("{name} is not set")))
}

/// Kills the server it holds, by PID, when dropped.
struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

const STEP: Duration = Duration::from_mins(15);

async fn step(interpreter: &RigInterpreter, completion: &Completion) -> (Completed, f64) {
    let started = std::time::Instant::now();
    let done = tokio::time::timeout(STEP, interpreter.complete(completion))
        .await
        .expect("a request over its bound")
        .expect("the request failed");
    (done, started.elapsed().as_secs_f64())
}

use crate::assistant::interpreter::Completed;

/// The file's SHA-256, as lowercase hex.
fn digest(path: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write;
    let mut hasher = Sha256::new();
    let mut file = std::fs::File::open(path).unwrap();
    let mut buffer = vec![0_u8; 1 << 20];
    loop {
        let n = file.read(&mut buffer).unwrap();
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    hasher
        .finalize()
        .iter()
        .fold(String::new(), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

#[test]
#[ignore = "live: pinned llama-server natively, Granite model (MIMIK_LIVE_CACHE_*), 2 threads, minutes"]
fn live_cpu_cache_restores_the_interpretation_prefix_after_a_reminder() {
    let runtime = env("MIMIK_LIVE_CACHE_RUNTIME");
    let model = env("MIMIK_LIVE_CACHE_MODEL");
    assert_eq!(
        digest(&model),
        DEFAULT_MODEL_SHA256,
        "not the pinned Granite file"
    );

    let dir = tempfile::tempdir().unwrap();
    let token_file = dir.path().join("token");
    std::fs::write(&token_file, "live-cache-dummy-key\n").unwrap();
    let port = {
        let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        probe.local_addr().unwrap().port()
    };
    let real = Real::new(Settings {
        data_dir: dir.path().join("data"),
        models_dir: model.parent().unwrap().to_owned(),
        runtime_dir: runtime.clone(),
        model: model.file_name().unwrap().to_string_lossy().into_owned(),
        model_sha256: DEFAULT_MODEL_SHA256.to_owned(),
        alias: "granite".into(),
        threads: 2,
        ctx_size: CTX_SIZE,
        runner: RunnerCommand::mimik().unwrap(),
        inner: InnerCommand::mimik().unwrap(),
        device: Choice::Cpu,
        roots: Roots::default(),
    });
    real.set_model_size(std::fs::metadata(&model).unwrap().len());
    let args = real.server_args(
        model.to_str().unwrap(),
        "127.0.0.1",
        Some(port),
        token_file.to_str().unwrap(),
        None,
    );
    println!("flags: {args:?}");
    let cpu = runtime.join("llama/cpu");
    let child = Command::new(cpu.join("llama-server"))
        .args(&args)
        .env_clear()
        .env("LD_LIBRARY_PATH", &cpu)
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            std::fs::File::create(dir.path().join("server.log")).unwrap(),
        ))
        .stderr(Stdio::from(
            std::fs::File::create(dir.path().join("server.err")).unwrap(),
        ))
        .spawn()
        .unwrap();
    println!("server pid {}", child.id());
    let mut server = Server(child);

    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(exchange(port, &mut server));
}

async fn exchange(port: u16, server: &mut Server) {
    {
        let url = url::Url::parse(&format!("http://127.0.0.1:{port}")).unwrap();
        let interpreter = RigInterpreter::new(
            Bounded::direct(Endpoint::Tcp(url.clone())),
            "live-cache-dummy-key".into(),
            "granite".into(),
        );
        // Ready: the model loads from the slow disk.
        let client = Bounded::direct(Endpoint::Tcp(url));
        let health = format!("{}/health", client.base());
        let mut up = false;
        for _ in 0..240 {
            if let Ok(Some(status)) = server.0.try_wait() {
                panic!("the server exited early: {status}");
            }
            if let Ok(Ok(stream)) = tokio::time::timeout(
                Duration::from_secs(2),
                tokio::net::TcpStream::connect(("127.0.0.1", port)),
            )
            .await
            {
                drop(stream);
                up = true;
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        assert!(up, "the server did not listen ({health})");
        for _ in 0..240 {
            if interpreter.tokens("ready").await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }

        let interpretation = Completion {
            dynamic: crate::assistant::runtime::calibration::priming_envelope(),
            ref_ids: Vec::new(),
            max_tokens: 16,
            kind: RequestKind::Interpretation,
        };
        let reminder = Completion {
            dynamic: r#"{"decrease":{"id":"hp","label":"Hit points","before":10,"after":5,"phrase":"took 5 damage"},"fields":[],"rows":[{"id":"r1","list":"items","name":"Antidote"}]}"#.into(),
            ref_ids: vec!["hp".into()],
            max_tokens: 16,
            kind: RequestKind::Reminder {
                rows: vec!["r1".into()],
                targets: vec!["hp".into()],
            },
        };
        let (first, t1) = step(&interpreter, &interpretation).await;
        println!("interpretation 1: {first:?} in {t1:.1} s");
        let (second, t2) = step(&interpreter, &reminder).await;
        println!("reminder: {second:?} in {t2:.1} s");
        let (third, t3) = step(&interpreter, &interpretation).await;
        println!("interpretation 2: {third:?} in {t3:.1} s");
        let prefix = first
            .prompt_tokens
            .expect("prompt tokens of the first request");
        let cached = third
            .cached_tokens
            .expect("cached_tokens did not arrive through rig");
        println!("prefix {prefix} tokens, cached {cached}");
        #[expect(clippy::cast_precision_loss, reason = "token counts of a few thousand")]
        let enough = cached as f64 >= 0.75 * prefix as f64;
        assert!(
            enough,
            "{cached} of {prefix} prefix tokens read from the cache"
        );
    }
}
