//! The optional interpretation assistant (assisted sheet plan §1.4, §1.4a; steps 2c-1, 2c-2a).
//!
//! Off by default: without `--assistant` there is no client, no worker, no model server and
//! no job route. With it, the runtime manager starts the pinned llama-server as Mimik's
//! child (`runtime`), and jobs run against it once it is primed and calibrated.
pub(crate) mod breaker;
pub(crate) mod contract;
pub(crate) mod download;
pub(crate) mod download_rows;
pub(crate) mod downloads;
#[cfg(test)]
pub(crate) mod fake_server;
pub(crate) mod interpreter;
pub(crate) mod jobs;
pub(crate) mod lexer;
pub mod model;
pub(crate) mod output;
pub(crate) mod proposal;
pub(crate) mod records;
pub(crate) mod reminder;
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
    /// The model downloads (step 2d-2); absent with the development endpoint, in most route
    /// tests, or when they could not start (`downloads_note` says why).
    pub(crate) downloads: Option<Arc<downloads::Downloads>>,
    pub(crate) downloads_note: Option<String>,
    /// The model server's threads, as the page states them (`None` where none is started).
    pub(crate) threads: Option<u32>,
    /// Where the reminder setting is stored when the runtime keeps no records (the
    /// development endpoint); `None` means the runtime's own.
    pub(crate) reminder_records: Option<Arc<dyn records::Records>>,
    /// Held across storing the reminder setting and setting it in memory, so two owner posts
    /// cannot leave the memory with the older choice.
    pub(crate) reminder_lock: Arc<tokio::sync::Mutex<()>>,
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
    /// Attaches the model downloads, or the reason they could not start.
    fn attach(&mut self, downloads: Result<downloads::Downloads, String>) {
        match downloads {
            Ok(downloads) => self.downloads = Some(Arc::new(downloads)),
            Err(why) => self.downloads_note = Some(why),
        }
    }

    /// This handle with the model downloads attached.
    #[cfg(test)]
    pub(crate) fn with_downloads(mut self, downloads: Arc<downloads::Downloads>) -> Self {
        self.downloads = Some(downloads);
        self
    }

    /// This handle with the owner's stored reminder choice read from `store` and the setting
    /// stored there from now on (the development endpoint keeps no other records).
    ///
    /// # Errors
    ///
    /// The stored setting cannot be read.
    pub async fn with_stored_reminders(mut self, store: crate::Store) -> Result<Self, String> {
        let records = records::SqlRecords(store);
        let choice = records::Records::reminders(&records).await?;
        self.assistant.set_reminder_choice(choice);
        self.reminder_records = Some(Arc::new(records));
        Ok(self)
    }

    /// Stores the owner's reminder choice and sets it in memory, one at a time; no restart
    /// follows.
    ///
    /// # Errors
    ///
    /// Nothing is stored: the records refused, or there are none.
    pub(crate) async fn store_reminders(&self, by: i64, enabled: bool) -> Result<(), String> {
        let _held = self.reminder_lock.lock().await;
        match (&self.reminder_records, &self.runtime) {
            (Some(records), _) => records.set_reminders(enabled, by).await?,
            (None, Some(runtime)) => runtime.set_reminders(by, enabled).await?,
            (None, None) => return Err("this assistant keeps no settings".to_owned()),
        }
        self.assistant.set_reminder_choice(Some(enabled));
        Ok(())
    }

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
            downloads: None,
            downloads_note: None,
            threads: None,
            reminder_records: None,
            reminder_lock: Arc::default(),
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
            downloads: None,
            downloads_note: None,
            threads: None,
            reminder_records: None,
            reminder_lock: Arc::default(),
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
        let downloads = downloads::Downloads::start(&downloads_dir(&config), store.clone()).await;
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
        let records = records::SqlRecords(store);
        let flags = flags_model(spec, &config, &records).await?;
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
        let threads = settings.threads;
        let reminder_choice = records::Records::reminders(&records).await?;
        let mut handle = Self::with_models(
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
        );
        handle.attach(downloads);
        handle.threads = Some(threads);
        handle.assistant.set_reminder_choice(reminder_choice);
        Ok(handle)
    }

    /// Stops the model server and any download (on Mimik's shutdown); resolves once they are
    /// gone. A partial download is kept.
    pub async fn shutdown(&self) {
        if let Some(downloads) = &self.downloads {
            downloads.downloader.shutdown().await;
        }
        if let Some(runtime) = &self.runtime {
            runtime.shutdown().await;
        }
    }

    /// A ready assistant over `interpreter` with `budgets`, its worker running, no runtime
    /// manager (route tests), with reminders available (and switched on) when `reminders`.
    #[cfg(test)]
    pub(crate) fn ready_for_tests_with(
        interpreter: Arc<dyn interpreter::Interpreter>,
        budgets: jobs::Budgets,
        reminders: bool,
    ) -> Self {
        let allowed = if reminders {
            jobs::Reminders::Available(runtime::reminder_budgets::ReminderBudgets {
                run: std::time::Duration::from_secs(60),
                estimate_s: 40.0,
                busy: std::time::Duration::from_secs(20),
                restore: runtime::reminder_budgets::Restore {
                    seconds: 0.1,
                    dev_host_constant: true,
                },
                reprime: std::time::Duration::from_secs(9),
                reply_tokens: 500,
                prefix_tokens: 1500,
                extra_wait: std::time::Duration::from_secs(21),
            })
        } else {
            jobs::Reminders::Unavailable("test".into())
        };
        let assistant = jobs::Assistant::new();
        assistant.event(jobs::RuntimeEvent::GenerationStarted { generation: 1 });
        assistant.event(jobs::RuntimeEvent::Ready {
            generation: 1,
            budgets,
            interpreter,
            path: runtime::Path::Cpu,
            reminders: allowed,
        });
        assistant.set_reminder_choice(reminders.then_some(true));
        tokio::spawn(Arc::clone(&assistant).work());
        Self {
            assistant,
            runtime: None,
            downloads: None,
            downloads_note: None,
            threads: None,
            reminder_records: None,
            reminder_lock: Arc::default(),
        }
    }
}

/// The model the start flags name (or the Granite placeholder of the device default): the
/// spec's file, its hash and alias, with the reply length stored for the file's hash, else the
/// default.
async fn flags_model(
    spec: String,
    config: &RuntimeConfig,
    records: &records::SqlRecords,
) -> Result<runtime::models::Model, String> {
    let name = model::local_name(&spec)?;
    let (sha256, max_tokens) = model::resolve(&spec, config.model_sha256.as_deref())?;
    let alias = match &config.model_alias {
        Some(alias) => alias.clone(),
        None => model::default_alias(&spec)?,
    };
    if !model::valid_alias(&alias) {
        return Err(
            "--assistant-model-alias must be 1 to 128 characters from [A-Za-z0-9._-]".into(),
        );
    }
    let max_tokens = records::Records::length(records, &sha256)
        .await?
        .unwrap_or(max_tokens);
    Ok(runtime::models::Model {
        spec,
        name,
        sha256,
        alias,
        max_tokens,
        size: None,
    })
}

/// The models folder the downloads fill: `--models-dir`, else `<data dir>/models`, absolute.
fn downloads_dir(config: &RuntimeConfig) -> PathBuf {
    let dir = config
        .models_dir
        .clone()
        .unwrap_or_else(|| config.data_dir.join("models"));
    std::path::absolute(&dir).unwrap_or(dir)
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
