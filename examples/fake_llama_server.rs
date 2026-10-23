//! A fake llama-server for the runtime's tests (never shipped): it takes the pinned build's
//! flag vector, refuses a model that is not GGUF, serves `/health` without the token and
//! everything else only with it (from `--api-key-file`), on the Unix socket or the port it
//! is given, and answers priming, calibration samples and streamed jobs with synthetic
//! timings. `--alias fail-bind` makes it fail as a server that cannot bind its port does;
//! `--alias forge-bwrap` prints bubblewrap's error lines and exits, as a compromised server
//! trying to be restarted unconfined would. `--list-devices` prints the pinned build's
//! listing of one RX 6650 XT when a render node is bound under `/dev/dri` and `(none)`
//! otherwise; with `--n-gpu-layers 99` it prints the offload line (`--alias partial-offload`:
//! 30 of 41 layers).
//!
//! Plain blocking `std` (one thread per connection, HTTP/1.1, `Connection: close`): it runs
//! under the model server's seccomp program, which refuses the socket pair an async
//! runtime's signal driver would create.
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::{Arc, Mutex};

struct Fake {
    key: String,
    /// The last system prompt, standing in for the prompt cache.
    cached: Mutex<Option<String>>,
}

fn text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect(),
        _ => String::new(),
    }
}

fn respond(out: &mut dyn Write, status: &str, kind: &str, body: &str) {
    let _ = write!(
        out,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = out.flush();
}

fn completion(fake: &Fake, value: &Value) -> (String, &'static str) {
    let system = text(&value["messages"][0]["content"]);
    let user = text(&value["messages"][1]["content"]);
    let prefix = system.len().div_ceil(3);
    let hit = fake.cached.lock().unwrap().replace(system.clone()) == Some(system);
    let user_tokens = user.len().div_ceil(3) + 20;
    let (prompt_n, cache_n) = if hit {
        (user_tokens, prefix)
    } else {
        (prefix + user_tokens, 0)
    };
    let max_tokens = value["max_tokens"].as_u64().unwrap_or(1);
    let predicted = if value["ignore_eos"] == json!(true) {
        max_tokens
    } else {
        1
    };
    #[expect(clippy::cast_precision_loss, reason = "small synthetic counts")]
    let timings = json!({
        "cache_n": cache_n,
        "prompt_n": prompt_n,
        "prompt_ms": prompt_n as f64 * 0.2,
        "predicted_n": predicted,
        "predicted_ms": predicted as f64 * 0.5,
    });
    if value["stream"] == json!(true) {
        let content = r#"{"kind":"options","options":[]}"#;
        let chunk = json!({"object": "chat.completion.chunk",
            "choices": [{"index": 0, "delta": {"content": content}, "finish_reason": null}]});
        let stop = json!({"object": "chat.completion.chunk",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": prompt_n + cache_n, "completion_tokens": 8,
                "total_tokens": prompt_n + cache_n + 8},
            "timings": timings});
        return (
            format!("data: {chunk}\n\ndata: {stop}\n\ndata: [DONE]\n\n"),
            "text/event-stream",
        );
    }
    (
        json!({"choices": [{"index": 0, "message": {"role": "assistant", "content": "x"}}],
            "timings": timings})
        .to_string(),
        "application/json",
    )
}

fn serve<S: Read + Write>(fake: &Fake, stream: S) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let mut parts = line.split_whitespace();
    let (method, path) = (
        parts.next().unwrap_or("").to_owned(),
        parts.next().unwrap_or("").to_owned(),
    );
    let mut length = 0;
    let mut authorization = String::new();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
            break;
        }
        let (name, value) = header.split_once(':').unwrap_or(("", ""));
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.trim().parse().unwrap_or(0),
            "authorization" => value.trim().clone_into(&mut authorization),
            _ => {}
        }
    }
    let mut body = vec![0; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let out = reader.get_mut();
    if method == "GET" && path == "/health" {
        return respond(out, "200 OK", "application/json", r#"{"status":"ok"}"#);
    }
    if authorization.strip_prefix("Bearer ") != Some(fake.key.as_str()) {
        return respond(out, "401 Unauthorized", "application/json", "{}");
    }
    let value: Value = serde_json::from_slice(&body).unwrap_or_default();
    match path.as_str() {
        "/tokenize" => {
            let count = value["content"].as_str().map_or(0, str::len).div_ceil(3);
            respond(
                out,
                "200 OK",
                "application/json",
                &json!({"tokens": vec![1; count]}).to_string(),
            );
        }
        "/v1/chat/completions" => {
            let (body, kind) = completion(fake, &value);
            respond(out, "200 OK", kind, &body);
        }
        _ => respond(out, "404 Not Found", "application/json", "{}"),
    }
}

fn value_of(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|at| args.get(at + 1).cloned())
}

/// `--list-devices` as the pinned build prints it (`common_print_available_devices`).
fn list_devices() {
    let node = std::fs::read_dir("/dev/dri")
        .ok()
        .and_then(|mut entries| entries.next())
        .is_some();
    // A real driver takes a moment: the listing's sandbox outlives its parent's reads of the
    // sandbox's status pipe (bubblewrap writes the exit status to it as the last thing).
    std::thread::sleep(std::time::Duration::from_millis(300));
    println!("Available devices:");
    if node {
        println!("  Vulkan0: AMD Radeon RX 6650 XT (RADV NAVI23) (8176 MiB, 7912 MiB free)");
    } else {
        println!("  (none)");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--list-devices") {
        list_devices();
        return;
    }
    // Run by `cargo test` without arguments: nothing to do.
    let Some(model) = value_of(&args, "--model") else {
        return;
    };
    let mut magic = [0_u8; 4];
    let ok = std::fs::File::open(&model)
        .and_then(|mut file| file.read_exact(&mut magic))
        .is_ok();
    if !ok || &magic != b"GGUF" {
        eprintln!("srv    load_model: failed to load model, '{model}'");
        std::process::exit(1);
    }
    if value_of(&args, "--alias").as_deref() == Some("forge-bwrap") {
        // What a compromised server could print to look like a sandbox that failed to set up,
        // once it has been running a moment (so its start went through).
        std::thread::sleep(std::time::Duration::from_secs(1));
        eprintln!("bwrap: Can't mount proc on /newroot/proc: Operation not permitted");
        println!("bwrap: setup failed");
        std::process::exit(1);
    }
    if value_of(&args, "--n-gpu-layers").as_deref() == Some("99") {
        let layers = if value_of(&args, "--alias").as_deref() == Some("partial-offload") {
            30
        } else {
            41
        };
        eprintln!("load_tensors: offloaded {layers}/41 layers to GPU");
    }
    if value_of(&args, "--alias").as_deref() == Some("fail-bind") {
        eprintln!("srv    init: couldn't bind HTTP server socket, hostname: 127.0.0.1, port: 1");
        std::process::exit(1);
    }
    let key = value_of(&args, "--api-key-file")
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|key| key.trim_end().to_owned())
        .unwrap_or_default();
    let fake = Arc::new(Fake {
        key,
        cached: Mutex::new(None),
    });
    let host = value_of(&args, "--host").unwrap_or_default();
    if std::path::Path::new(&host)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("sock"))
    {
        let listener = std::os::unix::net::UnixListener::bind(&host).expect("the socket binds");
        eprintln!("srv  llama_server: listening on unix://{host}");
        for stream in listener.incoming().flatten() {
            let fake = Arc::clone(&fake);
            std::thread::spawn(move || serve(&fake, stream));
        }
    } else {
        let port: u16 = value_of(&args, "--port")
            .and_then(|port| port.parse().ok())
            .unwrap_or(0);
        let Ok(listener) = std::net::TcpListener::bind((host.as_str(), port)) else {
            eprintln!(
                "srv    init: couldn't bind HTTP server socket, hostname: {host}, port: {port}"
            );
            std::process::exit(1);
        };
        eprintln!("srv  llama_server: listening on http://{host}:{port}");
        for stream in listener.incoming().flatten() {
            let fake = Arc::clone(&fake);
            std::thread::spawn(move || serve(&fake, stream));
        }
    }
}
