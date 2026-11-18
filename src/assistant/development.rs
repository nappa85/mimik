//! The 2c-1 development endpoint (assisted sheet plan §2 "2c-1", ASSIST-40), kept for tests
//! only: a llama-server started by hand on the dev host (or a fake), reached at a loopback IP
//! literal or a Unix socket path with a token file. Since 2c-2a it is primed and calibrated
//! by the runtime manager like a real child, so its budgets are measured, never placeholders.
//! Compiled only with the `dev-endpoint` feature, which release builds never enable; it has no
//! endpoint check, and the page says so.
use super::AssistantHandle;
use super::model::{default_alias, local_name, resolve};
use super::runtime::manager::{self, Child, Fail, Launcher};
use super::runtime::models::Model;
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
            .map_err(|refused| {
                format!("the development endpoint is refused: {}", refused.reason())
            });
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
        let (sha256, max_tokens) = resolve(&config.model, config.model_sha256.as_deref())?;
        let name = local_name(&config.model)?;
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
        // The model the hand-started server runs, by the flags; the owner cannot change it.
        let model = Model {
            spec: config.model,
            name,
            sha256,
            alias,
            max_tokens,
            size: None,
        };
        Ok(Self::with_models(
            Arc::new(Development { server }),
            manager::Config {
                path: Path::Cpu,
                // A hand-started external server, reported as level 3; nothing is started.
                levels: &[Level::Unconfined],
            },
            manager::ModelSetup {
                active: model,
                previous: None,
                records: None,
                from_owner_setting: false,
                device_default: false,
                older_prompt: crate::assistant::runtime::models::OlderPrompt::default(),
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

#[cfg(test)]
mod resolution_tests {
    use super::*;

    #[tokio::test]
    async fn the_9b_spec_gets_its_own_hash_not_granites() {
        let dir = tempfile::tempdir().unwrap();
        let token_file = dir.path().join("token");
        std::fs::write(&token_file, "dummy-token\n").unwrap();
        let handle = AssistantHandle::development(Config {
            endpoint: "http://127.0.0.1:9".into(),
            token_file,
            model: crate::assistant::GPU_DEFAULT_MODEL.into(),
            model_sha256: None,
            model_alias: None,
        })
        .unwrap();
        let active = handle.runtime.clone().unwrap().status().model.active;
        assert_eq!(active.sha256, crate::assistant::GPU_DEFAULT_MODEL_SHA256);
        assert_eq!(active.max_tokens, 1024);
    }
}

#[cfg(test)]
mod reminder_setting_tests {
    use super::*;

    /// The development endpoint reads the owner's stored reminder choice from the database and
    /// stores a new one through the same records (the browser check turns reminders on that
    /// way), under one lock held across the store and the memory.
    #[tokio::test]
    async fn the_development_endpoint_reads_and_stores_the_reminder_choice() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::Store::open(&dir.path().join("mimik.sqlite3")).unwrap();
        store
            .run(|db| {
                db.execute(
                    "INSERT INTO assistant_reminders(id,enabled,changed_by,changed_at) VALUES(1,1,1,1)",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let token_file = dir.path().join("token");
        std::fs::write(&token_file, "dummy-token\n").unwrap();
        let handle = AssistantHandle::development(Config {
            endpoint: "http://127.0.0.1:9".into(),
            token_file,
            model: crate::assistant::DEFAULT_MODEL.into(),
            model_sha256: None,
            model_alias: None,
        })
        .unwrap();
        assert_eq!(handle.assistant.reminder_choice(), None);
        let handle = handle.with_stored_reminders(store.clone()).await.unwrap();
        assert_eq!(handle.assistant.reminder_choice(), Some(true));
        handle.store_reminders(1, false).await.unwrap();
        assert_eq!(handle.assistant.reminder_choice(), Some(false));
        let stored = crate::assistant::records::Records::reminders(
            &crate::assistant::records::SqlRecords(store),
        )
        .await;
        assert_eq!(stored, Ok(Some(false)));
    }
}
