//! The 2c-1 development endpoint (assisted sheet plan §2 "2c-1", ASSIST-40), kept for tests
//! only: a llama-server started by hand on the dev host (or a fake), reached at a loopback IP
//! literal or a Unix socket path with a token file. Since 2c-2a it is primed and calibrated
//! by the runtime manager like a real child, so its budgets are measured, never placeholders
//! (ASSIST-L30). Compiled only with the `dev-endpoint` feature, which release builds never
//! enable; it has no endpoint check, and the page says so.
use super::AssistantHandle;
use super::model::{default_alias, max_tokens};
use super::runtime::manager::{self, Child, Fail, Launcher};
use super::runtime::server::{Llama, Server};
use super::runtime::{Level, Path};
use super::transport::{self, Bounded, Endpoint};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;

/// The development configuration, from the dev build's flags.
pub struct Config {
    /// `http://127.0.0.1:<port>`, `http://[::1]:<port>`, or an absolute socket path.
    pub endpoint: String,
    /// A file holding the server's API key (one line); never logged.
    pub token_file: PathBuf,
    pub model: String,
    pub model_sha256: Option<String>,
    pub model_alias: Option<String>,
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

/// The hand-started server as the manager's only "child": it never exits by itself and is
/// never stopped.
struct External {
    server: Arc<dyn Server>,
}

#[async_trait]
impl Child for External {
    fn server(&self) -> Arc<dyn Server> {
        Arc::clone(&self.server)
    }

    async fn exited(&self) -> Fail {
        std::future::pending().await
    }

    async fn stop(&self) {}

    fn notes(&self) -> Vec<String> {
        vec!["development endpoint: a hand-started server, no endpoint check".into()]
    }
}

struct Development {
    server: Arc<dyn Server>,
}

#[async_trait]
impl Launcher for Development {
    async fn preflight(&self, _: Path) -> Result<(), String> {
        Ok(())
    }

    async fn probe(&self, _: Path) -> Vec<(Level, Result<(), String>)> {
        vec![(Level::Unconfined, Ok(()))]
    }

    async fn start(&self, _: u64, _: Level, _: Path) -> Result<Arc<dyn Child>, Fail> {
        Ok(Arc::new(External {
            server: Arc::clone(&self.server),
        }))
    }
}

impl AssistantHandle {
    /// Starts the development assistant: the client for the hand-started server, its
    /// worker, and the runtime manager, which primes and calibrates it before it takes jobs
    /// (CPU path caps). Call from inside the Tokio runtime.
    ///
    /// # Errors
    ///
    /// A refused endpoint or model spec, or an unreadable token file, by message.
    pub fn development(config: Config) -> Result<Self, String> {
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
        let server: Arc<dyn Server> = Arc::new(Llama::new(
            Bounded::direct(endpoint(&config.endpoint)?),
            &token,
            &alias,
        ));
        Ok(Self::with_launcher(
            Arc::new(Development { server }),
            manager::Config {
                path: Path::Cpu,
                max_tokens,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
