//! The optional interpretation assistant (assisted sheet plan §1.4, §1.4a; steps 2c-1, 2c-2a).
//!
//! Off by default: without `--assistant` there is no client, no worker, no model server and
//! no job route. With it, the runtime manager starts the pinned llama-server as Mimik's
//! child (`runtime`), and jobs run against it once it is primed and calibrated.
pub(crate) mod breaker;
pub(crate) mod contract;
pub(crate) mod download;
#[cfg(test)]
pub(crate) mod fake_server;
pub(crate) mod interpreter;
pub(crate) mod jobs;
pub(crate) mod lexer;
pub mod model;
pub(crate) mod output;
pub(crate) mod records;
pub(crate) mod request;
pub(crate) mod runtime;
pub(crate) mod transport;

pub use model::{DEFAULT_MODEL, DEFAULT_MODEL_SHA256, GPU_DEFAULT_MODEL, GPU_DEFAULT_MODEL_SHA256};
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
    /// `--assistant-model` (`local:<name>.gguf`); `None` takes the default by device
    /// (ASSIST-55: the Qwen3.5-9B on the GPU path, Granite 4.2 3B on the CPU path).
    pub model: Option<String>,
    pub model_sha256: Option<String>,
    pub model_alias: Option<String>,
    /// `--assistant-threads`, default the physical cores minus one (ASSIST-41).
    pub threads: Option<u32>,
    /// `--assistant-device`: `auto` (the default: the best hardware GPU, else the CPU),
    /// `cpu`, or a GPU's PCI address.
    pub device: Option<String>,
}

impl AssistantHandle {
    /// The post-commit note hook the store calls after every save that wrote notes.
    pub(crate) fn note_hook(&self) -> crate::store::NoteHook {
        let assistant = Arc::clone(&self.assistant);
        Arc::new(move |sheet, revision, addresses| {
            assistant.note_written(sheet, revision, addresses);
        })
    }

    /// The worker and a runtime manager over `launcher` with a fixed model (tests). Call from
    /// inside the Tokio runtime.
    #[cfg(test)]
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

    /// [`Self::with_launcher`] with the owner's model setting.
    pub(crate) fn with_models(
        launcher: Arc<dyn runtime::manager::Launcher>,
        config: runtime::manager::Config,
        models: runtime::manager::ModelSetup,
    ) -> Self {
        let assistant = jobs::Assistant::new();
        tokio::spawn(Arc::clone(&assistant).work());
        let runtime =
            runtime::manager::spawn_with(Arc::clone(&assistant), launcher, config, models);
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
    /// A refused model spec or thread count, an unreadable model setting, or Mimik's own path
    /// unknown, by message.
    pub async fn runtime(config: RuntimeConfig, store: crate::Store) -> Result<Self, String> {
        let device = match config.device.as_deref() {
            None => runtime::device::Choice::Auto,
            Some(text) => runtime::device::Choice::parse(text)?,
        };
        let path = runtime_path(&device);
        // Without a start flag the model is the device default, picked per start by the
        // manager (ASSIST-55); the Granite spec stands in until then.
        let device_default = config.model.is_none();
        let spec = config
            .model
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
        if device_default && (config.model_sha256.is_some() || config.model_alias.is_some()) {
            return Err(
                "--assistant-model-sha256 and --assistant-model-alias need --assistant-model"
                    .into(),
            );
        }
        let name = model::local_name(&spec)?;
        let (sha256, max_tokens) = model::resolve(&spec, config.model_sha256.as_deref())?;
        let alias = match config.model_alias {
            Some(alias) => alias,
            None => model::default_alias(&spec)?,
        };
        if !model::valid_alias(&alias) {
            return Err(
                "--assistant-model-alias must be 1 to 128 characters from [A-Za-z0-9._-]".into(),
            );
        }
        let flags = runtime::models::Model {
            spec,
            name: name.clone(),
            sha256: sha256.clone(),
            alias: alias.clone(),
            max_tokens,
            size: None,
        };
        let records = records::SqlRecords(store);
        // The owner's stored setting beats the start flags.
        let stored = records.load().await?;
        let from_owner_setting = stored.active.is_some();
        let device_default = device_default && !from_owner_setting;
        let older_prompt = stored.older_prompt();
        let (active, previous) = match stored.active {
            Some(active) => (active.model, stored.previous.map(|record| record.model)),
            None => (flags, None),
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
        // Absolute, so the notes name the folder unambiguously.
        let models_dir = config
            .models_dir
            .unwrap_or_else(|| config.data_dir.join("models"));
        let models_dir = std::path::absolute(&models_dir).unwrap_or(models_dir);
        let settings = runtime::launch::Settings {
            models_dir,
            data_dir: config.data_dir,
            runtime_dir,
            model: active.name.clone(),
            model_sha256: active.sha256.clone(),
            alias: active.alias.clone(),
            threads: config
                .threads
                .unwrap_or_else(runtime::threads::default_threads),
            ctx_size: runtime::launch::CTX_SIZE,
            runner: runtime::runner::RunnerCommand::mimik()
                .map_err(|_| "Mimik's own path is unknown".to_owned())?,
            inner: runtime::launch::InnerCommand::mimik()
                .map_err(|_| "Mimik's own path is unknown".to_owned())?,
            device,
            roots: runtime::device::Roots::default(),
        };
        Ok(Self::with_models(
            Arc::new(runtime::launch::Launch(Arc::new(
                runtime::launch::Real::new(settings),
            ))),
            runtime::manager::Config {
                path,
                levels: runtime::manager::USED_LEVELS,
            },
            runtime::manager::ModelSetup {
                active,
                previous,
                records: Some(Arc::new(records)),
                from_owner_setting,
                device_default,
                older_prompt,
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

/// The path the runtime tries first: the GPU unless the CPU was chosen (ASSIST-27); with
/// `cpu` no device is ever listed.
pub(crate) fn runtime_path(device: &runtime::device::Choice) -> runtime::Path {
    match device {
        runtime::device::Choice::Cpu => runtime::Path::Cpu,
        runtime::device::Choice::Auto | runtime::device::Choice::Pci(_) => runtime::Path::Gpu,
    }
}

/// `mimik assistant-runner …` (hidden): the runner, the probe inside a level, or the probe
/// check. Returns the exit code. Called before any runtime or thread starts.
#[must_use]
pub fn runner_main(args: &[String]) -> i32 {
    runtime::probe::main(args)
}

/// `mimik assistant-apparmor-profile [--for <path>]`: prints the `AppArmor` profile for this
/// binary, or for `path` (the Hakoniwa sandbox needs it on Ubuntu 23.10 and later). Returns
/// the exit code.
#[must_use]
pub fn apparmor_profile_main(args: &[String]) -> i32 {
    match runtime::hako::apparmor_profile_for(args) {
        Ok(profile) => {
            print!("{profile}");
            0
        }
        Err(why) => {
            eprintln!("assistant-apparmor-profile: {why}");
            1
        }
    }
}

#[cfg(feature = "dev-endpoint")]
pub mod development;
