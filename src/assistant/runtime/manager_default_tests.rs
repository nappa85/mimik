//! The default model by device (ASSIST-55) through the manager: which model each start
//! selects, with a fake launcher and Tokio's paused clock.
use super::tests::{Behaviour, CONFIG, FakeLauncher, Start, ready};
use super::*;
use crate::assistant::model::{DEFAULT_MODEL_SHA256, GPU_DEFAULT_MODEL, GPU_DEFAULT_MODEL_SHA256};
use crate::assistant::records::tests::MemRecords;
use crate::assistant::runtime::models::{GPU_DEFAULT_MISSING, GPU_DEFAULT_ON_CPU};

const GPU_CONFIG: Config = Config {
    path: Path::Gpu,
    levels: USED_LEVELS,
};

fn placeholder() -> Model {
    Model::test("granite-4.2-3b-Q4_K_M.gguf", DEFAULT_MODEL_SHA256, 352)
}

fn spawn_default(
    launcher: &Arc<FakeLauncher>,
    config: Config,
    device_default: bool,
    records: Option<Arc<MemRecords>>,
) -> (Arc<Assistant>, Handle) {
    let assistant = Assistant::new();
    let handle = spawn_with(
        Arc::clone(&assistant),
        Arc::clone(launcher) as Arc<dyn Launcher>,
        config,
        ModelSetup {
            active: placeholder(),
            previous: None,
            records: records.map(|records| records as Arc<dyn Records>),
            from_owner_setting: false,
            device_default,
        },
    );
    (assistant, handle)
}

fn selected_shas(launcher: &FakeLauncher) -> Vec<String> {
    launcher
        .selected
        .lock()
        .unwrap()
        .iter()
        .map(|model| model.sha256.clone())
        .collect()
}

#[tokio::test(start_paused = true)]
async fn the_gpu_path_runs_the_9b_when_its_file_is_there() {
    let launcher = FakeLauncher::scripted([]);
    let (assistant, handle) = spawn_default(&launcher, GPU_CONFIG, true, None);
    ready(&assistant).await;
    let status = handle.status();
    assert_eq!(status.path, Path::Gpu);
    assert_eq!(selected_shas(&launcher), [GPU_DEFAULT_MODEL_SHA256]);
    assert_eq!(status.model.active.spec, GPU_DEFAULT_MODEL);
    assert_eq!(status.model.active.max_tokens, 352);
    assert!(status.model.device_default.is_on());
    assert_eq!(status.model.device_default.note(), None);
}

#[tokio::test(start_paused = true)]
async fn a_missing_9b_file_means_granite_with_the_reason_on_the_page() {
    let launcher = FakeLauncher::scripted([]);
    launcher
        .absent
        .lock()
        .unwrap()
        .push("Qwen3.5-9B-Q4_K_M.gguf".into());
    let (assistant, handle) = spawn_default(&launcher, GPU_CONFIG, true, None);
    ready(&assistant).await;
    let status = handle.status();
    // Warned, not blocked: the GPU still serves, with Granite.
    assert_eq!(status.path, Path::Gpu);
    assert_eq!(selected_shas(&launcher), [DEFAULT_MODEL_SHA256]);
    assert_eq!(status.model.active.sha256, DEFAULT_MODEL_SHA256);
    assert_eq!(
        status.model.device_default.note(),
        Some(GPU_DEFAULT_MISSING)
    );
}

#[tokio::test(start_paused = true)]
async fn the_cpu_path_runs_granite_and_never_looks_for_the_9b() {
    let launcher = FakeLauncher::scripted([]);
    let (assistant, handle) = spawn_default(&launcher, CONFIG, true, None);
    ready(&assistant).await;
    let status = handle.status();
    assert_eq!(selected_shas(&launcher), [DEFAULT_MODEL_SHA256]);
    assert_eq!(status.model.active.sha256, DEFAULT_MODEL_SHA256);
    assert_eq!(status.model.device_default.note(), None);
}

#[tokio::test(start_paused = true)]
async fn a_gpu_start_of_the_9b_that_falls_back_runs_granite_on_the_cpu() {
    let launcher = FakeLauncher::scripted([
        Start::Other("the model server exited before it was ready"),
        Start::Ok(Behaviour::default()),
    ]);
    let (assistant, handle) = spawn_default(&launcher, GPU_CONFIG, true, None);
    ready(&assistant).await;
    let status = handle.status();
    assert_eq!(*launcher.paths.lock().unwrap(), [Path::Gpu, Path::Cpu]);
    assert_eq!(
        selected_shas(&launcher),
        [GPU_DEFAULT_MODEL_SHA256, DEFAULT_MODEL_SHA256]
    );
    assert_eq!(status.path, Path::Cpu);
    assert_eq!(status.model.active.sha256, DEFAULT_MODEL_SHA256);
    assert_eq!(status.model.device_default.note(), Some(GPU_DEFAULT_ON_CPU));
}

#[tokio::test(start_paused = true)]
async fn an_explicit_or_stored_model_is_never_replaced_by_the_default() {
    let launcher = FakeLauncher::scripted([]);
    // device_default false: the flag's or the stored model, whatever the device.
    let (assistant, handle) = spawn_default(&launcher, GPU_CONFIG, false, None);
    ready(&assistant).await;
    let status = handle.status();
    assert_eq!(selected_shas(&launcher), [DEFAULT_MODEL_SHA256]);
    assert!(!status.model.device_default.is_on());
    assert_eq!(status.model.device_default.note(), None);
}

#[tokio::test(start_paused = true)]
async fn after_a_switch_the_device_default_no_longer_applies_and_switch_back_returns_to_what_ran() {
    let launcher = FakeLauncher::scripted([]);
    launcher.files.lock().unwrap().insert(
        "other.gguf".into(),
        Ok(ModelFile {
            sha256: "b".repeat(64),
            size: 1 << 30,
        }),
    );
    let records = Arc::new(MemRecords::default());
    let (assistant, handle) = spawn_default(&launcher, GPU_CONFIG, true, Some(records));
    ready(&assistant).await;
    assert!(matches!(
        handle.try_model(7, "local:other.gguf".into(), "other.gguf".into()),
        TryModel::Accepted(_)
    ));
    let probe = handle.clone();
    super::tests::until(Duration::from_hours(1), move || {
        probe.status().trial.is_some_and(|trial| !trial.running())
    })
    .await;
    ready(&assistant).await;
    // The previous model is back after the trial: still the 9B default.
    assert_eq!(
        handle.status().model.active.sha256,
        GPU_DEFAULT_MODEL_SHA256
    );
    let Some(TrialState::Passed(report)) = handle.status().trial else {
        panic!("{:?}", handle.status().trial)
    };
    assert!(matches!(handle.switch(7, &report.key), Chosen::Accepted));
    let probe = handle.clone();
    super::tests::until(Duration::from_hours(1), move || {
        !probe.status().model.pending
    })
    .await;
    ready(&assistant).await;
    let status = handle.status();
    assert_eq!(status.model.active.name, "other.gguf");
    assert!(!status.model.device_default.is_on());
    assert_eq!(status.model.device_default.note(), None);
    // The previous model is the 9B that was running, whatever the default is.
    assert_eq!(
        status.model.previous.map(|model| model.sha256),
        Some(GPU_DEFAULT_MODEL_SHA256.to_owned())
    );
    assert!(matches!(handle.switch_back(7), Chosen::Accepted));
    let probe = handle.clone();
    super::tests::until(Duration::from_hours(1), move || {
        !probe.status().model.pending
    })
    .await;
    ready(&assistant).await;
    assert_eq!(
        handle.status().model.active.sha256,
        GPU_DEFAULT_MODEL_SHA256
    );
}
