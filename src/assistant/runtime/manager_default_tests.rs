//! The default model by device (ASSIST-55) through the manager: which model each start
//! selects, with a fake launcher and Tokio's paused clock.
use super::tests::{Behaviour, CONFIG, FakeLauncher, Start, ready};
use super::*;
use crate::assistant::model::{DEFAULT_MODEL_SHA256, GPU_DEFAULT_MODEL, GPU_DEFAULT_MODEL_SHA256};
use crate::assistant::records::tests::MemRecords;
use crate::assistant::runtime::models::GPU_DEFAULT_ON_CPU;

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
            older_prompt: crate::assistant::runtime::models::OlderPrompt::default(),
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
        Some("Qwen3.5-9B not found or not readable in /srv/models; using Granite 4.2 3B")
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

#[tokio::test(start_paused = true)]
async fn a_9b_file_with_the_wrong_hash_falls_back_to_granite_with_the_reason() {
    // The default 9B fails its hash check at the load: Granite runs instead, on the GPU path.
    let launcher = FakeLauncher::scripted([
        Start::Other("the model file changed: it no longer has the SHA-256 recorded for it"),
        Start::Ok(Behaviour::default()),
    ]);
    let (assistant, handle) = spawn_default(&launcher, GPU_CONFIG, true, None);
    ready(&assistant).await;
    let status = handle.status();
    assert_eq!(
        selected_shas(&launcher),
        [GPU_DEFAULT_MODEL_SHA256, DEFAULT_MODEL_SHA256]
    );
    assert_eq!(status.model.active.sha256, DEFAULT_MODEL_SHA256);
    assert_eq!(
        status.model.device_default.note(),
        Some("Qwen3.5-9B in /srv/models does not match its pinned hash; using Granite 4.2 3B")
    );
}

#[tokio::test(start_paused = true)]
async fn a_restart_keeps_the_9b_hash_mismatch_until_try_the_gpu_again() {
    let launcher = FakeLauncher::scripted([
        Start::Other("the model file changed: it no longer has the SHA-256 recorded for it"),
        Start::Ok(Behaviour::default()),
    ]);
    let (assistant, handle) = spawn_default(&launcher, GPU_CONFIG, true, None);
    ready(&assistant).await;
    let note = "Qwen3.5-9B in /srv/models does not match its pinned hash; using Granite 4.2 3B";
    assert_eq!(handle.status().model.device_default.note(), Some(note));
    assert_eq!(handle.restart(), Restart::Accepted);
    super::tests::until(Duration::from_mins(15), || {
        launcher.starts.lock().unwrap().len() >= 3
    })
    .await;
    ready(&assistant).await;
    let status = handle.status();
    assert_eq!(status.path, Path::Gpu);
    assert!(status.model.device_default.is_on());
    assert_eq!(status.model.active.sha256, DEFAULT_MODEL_SHA256);
    assert_eq!(status.model.device_default.note(), Some(note));
    assert_eq!(
        selected_shas(&launcher),
        [
            GPU_DEFAULT_MODEL_SHA256,
            DEFAULT_MODEL_SHA256,
            DEFAULT_MODEL_SHA256
        ]
    );
    // Only "Try the GPU again" clears the mismatch, even though the GPU never failed.
    assert_eq!(handle.try_gpu(), TryGpu::Accepted);
    super::tests::until(Duration::from_mins(15), || {
        launcher.starts.lock().unwrap().len() >= 4
    })
    .await;
    ready(&assistant).await;
    assert_eq!(
        handle.status().model.active.sha256,
        GPU_DEFAULT_MODEL_SHA256
    );
    assert_eq!(handle.status().model.device_default.note(), None);
}

#[tokio::test(start_paused = true)]
async fn a_9b_trial_while_the_device_default_is_on_runs_the_candidate_and_stores_the_choice() {
    for config in [CONFIG, GPU_CONFIG] {
        let launcher = FakeLauncher::scripted([]);
        launcher.files.lock().unwrap().insert(
            "Qwen3.5-9B-Q4_K_M.gguf".into(),
            Ok(ModelFile {
                sha256: GPU_DEFAULT_MODEL_SHA256.into(),
                size: 5_680_522_464,
            }),
        );
        let records = Arc::new(MemRecords::default());
        let (assistant, handle) =
            spawn_default(&launcher, config, true, Some(Arc::clone(&records)));
        ready(&assistant).await;
        let original = handle.status().model.active;
        assert!(handle.status().model.device_default.is_on());
        assert!(matches!(
            handle.try_model(7, GPU_DEFAULT_MODEL.into(), "Qwen3.5-9B-Q4_K_M.gguf".into()),
            TryModel::Accepted(_)
        ));
        super::tests::until(Duration::from_hours(1), || {
            handle.status().trial.is_some_and(|trial| !trial.running())
        })
        .await;
        ready(&assistant).await;
        let Some(TrialState::Passed(report)) = handle.status().trial else {
            panic!("{:?}", handle.status().trial)
        };
        assert_eq!(report.path, config.path);
        assert_eq!(report.model.sha256, GPU_DEFAULT_MODEL_SHA256);
        assert_eq!(report.model.max_tokens, 352);
        assert_eq!(
            selected_shas(&launcher),
            [
                original.sha256.clone(),
                GPU_DEFAULT_MODEL_SHA256.to_owned(),
                original.sha256.clone()
            ]
        );
        assert_eq!(handle.status().model.active, original);
        assert!(handle.status().model.device_default.is_on());
        assert!(records.log.lock().unwrap().is_empty());
        assert_eq!(handle.switch(7, &report.key), Chosen::Accepted);
        super::tests::until(Duration::from_mins(15), || {
            !handle.status().model.pending && handle.status().state == State::Ready
        })
        .await;
        let status = handle.status();
        assert_eq!(status.model.active, report.model);
        assert!(!status.model.device_default.is_on());
        assert_eq!(status.model.device_default.note(), None);
        assert!(status.model.from_owner_setting);
        assert_eq!(
            records
                .stored
                .lock()
                .unwrap()
                .active
                .as_ref()
                .unwrap()
                .model,
            report.model
        );
        assert_eq!(
            launcher.selected.lock().unwrap().last(),
            Some(&report.model)
        );
        handle.shutdown().await;
    }
}

#[tokio::test(start_paused = true)]
async fn an_explicit_9b_with_the_wrong_hash_stays_unavailable() {
    let launcher = FakeLauncher::scripted([Start::Other(
        "the model file changed: it no longer has the SHA-256 recorded for it",
    )]);
    let (_assistant, handle) = {
        let assistant = Assistant::new();
        let handle = spawn_with(
            Arc::clone(&assistant),
            Arc::clone(&launcher) as Arc<dyn Launcher>,
            GPU_CONFIG,
            ModelSetup {
                active: Model::test("Qwen3.5-9B-Q4_K_M.gguf", GPU_DEFAULT_MODEL_SHA256, 352),
                previous: None,
                records: None,
                from_owner_setting: false,
                device_default: false,
                older_prompt: crate::assistant::runtime::models::OlderPrompt::default(),
            },
        );
        (assistant, handle)
    };
    super::tests::until(Duration::from_mins(5), || {
        matches!(handle.status().state, State::Unavailable(_))
    })
    .await;
    assert_eq!(launcher.starts.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_restart_keeps_the_device_default_and_try_the_gpu_again_picks_the_9b_again() {
    let launcher = FakeLauncher::scripted([
        Start::Other("the model server exited before it was ready"),
        Start::Ok(Behaviour::default()),
        Start::Ok(Behaviour::default()),
        Start::Ok(Behaviour::default()),
    ]);
    let (assistant, handle) = spawn_default(&launcher, GPU_CONFIG, true, None);
    ready(&assistant).await;
    // On the CPU after the GPU failure: a Restart keeps Granite and the default.
    assert_eq!(handle.restart(), Restart::Accepted);
    super::tests::until(Duration::from_mins(15), || {
        launcher.starts.lock().unwrap().len() >= 3
    })
    .await;
    ready(&assistant).await;
    assert!(handle.status().model.device_default.is_on());
    assert_eq!(handle.status().model.active.sha256, DEFAULT_MODEL_SHA256);
    // "Try the GPU again": the 9B once more.
    assert_eq!(handle.try_gpu(), TryGpu::Accepted);
    super::tests::until(Duration::from_mins(15), || {
        launcher.starts.lock().unwrap().len() >= 4
    })
    .await;
    ready(&assistant).await;
    assert_eq!(handle.status().path, Path::Gpu);
    assert_eq!(
        handle.status().model.active.sha256,
        GPU_DEFAULT_MODEL_SHA256
    );
    assert_eq!(
        selected_shas(&launcher),
        [
            GPU_DEFAULT_MODEL_SHA256,
            DEFAULT_MODEL_SHA256,
            DEFAULT_MODEL_SHA256,
            GPU_DEFAULT_MODEL_SHA256
        ]
    );
}
