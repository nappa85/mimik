//! The optional interpretation assistant (assisted sheet plan §1.4, §1.4a; steps 2c-1, 2c-2a).
//!
//! Off by default: without `--assistant` there is no client, no worker, no model server and
//! no job route. With it, the runtime manager starts the pinned llama-server as Mimik's
//! child (`runtime`), and jobs run against it once it is primed and calibrated.
pub(crate) mod breaker;
pub(crate) mod contract;
#[cfg(test)]
pub(crate) mod fake_server;
pub(crate) mod interpreter;
pub(crate) mod jobs;
pub(crate) mod lexer;
pub mod model;
pub(crate) mod output;
pub(crate) mod request;
pub(crate) mod runtime;
pub(crate) mod transport;

pub use model::{DEFAULT_MODEL, DEFAULT_MODEL_SHA256};
use std::path::PathBuf;
use std::sync::Arc;

/// The assistant as the binary hands it to the router: an opaque handle. Without one (the
/// default) there is no client, no worker and no job route.
#[derive(Clone)]
pub struct AssistantHandle {
    pub(crate) assistant: Arc<jobs::Assistant>,
    /// The runtime manager (absent only in route tests).
    pub(crate) runtime: Option<runtime::manager::Handle>,
}

/// The model runtime's configuration, from `--assistant-*` and the data directory.
#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    /// Mimik's data directory (the database's folder): `run/` lives there.
    pub data_dir: PathBuf,
    /// `--models-dir`, default `<data dir>/models`.
    pub models_dir: Option<PathBuf>,
    /// `--assistant-runtime`, default `llama/` beside Mimik's binary; either way it must hold
    /// exactly the pinned files.
    pub runtime_dir: Option<PathBuf>,
    /// `--assistant-model` (`local:<name>.gguf`).
    pub model: String,
    pub model_sha256: Option<String>,
    pub model_alias: Option<String>,
    /// `--assistant-threads`, default the physical cores minus one (ASSIST-41).
    pub threads: Option<u32>,
}

impl AssistantHandle {
    /// The post-commit note hook the store calls after every save that wrote notes.
    pub(crate) fn note_hook(&self) -> crate::store::NoteHook {
        let assistant = Arc::clone(&self.assistant);
        Arc::new(move |sheet, revision, addresses| {
            assistant.note_written(sheet, revision, addresses);
        })
    }

    /// The worker and a runtime manager over `launcher`. Call from inside the Tokio runtime.
    pub(crate) fn with_launcher(
        launcher: Arc<dyn runtime::manager::Launcher>,
        config: runtime::manager::Config,
    ) -> Self {
        let assistant = jobs::Assistant::new();
        tokio::spawn(Arc::clone(&assistant).work());
        let runtime = runtime::manager::spawn(Arc::clone(&assistant), launcher, config);
        Self {
            assistant,
            runtime: Some(runtime),
        }
    }

    /// Starts the assistant with the model runtime: the pinned llama-server as Mimik's child
    /// at the highest confinement level that works, primed and calibrated before it takes
    /// jobs. Call from inside the Tokio runtime.
    ///
    /// # Errors
    ///
    /// A refused model spec or thread count, or Mimik's own path unknown, by message.
    pub fn runtime(config: RuntimeConfig) -> Result<Self, String> {
        let name = model::local_name(&config.model)?.to_owned();
        let max_tokens = model::max_tokens(&config.model, config.model_sha256.as_deref())?;
        let sha256 = config
            .model_sha256
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL_SHA256.to_owned());
        let alias = match config.model_alias {
            Some(alias) => alias,
            None => model::default_alias(&config.model)?,
        };
        if config.threads == Some(0) {
            return Err("--assistant-threads must be at least 1".into());
        }
        let exe = std::env::current_exe().map_err(|_| "Mimik's own path is unknown".to_owned())?;
        let runtime_dir = match config.runtime_dir {
            Some(dir) => dir,
            None => exe
                .parent()
                .map(|dir| dir.join("llama"))
                .ok_or("Mimik's own folder is unknown")?,
        };
        let settings = runtime::launch::Settings {
            models_dir: config
                .models_dir
                .unwrap_or_else(|| config.data_dir.join("models")),
            data_dir: config.data_dir,
            runtime_dir,
            model: name,
            model_sha256: sha256,
            alias,
            threads: config
                .threads
                .unwrap_or_else(runtime::threads::default_threads),
            ctx_size: runtime::launch::CTX_SIZE,
            runner: runtime::runner::RunnerCommand::mimik()
                .map_err(|_| "Mimik's own path is unknown".to_owned())?,
            inner: runtime::launch::InnerCommand::mimik()
                .map_err(|_| "Mimik's own path is unknown".to_owned())?,
        };
        Ok(Self::with_launcher(
            Arc::new(runtime::launch::Launch(Arc::new(
                runtime::launch::Real::new(settings),
            ))),
            runtime::manager::Config {
                path: runtime::Path::Cpu,
                max_tokens,
                levels: runtime::manager::USED_LEVELS,
            },
        ))
    }

    /// Stops the model server (on Mimik's shutdown); resolves once it is gone.
    pub async fn shutdown(&self) {
        if let Some(runtime) = &self.runtime {
            runtime.shutdown().await;
        }
    }

    /// A ready assistant over `interpreter` with `budgets`, its worker running, no runtime
    /// manager (route tests).
    #[cfg(test)]
    pub(crate) fn ready_for_tests(
        interpreter: Arc<dyn interpreter::Interpreter>,
        budgets: jobs::Budgets,
    ) -> Self {
        let assistant = jobs::Assistant::new();
        assistant.event(jobs::RuntimeEvent::GenerationStarted { generation: 1 });
        assistant.event(jobs::RuntimeEvent::Ready {
            generation: 1,
            budgets,
            interpreter,
        });
        tokio::spawn(Arc::clone(&assistant).work());
        Self {
            assistant,
            runtime: None,
        }
    }
}

/// `mimik assistant-runner …` (hidden): the runner, the probe inside a level, or the probe
/// check. Returns the exit code. Called before any runtime or thread starts.
#[must_use]
pub fn runner_main(args: &[String]) -> i32 {
    runtime::probe::main(args)
}

#[cfg(feature = "dev-endpoint")]
pub mod development;
