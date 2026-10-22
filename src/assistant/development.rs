//! The 2c-1 development endpoint (assisted sheet plan §2 "2c-1", ASSIST-40): a llama-server
//! started by hand on the dev host, reached at a loopback IP literal or a Unix socket path
//! with a token file, and fixed development budgets. Compiled only with the `dev-endpoint`
//! feature, which release builds never enable; it has no endpoint check.
use super::AssistantHandle;
use super::interpreter::RigInterpreter;
use super::jobs::{Assistant, Budgets, RuntimeEvent};
use super::request::Bounds;
use super::transport::{self, Bounded, Endpoint};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// The default model spec (§1.4b) and the SHA-256 compiled in for it: the Granite 4.2 3B
/// `Q4_K_M` file, equal to the Hub's LFS SHA-256 at revision `c40945d…`.
pub const DEFAULT_MODEL: &str = "local:granite-4.2-3b-Q4_K_M.gguf";
pub const DEFAULT_MODEL_SHA256: &str =
    "e0406663965846ae22a403456eb826ccce5f450840491f71952f18a7cb78e7d5";
/// `max_tokens` of the default file under the adopted prefix, by the 2b rule (round up to a
/// multiple of 32 of 1.25 × the largest `predicted_n` of a valid, untruncated reply, at most
/// 384) over the 2c-1 screen of that prefix (v2-short): largest 263, 1.25 × 263 = 328.75, so
/// 352.
const DEFAULT_MAX_TOKENS: u32 = 352;
/// Any other file until the 2d trial measures it (never below 256, §1.4b).
const OTHER_MAX_TOKENS: u32 = 256;

/// The development configuration, from the dev build's flags.
pub struct Config {
    /// `http://127.0.0.1:<port>`, `http://[::1]:<port>`, or an absolute socket path.
    pub endpoint: String,
    /// A file holding the server's API key (one line); never logged.
    pub token_file: PathBuf,
    pub model: String,
    pub model_sha256: Option<String>,
    pub model_alias: Option<String>,
    /// B, at most 4,096.
    pub bytes: usize,
    /// N(B).
    pub tokens: usize,
    /// R.
    pub deadline: Duration,
    /// D.
    pub queue: usize,
    /// Q.
    pub queue_wait: Duration,
}

/// The model's file name from a `local:` spec (the 2d-1 grammar's `local:` form).
fn local_name(spec: &str) -> Result<&str, String> {
    let name = spec
        .strip_prefix("local:")
        .ok_or("the model must be a local:<name>.gguf spec in this build")?;
    let valid = (1..=128).contains(&name.len())
        && name
            .rsplit_once('.')
            .is_some_and(|(_, extension)| extension == "gguf")
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if valid {
        Ok(name)
    } else {
        Err("the model spec is not local:<name>.gguf with [A-Za-z0-9._-] characters".into())
    }
}

/// The model alias derived from a spec: its file name without `.gguf`, lower case
/// (`granite-4.2-3b-q4_k_m` for the default).
///
/// # Errors
///
/// A spec that is not `local:<name>.gguf`, by message.
pub fn default_alias(spec: &str) -> Result<String, String> {
    let name = local_name(spec)?;
    Ok(name.trim_end_matches(".gguf").to_ascii_lowercase())
}

/// The model's `max_tokens`: 352 for the default file by its SHA-256, 256 otherwise.
fn max_tokens(spec: &str, sha256: Option<&str>) -> Result<u32, String> {
    local_name(spec)?;
    let sha256 = match sha256 {
        Some(sha)
            if sha.len() == 64 && sha.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) =>
        {
            sha
        }
        Some(_) => return Err("--assistant-model-sha256 must be 64 lowercase hex digits".into()),
        None if spec == DEFAULT_MODEL => DEFAULT_MODEL_SHA256,
        None => return Err("--assistant-model-sha256 is required for this model".into()),
    };
    Ok(if sha256 == DEFAULT_MODEL_SHA256 {
        DEFAULT_MAX_TOKENS
    } else {
        OTHER_MAX_TOKENS
    })
}

fn endpoint(raw: &str) -> Result<Endpoint, String> {
    if raw.starts_with("http") {
        return transport::tcp(raw, true)
            .map(Endpoint::Tcp)
            .map_err(|refused| format!("the development endpoint is refused ({refused:?})"));
    }
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        Ok(Endpoint::Unix(path))
    } else {
        Err("the development endpoint must be a loopback URL or an absolute socket path".into())
    }
}

impl AssistantHandle {
    /// Starts the development assistant: the client for the hand-started server, its
    /// worker, and one generation made ready at once with the development budgets
    /// (development placeholders, not evidence). Call from inside the Tokio runtime.
    ///
    /// # Errors
    ///
    /// A refused endpoint, model spec, budget or an unreadable token file, by message.
    pub fn development(config: Config) -> Result<Self, String> {
        if config.bytes == 0 || config.bytes > super::contract::MAX_DYNAMIC_BYTES {
            return Err("--assistant-dev-bytes must be from 1 to 4096".into());
        }
        if config.tokens == 0 || config.deadline.is_zero() || config.queue_wait.is_zero() {
            return Err("development budgets must be positive".into());
        }
        let max_tokens = max_tokens(&config.model, config.model_sha256.as_deref())?;
        let alias = match config.model_alias {
            Some(alias) => alias,
            None => default_alias(&config.model)?,
        };
        let token = std::fs::read_to_string(&config.token_file)
            .map_err(|_| "the token file cannot be read".to_owned())?
            .trim_end_matches(['\n', '\r'])
            .to_owned();
        if token.is_empty() || token.chars().any(char::is_control) {
            return Err("the token file must hold one non-empty line".into());
        }
        let interpreter =
            RigInterpreter::new(Bounded::direct(endpoint(&config.endpoint)?), token, alias);
        let assistant = Assistant::new();
        assistant.reset_breaker();
        assistant.event(RuntimeEvent::GenerationStarted { generation: 1 });
        assistant.event(RuntimeEvent::Ready {
            generation: 1,
            budgets: Budgets {
                bounds: Bounds {
                    bytes: config.bytes,
                    tokens: config.tokens,
                },
                run: config.deadline,
                depth: config.queue,
                queue_wait: config.queue_wait,
                max_tokens,
                busy_after_expiry: Duration::ZERO,
            },
            interpreter: Arc::new(interpreter),
        });
        tokio::spawn(Arc::clone(&assistant).work());
        Ok(Self(assistant))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_specs_aliases_and_max_tokens() {
        assert_eq!(
            default_alias(DEFAULT_MODEL).unwrap(),
            "granite-4.2-3b-q4_k_m"
        );
        assert_eq!(max_tokens(DEFAULT_MODEL, None), Ok(352));
        assert_eq!(
            max_tokens(DEFAULT_MODEL, Some(DEFAULT_MODEL_SHA256)),
            Ok(352)
        );
        // Whatever its name, another file is 256.
        assert_eq!(max_tokens(DEFAULT_MODEL, Some(&"a".repeat(64))), Ok(256));
        assert_eq!(
            max_tokens("local:other.gguf", Some(DEFAULT_MODEL_SHA256)),
            Ok(352)
        );
        assert!(max_tokens("local:other.gguf", None).is_err());
        assert!(max_tokens("local:other.gguf", Some("ABC")).is_err());
        for refused in [
            "hf:x/y/z.gguf@0",
            "local:.hidden.gguf",
            "local:a/b.gguf",
            "local:x.bin",
            "local:",
        ] {
            assert!(default_alias(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn endpoints() {
        assert!(matches!(
            endpoint("http://127.0.0.1:8080"),
            Ok(Endpoint::Tcp(_))
        ));
        assert!(matches!(
            endpoint("http://[::1]:8080"),
            Ok(Endpoint::Tcp(_))
        ));
        assert!(matches!(endpoint("/run/llama.sock"), Ok(Endpoint::Unix(_))));
        for refused in [
            "http://localhost:8080",
            "https://127.0.0.1:8080",
            "llama.sock",
            "http://10.0.0.1:80",
        ] {
            assert!(endpoint(refused).is_err(), "{refused}");
        }
    }
}
