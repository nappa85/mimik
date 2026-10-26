//! The model setting through the manager (`docs/model-runtime.md`, "Owner model setting"): the trial with
//! its bounds and Cancel, Switch, Keep and Switch back, with a fake launcher, fake children
//! and Tokio's paused clock.
use super::super::tests::{
    Behaviour, CONFIG, FakeLauncher, Fault, Start, ready, unavailable, until,
};
use super::super::{
    Assistant, Chosen, Config, Handle, Launcher, ModelSetup, Records, Restart, State, TryModel,
    spawn, spawn_with,
};
use super::*;
use crate::assistant::DEFAULT_MODEL_SHA256;
use crate::assistant::jobs::RuntimeState;
use crate::assistant::records::tests::MemRecords;
use crate::assistant::runtime::models::ModelFile;
use std::sync::atomic::Ordering;
use std::time::Duration;

fn sha(c: char) -> String {
    c.to_string().repeat(64)
}

fn file(sha256: &str) -> ModelFile {
    ModelFile {
        sha256: sha256.to_owned(),
        size: 1 << 30,
    }
}

/// Whether a measured token count is `expected`.
fn tokens_are(measured: f64, expected: u32) -> bool {
    (measured - f64::from(expected)).abs() < 1e-9
}

/// The default Granite file, as the manager starts with it.
fn granite() -> Model {
    Model::test("granite-4.2-3b-Q4_K_M.gguf", DEFAULT_MODEL_SHA256, 352)
}

struct Fixture {
    launcher: Arc<FakeLauncher>,
    assistant: Arc<Assistant>,
    handle: Handle,
    records: Arc<MemRecords>,
}

/// A manager over a fake launcher whose `other.gguf` hashes to `b…b`; the default Granite
/// file is active.
fn fixture(script: impl IntoIterator<Item = Start>, config: Config) -> Fixture {
    let launcher = FakeLauncher::scripted(script);
    launcher
        .files
        .lock()
        .unwrap()
        .insert("other.gguf".into(), Ok(file(&sha('b'))));
    launcher
        .files
        .lock()
        .unwrap()
        .insert("granite-4.2-3b-Q4_K_M.gguf".into(), Ok(file(&sha('c'))));
    let assistant = Assistant::new();
    let records = Arc::new(MemRecords::default());
    let handle = spawn_with(
        Arc::clone(&assistant),
        Arc::clone(&launcher) as Arc<dyn Launcher>,
        config,
        ModelSetup {
            active: granite(),
            previous: None,
            records: Some(Arc::clone(&records) as Arc<dyn Records>),
            from_owner_setting: false,
        },
    );
    Fixture {
        launcher,
        assistant,
        handle,
        records,
    }
}

fn gpu_config() -> Config {
    Config {
        path: Path::Gpu,
        ..CONFIG
    }
}

impl Fixture {
    /// Tries `other.gguf` once the first generation is ready; waits until the trial ends and
    /// the previous model is serving again.
    async fn trial(&self) -> TrialState {
        ready(&self.assistant).await;
        self.try_and_wait("other.gguf").await
    }

    async fn try_and_wait(&self, name: &str) -> TrialState {
        assert!(matches!(
            self.handle
                .try_model(7, format!("local:{name}"), name.to_owned()),
            TryModel::Accepted(_)
        ));
        let handle = self.handle.clone();
        until(Duration::from_hours(1), move || {
            handle.status().trial.is_some_and(|trial| !trial.running())
        })
        .await;
        ready(&self.assistant).await;
        self.handle.status().trial.unwrap()
    }
}

fn passed(trial: TrialState) -> TrialReport {
    match trial {
        TrialState::Passed(report) => *report,
        other => panic!("{other:?}"),
    }
}

fn failure(trial: TrialState) -> String {
    match trial {
        TrialState::Failed { why, .. } => why,
        other => panic!("{other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn a_passing_trial_measures_the_candidate_and_brings_the_previous_model_back() {
    let fixture = fixture([], CONFIG);
    let report = passed(fixture.trial().await);
    // Generation 1 served, 2 was the candidate, 3 is the previous model again.
    assert_eq!(
        report.key,
        TrialKey {
            number: 1,
            generation: 2,
            sha256: sha('b')
        }
    );
    assert_eq!(report.by, 7);
    assert_eq!(report.path, Path::Cpu);
    assert_eq!(report.phrases.len(), 5);
    assert_eq!(report.largest_predicted, 80);
    // 1.25 x 80 = 100, rounded up to 32 is 128, raised to the floor of 256; the calibration
    // generated exactly that many tokens per sample.
    assert_eq!(report.model.max_tokens, 256);
    assert!(
        report
            .calibration
            .samples
            .iter()
            .all(|s| tokens_are(s.predicted_n, 256))
    );
    assert_eq!(report.budgets.max_tokens, 256);
    assert!(report.budgets.field_refs, "any model but the default");
    assert_eq!(report.model.spec, "local:other.gguf");
    assert_eq!(report.model.alias, "other");
    assert_eq!(report.model.size, Some(1 << 30));
    // The candidate was started with the measurement cap and its recorded hash; the previous
    // model then started again, and serves with its own budgets.
    let selected = fixture.launcher.selected.lock().unwrap().clone();
    assert_eq!(selected.len(), 3);
    assert_eq!(
        (selected[1].sha256.clone(), selected[1].max_tokens),
        (sha('b'), 384)
    );
    assert_eq!(selected[0], granite());
    assert_eq!(selected[2], granite());
    assert_eq!(fixture.assistant.budgets().unwrap().max_tokens, 352);
    assert_eq!(fixture.handle.status().generation, 3);
    assert_eq!(fixture.handle.status().model.active, granite());
    // Two children ran (the candidate was stopped), never together.
    assert!(!fixture.launcher.coexisted.load(Ordering::SeqCst));
    assert_eq!(fixture.launcher.alive.load(Ordering::SeqCst), 1);
    assert_eq!(*fixture.launcher.hashed.lock().unwrap(), ["other.gguf"]);
    assert!(
        fixture.records.log.lock().unwrap().is_empty(),
        "nothing stored by a trial"
    );
}

#[tokio::test(start_paused = true)]
async fn players_see_starting_while_the_trial_runs() {
    let fixture = fixture([], CONFIG);
    ready(&fixture.assistant).await;
    assert!(matches!(
        fixture
            .handle
            .try_model(7, "local:other.gguf".into(), "other.gguf".into()),
        TryModel::Accepted(1)
    ));
    // The candidate is being measured: the assistant is starting, not ready.
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        matches!(
            handle.status().trial,
            Some(TrialState::Running {
                phase: "trial phrase 2 of 5",
                ..
            })
        )
    })
    .await;
    assert_eq!(fixture.assistant.state(), RuntimeState::Starting);
    assert_eq!(fixture.assistant.budgets(), None);
    assert!(matches!(fixture.handle.status().state, State::Starting(_)));
}

#[tokio::test(start_paused = true)]
async fn max_tokens_follows_the_largest_reply_with_the_floor_and_the_cap() {
    for (tokens, expected) in [
        ([60.0, 70.0, 80.0, 65.0, 75.0], 256),
        ([60.0, 70.0, 263.0, 65.0, 75.0], 352),
        ([60.0, 70.0, 281.0, 65.0, 75.0], 352),
        ([60.0, 70.0, 282.0, 65.0, 75.0], 384),
        ([60.0, 380.0, 80.0, 65.0, 75.0], 384),
    ] {
        let fixture = fixture(
            [
                Start::Ok(Behaviour::default()),
                Start::Ok(Behaviour {
                    phrase_tokens: tokens,
                    ..Behaviour::default()
                }),
            ],
            CONFIG,
        );
        let report = passed(fixture.trial().await);
        assert_eq!(report.model.max_tokens, expected, "{tokens:?}");
        // The calibration ran with that max_tokens.
        assert!(
            report
                .calibration
                .samples
                .iter()
                .all(|s| tokens_are(s.predicted_n, expected)),
            "{tokens:?}"
        );
        assert_eq!(report.budgets.max_tokens, expected);
        fixture.handle.shutdown().await;
    }
}

#[tokio::test(start_paused = true)]
async fn the_default_file_keeps_352_by_its_hash_and_a_file_with_its_name_does_not() {
    // The default Granite file under another name, found by its hash.
    let renamed = fixture([], CONFIG);
    renamed
        .launcher
        .files
        .lock()
        .unwrap()
        .insert("renamed.gguf".into(), Ok(file(DEFAULT_MODEL_SHA256)));
    ready(&renamed.assistant).await;
    let report = passed(renamed.try_and_wait("renamed.gguf").await);
    assert_eq!(report.model.max_tokens, 352);
    assert_eq!(report.largest_predicted, 80, "the phrases still ran");
    assert!(
        report
            .calibration
            .samples
            .iter()
            .all(|s| tokens_are(s.predicted_n, 352))
    );
    assert!(
        !report.budgets.field_refs,
        "the default file offers no field references"
    );
    renamed.handle.shutdown().await;

    // A different file with the default's name gets the measured value.
    let impostor = fixture([], CONFIG);
    impostor
        .launcher
        .files
        .lock()
        .unwrap()
        .insert("granite-4.2-3b-Q4_K_M.gguf".into(), Ok(file(&sha('d'))));
    ready(&impostor.assistant).await;
    let report = passed(impostor.try_and_wait("granite-4.2-3b-Q4_K_M.gguf").await);
    assert_eq!(report.model.max_tokens, 256);
    assert_eq!(report.model.sha256, sha('d'));
    assert!(report.budgets.field_refs);
    impostor.handle.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_truncated_or_invalid_reply_fails_the_trial_and_the_previous_model_restarts() {
    for (fault, expected) in [
        (Fault::TruncatedPhrase, "cut off at 384 tokens"),
        (Fault::InvalidPhrase, "does not follow the reply schema"),
    ] {
        let fixture = fixture(
            [
                Start::Ok(Behaviour::default()),
                Start::Ok(Behaviour {
                    fault,
                    ..Behaviour::default()
                }),
            ],
            CONFIG,
        );
        let why = failure(fixture.trial().await);
        assert!(why.contains(expected), "{why}");
        // The candidate is gone and the previous model serves again.
        assert_eq!(
            fixture.launcher.selected.lock().unwrap().last(),
            Some(&granite())
        );
        assert_eq!(fixture.assistant.state(), RuntimeState::Ready);
        assert_eq!(fixture.launcher.alive.load(Ordering::SeqCst), 1);
        assert!(!fixture.launcher.coexisted.load(Ordering::SeqCst));
        assert_eq!(fixture.handle.status().model.active, granite());
        fixture.handle.shutdown().await;
    }
}

#[tokio::test(start_paused = true)]
async fn a_model_too_slow_for_the_caps_fails_with_the_numbers() {
    let fixture = fixture(
        [
            Start::Ok(Behaviour::default()),
            // 0.4 s per generated token: 256 tokens take about 102 s, over the CPU path's 90 s.
            Start::Ok(Behaviour {
                g: 0.4,
                ..Behaviour::default()
            }),
        ],
        CONFIG,
    );
    let why = failure(fixture.trial().await);
    assert!(why.contains("too slow"), "{why}");
    assert!(why.contains("90 s"), "{why}");
    assert_eq!(fixture.assistant.state(), RuntimeState::Ready);
    assert_eq!(
        fixture.launcher.selected.lock().unwrap().last(),
        Some(&granite())
    );
}

#[tokio::test(start_paused = true)]
async fn a_phrase_past_its_deadline_stops_the_candidate_and_restores_the_previous_model() {
    for (config, seconds) in [(CONFIG, 180), (gpu_config(), 120)] {
        let fixture = fixture(
            [
                Start::Ok(Behaviour::default()),
                Start::Ok(Behaviour {
                    phrase_delay: Duration::from_secs(seconds + 5),
                    ..Behaviour::default()
                }),
            ],
            config,
        );
        let why = failure(fixture.trial().await);
        assert!(
            why.contains(&format!("took more than {seconds} s")),
            "{why}"
        );
        assert_eq!(
            fixture.launcher.selected.lock().unwrap().last(),
            Some(&granite())
        );
        assert_eq!(fixture.launcher.alive.load(Ordering::SeqCst), 1);
        assert!(!fixture.launcher.coexisted.load(Ordering::SeqCst));
        fixture.handle.shutdown().await;
    }
}

#[tokio::test(start_paused = true)]
async fn the_whole_trial_is_bounded_hashing_and_the_candidates_start_included() {
    // Hashing alone may not take the whole bound.
    let slow_disk = fixture([], CONFIG);
    *slow_disk.launcher.hash_delay.lock().unwrap() = Some(Duration::from_mins(40));
    let why = failure(slow_disk.trial().await);
    assert!(why.contains("longer than its 30 minutes"), "{why}");
    assert_eq!(slow_disk.assistant.state(), RuntimeState::Ready);
    slow_disk.handle.shutdown().await;

    // The GPU path's bound is 20 minutes, and a start that takes longer fails the same way.
    let slow_start = fixture([], gpu_config());
    ready(&slow_start.assistant).await;
    // Only the candidate's host check is slow; the previous model's own start is not.
    slow_start
        .launcher
        .preflight_delays
        .lock()
        .unwrap()
        .push_back(Duration::from_mins(25));
    let why = failure(slow_start.try_and_wait("other.gguf").await);
    assert!(why.contains("longer than its 20 minutes"), "{why}");
    assert_eq!(
        slow_start.launcher.selected.lock().unwrap().last(),
        Some(&granite())
    );
}

#[tokio::test(start_paused = true)]
async fn the_trial_runs_on_the_current_device_path() {
    let fixture = fixture([], gpu_config());
    let report = passed(fixture.trial().await);
    assert_eq!(report.path, Path::Gpu);
    // Session, candidate and the restored session all started on the GPU path.
    assert_eq!(
        *fixture.launcher.paths.lock().unwrap(),
        [Path::Gpu, Path::Gpu, Path::Gpu]
    );
}

#[tokio::test(start_paused = true)]
async fn the_owners_cancel_stops_the_candidate_and_restores_the_previous_model() {
    let fixture = fixture(
        [
            Start::Ok(Behaviour::default()),
            Start::Ok(Behaviour {
                phrase_delay: Duration::from_secs(100),
                ..Behaviour::default()
            }),
        ],
        CONFIG,
    );
    ready(&fixture.assistant).await;
    assert!(!fixture.handle.cancel_trial(), "no trial runs yet");
    fixture
        .handle
        .try_model(7, "local:other.gguf".into(), "other.gguf".into());
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        matches!(
            handle.status().trial,
            Some(TrialState::Running {
                phase: "trial phrase 1 of 5",
                ..
            })
        )
    })
    .await;
    assert!(fixture.handle.cancel_trial());
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        matches!(handle.status().trial, Some(TrialState::Cancelled { .. }))
    })
    .await;
    ready(&fixture.assistant).await;
    let Some(TrialState::Cancelled { why, .. }) = fixture.handle.status().trial else {
        panic!()
    };
    assert_eq!(why, "cancelled by the owner");
    assert_eq!(
        fixture.launcher.selected.lock().unwrap().last(),
        Some(&granite())
    );
    assert_eq!(fixture.launcher.alive.load(Ordering::SeqCst), 1);
    assert!(!fixture.launcher.coexisted.load(Ordering::SeqCst));
    // A late Cancel is not carried over into the next trial.
    assert!(!fixture.handle.cancel_trial());
    let report = passed(fixture.try_and_wait("other.gguf").await);
    assert_eq!(report.key.number, 2);
}

#[tokio::test(start_paused = true)]
async fn a_second_trial_while_one_runs_is_refused_and_switching_waits_for_it() {
    let fixture = fixture(
        [
            Start::Ok(Behaviour::default()),
            Start::Ok(Behaviour {
                phrase_delay: Duration::from_secs(100),
                ..Behaviour::default()
            }),
        ],
        CONFIG,
    );
    ready(&fixture.assistant).await;
    assert_eq!(
        fixture
            .handle
            .try_model(7, "local:other.gguf".into(), "other.gguf".into()),
        TryModel::Accepted(1)
    );
    assert_eq!(
        fixture
            .handle
            .try_model(7, "local:third.gguf".into(), "third.gguf".into()),
        TryModel::Busy
    );
    let key = TrialKey {
        number: 1,
        generation: 2,
        sha256: sha('b'),
    };
    assert_eq!(fixture.handle.switch(7, &key), Chosen::Busy);
    assert_eq!(fixture.handle.switch_back(7), Chosen::Busy);
    assert_eq!(fixture.handle.keep(), Chosen::Busy);
    // Only one file was ever hashed.
    let handle = fixture.handle.clone();
    until(Duration::from_secs(3600), move || {
        handle.status().trial.is_some_and(|trial| !trial.running())
    })
    .await;
    assert_eq!(*fixture.launcher.hashed.lock().unwrap(), ["other.gguf"]);
}

#[tokio::test(start_paused = true)]
async fn a_failed_hash_fails_the_trial_and_a_missing_file_says_so() {
    let fixture = fixture([], CONFIG);
    ready(&fixture.assistant).await;
    let why = failure(fixture.try_and_wait("nothing-here.gguf").await);
    assert!(why.contains("nothing-here.gguf is missing"), "{why}");
    // Nothing started for it: the session and the restored session only.
    assert_eq!(fixture.launcher.starts.lock().unwrap().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_restart_or_shutdown_during_a_trial_ends_it() {
    let slow = || {
        [
            Start::Ok(Behaviour::default()),
            Start::Ok(Behaviour {
                phrase_delay: Duration::from_secs(100),
                ..Behaviour::default()
            }),
        ]
    };
    let fixture = fixture(slow(), CONFIG);
    ready(&fixture.assistant).await;
    fixture
        .handle
        .try_model(7, "local:other.gguf".into(), "other.gguf".into());
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        matches!(
            handle.status().trial,
            Some(TrialState::Running {
                phase: "trial phrase 1 of 5",
                ..
            })
        )
    })
    .await;
    assert_eq!(fixture.handle.restart(), Restart::Accepted);
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        matches!(handle.status().trial, Some(TrialState::Cancelled { .. }))
    })
    .await;
    ready(&fixture.assistant).await;
    let Some(TrialState::Cancelled { why, .. }) = fixture.handle.status().trial else {
        panic!()
    };
    assert_eq!(why, "ended by a Restart");
    fixture.handle.shutdown().await;

    // Shutdown while the candidate runs stops it and the manager.
    let fixture = self::fixture(slow(), CONFIG);
    ready(&fixture.assistant).await;
    fixture
        .handle
        .try_model(7, "local:other.gguf".into(), "other.gguf".into());
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        matches!(
            handle.status().trial,
            Some(TrialState::Running {
                phase: "trial phrase 1 of 5",
                ..
            })
        )
    })
    .await;
    fixture.handle.shutdown().await;
    assert_eq!(fixture.launcher.alive.load(Ordering::SeqCst), 0);
    assert!(matches!(
        fixture.handle.status().trial,
        Some(TrialState::Cancelled { .. })
    ));
}

#[tokio::test(start_paused = true)]
async fn switch_stores_the_model_then_restarts_and_reprimes_with_it() {
    let fixture = fixture([], CONFIG);
    let report = passed(fixture.trial().await);
    let primed = fixture.launcher.primed.load(Ordering::SeqCst);
    assert_eq!(fixture.handle.switch(7, &report.key), Chosen::Accepted);
    // Stored first, with the owner's account.
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        handle.status().generation >= 4 && handle.status().state == State::Ready
    })
    .await;
    assert_eq!(
        *fixture.records.log.lock().unwrap(),
        ["activate local:other.gguf (previous local:granite-4.2-3b-Q4_K_M.gguf) by 7"]
    );
    let status = fixture.handle.status();
    assert_eq!(status.model.active, report.model);
    assert_eq!(status.model.previous, Some(granite()));
    assert!(status.model.from_owner_setting);
    assert_eq!(status.trial, None, "the switched trial is consumed");
    // The new model restarted and was primed and calibrated with its recorded max_tokens.
    assert_eq!(
        fixture.launcher.selected.lock().unwrap().last(),
        Some(&report.model)
    );
    assert_eq!(fixture.launcher.primed.load(Ordering::SeqCst), primed + 1);
    assert_eq!(fixture.assistant.budgets().unwrap().max_tokens, 256);
    assert!(fixture.assistant.budgets().unwrap().field_refs);
    // The stored trial is the record of what was measured.
    let stored = fixture.records.stored.lock().unwrap().clone();
    let trial: serde_json::Value =
        serde_json::from_str(stored.active.unwrap().trial.as_deref().unwrap()).unwrap();
    assert_eq!(trial["max_tokens"], 256);
    assert_eq!(trial["device"], "cpu");
    assert_eq!(trial["phrases"].as_array().unwrap().len(), 5);
    assert_eq!(trial["budgets"]["bytes"], 1024);
    assert_eq!(trial["samples"].as_array().unwrap().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn switch_with_a_stale_trial_is_refused_and_changes_nothing() {
    let fixture = fixture([], CONFIG);
    let report = passed(fixture.trial().await);
    let generation = fixture.handle.status().generation;
    for stale in [
        TrialKey {
            sha256: sha('c'),
            ..report.key.clone()
        },
        TrialKey {
            number: 99,
            ..report.key.clone()
        },
        TrialKey {
            generation: report.key.generation - 1,
            ..report.key.clone()
        },
        TrialKey {
            generation: report.key.generation + 1,
            ..report.key.clone()
        },
    ] {
        assert_eq!(fixture.handle.switch(7, &stale), Chosen::Stale, "{stale}");
    }
    tokio::time::sleep(Duration::from_mins(5)).await;
    assert_eq!(
        fixture.handle.status().generation,
        generation,
        "nothing restarted"
    );
    assert!(fixture.records.log.lock().unwrap().is_empty());
    assert_eq!(fixture.handle.status().model.active, granite());
    // An older trial's key is stale once a newer trial has finished.
    let newer = passed(fixture.try_and_wait("other.gguf").await);
    assert_eq!(newer.key.number, 2);
    assert_eq!(fixture.handle.switch(7, &report.key), Chosen::Stale);
    assert_eq!(fixture.handle.switch(7, &newer.key), Chosen::Accepted);
}

#[tokio::test(start_paused = true)]
async fn a_failed_trial_has_nothing_to_switch_to() {
    let fixture = fixture(
        [
            Start::Ok(Behaviour::default()),
            Start::Ok(Behaviour {
                fault: Fault::TruncatedPhrase,
                ..Behaviour::default()
            }),
        ],
        CONFIG,
    );
    failure(fixture.trial().await);
    let key = TrialKey {
        number: 1,
        generation: 2,
        sha256: sha('b'),
    };
    assert_eq!(fixture.handle.switch(7, &key), Chosen::Stale);
}

#[tokio::test(start_paused = true)]
async fn keep_dismisses_the_result_and_the_previous_model_keeps_serving() {
    let fixture = fixture([], CONFIG);
    let report = passed(fixture.trial().await);
    let generation = fixture.handle.status().generation;
    assert_eq!(fixture.handle.keep(), Chosen::Accepted);
    assert_eq!(fixture.handle.status().trial, None);
    assert_eq!(
        fixture.handle.keep(),
        Chosen::Stale,
        "nothing left to dismiss"
    );
    tokio::time::sleep(Duration::from_mins(5)).await;
    assert_eq!(fixture.handle.status().generation, generation);
    assert_eq!(fixture.assistant.state(), RuntimeState::Ready);
    // The dismissed trial cannot be switched to any more.
    assert_eq!(fixture.handle.switch(7, &report.key), Chosen::Stale);
}

#[tokio::test(start_paused = true)]
async fn switch_back_restores_the_previous_model_with_a_restart_and_a_priming() {
    let fixture = fixture([], CONFIG);
    assert_eq!(fixture.handle.switch_back(7), Chosen::NoPrevious);
    let report = passed(fixture.trial().await);
    fixture.handle.switch(7, &report.key);
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        handle.status().model.active.spec == "local:other.gguf"
            && handle.status().state == State::Ready
    })
    .await;
    let primed = fixture.launcher.primed.load(Ordering::SeqCst);
    let generation = fixture.handle.status().generation;
    assert_eq!(fixture.handle.switch_back(7), Chosen::Accepted);
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        handle.status().generation > generation && handle.status().state == State::Ready
    })
    .await;
    let status = fixture.handle.status();
    assert_eq!(status.model.active, granite());
    assert_eq!(status.model.previous, Some(report.model.clone()));
    assert_eq!(
        fixture.launcher.selected.lock().unwrap().last(),
        Some(&granite())
    );
    assert_eq!(fixture.launcher.primed.load(Ordering::SeqCst), primed + 1);
    assert_eq!(fixture.assistant.budgets().unwrap().max_tokens, 352);
    assert!(!fixture.assistant.budgets().unwrap().field_refs);
    assert_eq!(
        fixture.records.log.lock().unwrap().last().unwrap(),
        "swap by 7"
    );
    // And forward again.
    assert_eq!(fixture.handle.switch_back(7), Chosen::Accepted);
}

#[tokio::test(start_paused = true)]
async fn a_switch_that_cannot_be_stored_changes_nothing_and_says_why() {
    let fixture = fixture([], CONFIG);
    let report = passed(fixture.trial().await);
    *fixture.records.fail.lock().unwrap() = Some("disk full".into());
    fixture.handle.switch(7, &report.key);
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        handle.status().model.note.is_some() && handle.status().state == State::Ready
    })
    .await;
    let status = fixture.handle.status();
    assert!(status.model.note.unwrap().contains("disk full"));
    assert_eq!(status.model.active, granite());
    assert_eq!(
        fixture.launcher.selected.lock().unwrap().last(),
        Some(&granite())
    );
    // The result stays: the owner may try the switch again once storage works.
    assert!(matches!(status.trial, Some(TrialState::Passed(_))));
}

#[tokio::test(start_paused = true)]
async fn a_file_that_changed_after_its_trial_makes_the_assistant_unavailable_without_retries() {
    let fixture = fixture([], CONFIG);
    let report = passed(fixture.trial().await);
    // After the switch the launcher's hash check refuses the file.
    fixture.launcher.script.lock().unwrap().push_back(Start::Other(
        "the model file changed: it no longer has the SHA-256 recorded for it; try the model again from the Model field",
    ));
    fixture.handle.switch(7, &report.key);
    let why = unavailable(&fixture.handle).await;
    assert!(why.starts_with("the model file changed"), "{why}");
    let starts = fixture.launcher.starts.lock().unwrap().len();
    tokio::time::sleep(Duration::from_mins(30)).await;
    assert_eq!(
        fixture.launcher.starts.lock().unwrap().len(),
        starts,
        "no retry"
    );
    assert_eq!(fixture.handle.status().failures, 0);
    // The owner can still switch back, or try a model again.
    assert_eq!(fixture.handle.switch_back(7), Chosen::Accepted);
}

#[tokio::test(start_paused = true)]
async fn without_a_model_setting_the_owner_cannot_try_models() {
    let launcher = FakeLauncher::scripted([]);
    let assistant = Assistant::new();
    let handle = spawn(
        Arc::clone(&assistant),
        Arc::clone(&launcher) as Arc<dyn Launcher>,
        CONFIG,
    );
    ready(&assistant).await;
    assert_eq!(
        handle.try_model(1, "local:x.gguf".into(), "x.gguf".into()),
        TryModel::Unsupported
    );
    assert_eq!(handle.switch_back(1), Chosen::Busy);
    assert!(!handle.status().model.switchable);
}

#[tokio::test(start_paused = true)]
async fn the_default_model_serves_without_field_references_and_others_with() {
    let default = fixture([], CONFIG);
    ready(&default.assistant).await;
    assert!(!default.assistant.budgets().unwrap().field_refs);
    default.handle.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_cancel_right_after_try_cancels_that_trial_and_never_the_next_one() {
    let fixture = fixture([], CONFIG);
    ready(&fixture.assistant).await;
    // Cancel pressed before the manager has stopped the serving model for the trial.
    assert_eq!(
        fixture
            .handle
            .try_model(7, "local:other.gguf".into(), "other.gguf".into()),
        TryModel::Accepted(1)
    );
    assert!(fixture.handle.cancel_trial());
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        handle.status().trial.is_some_and(|trial| !trial.running())
    })
    .await;
    ready(&fixture.assistant).await;
    let Some(TrialState::Cancelled { why, .. }) = fixture.handle.status().trial else {
        panic!("{:?}", fixture.handle.status().trial)
    };
    assert_eq!(why, "cancelled by the owner");
    assert!(
        fixture.launcher.hashed.lock().unwrap().is_empty(),
        "the cancelled trial never touched the file"
    );
    // That Cancel was for trial 1: trial 2 runs to its end.
    let report = passed(fixture.try_and_wait("other.gguf").await);
    assert_eq!(report.key.number, 2);
}
