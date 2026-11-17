//! The model setting through the manager (`docs/model-runtime.md`, "Owner model setting"): the trial with
//! its bounds and Cancel, Switch, Keep and Switch back, with a fake launcher, fake children
//! and Tokio's paused clock.
use super::super::tests::{
    Behaviour, CONFIG, FakeLauncher, Fault, Start, ready, unavailable, until,
};
use super::super::{
    Assistant, CancelTrial, Chosen, Config, Handle, Launcher, ModelSetup, Records, Restart, State,
    TryModel, spawn, spawn_with,
};
use super::*;
use crate::assistant::DEFAULT_MODEL_SHA256;
use crate::assistant::jobs::RuntimeState;
use crate::assistant::records::Stored;
use crate::assistant::records::tests::MemRecords;
use crate::assistant::runtime::models::{ModelFile, OlderPrompt};
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
    Model::test("granite-4.2-3b-Q4_K_M.gguf", DEFAULT_MODEL_SHA256, 1024)
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
    fixture_older(script, config, (false, false))
}

/// The same, with the stored trials of the active and the previous model flagged as
/// measured with an older prompt (ASSIST-56).
fn fixture_older(
    script: impl IntoIterator<Item = Start>,
    config: Config,
    older_prompt: (bool, bool),
) -> Fixture {
    let older_prompt = OlderPrompt {
        active: older_prompt.0,
        previous: older_prompt.1,
    };
    fixture_models(script, config, (granite(), None), older_prompt)
}

/// The same with the models the manager starts with.
fn fixture_models(
    script: impl IntoIterator<Item = Start>,
    config: Config,
    (active, previous): (Model, Option<Model>),
    older_prompt: OlderPrompt,
) -> Fixture {
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
            active,
            previous,
            records: Some(Arc::clone(&records) as Arc<dyn Records>),
            from_owner_setting: false,
            device_default: false,
            older_prompt,
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
        until(Duration::from_hours(3), move || {
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
    // A new file starts at the default reply length; the calibration samples generated 64
    // tokens each, and the budgets were derived for the default reply.
    assert_eq!(report.model.max_tokens, 1024);
    assert!(
        report
            .calibration
            .samples
            .iter()
            .all(|s| tokens_are(s.predicted_n, 64))
    );
    assert_eq!(report.budgets.max_tokens, 1024);
    assert_eq!(report.model.spec, "local:other.gguf");
    assert_eq!(report.model.alias, "other");
    assert_eq!(report.model.size, Some(1 << 30));
    // The candidate was started at the default reply length and its recorded hash; the previous
    // model then started again, and serves with its own budgets.
    let selected = fixture.launcher.selected.lock().unwrap().clone();
    assert_eq!(selected.len(), 3);
    assert_eq!(
        (selected[1].sha256.clone(), selected[1].max_tokens),
        (sha('b'), 1024)
    );
    assert_eq!(selected[0], granite());
    assert_eq!(selected[2], granite());
    assert_eq!(fixture.assistant.budgets().unwrap().max_tokens, 1024);
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
                phase: Phase::Phrase(2),
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
async fn the_trial_records_the_largest_reply_as_information_and_always_starts_at_1024() {
    for (tokens, largest) in [
        ([60.0, 70.0, 80.0, 65.0, 75.0], 80),
        ([60.0, 70.0, 263.0, 65.0, 75.0], 263),
        ([60.0, 970.0, 80.0, 65.0, 75.0], 970),
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
        assert_eq!(report.largest_predicted, largest, "{tokens:?}");
        assert_eq!(report.model.max_tokens, 1024, "{tokens:?}");
        assert_eq!(report.budgets.max_tokens, 1024);
        fixture.handle.shutdown().await;
    }
}

#[tokio::test(start_paused = true)]
async fn a_file_with_a_default_files_hash_or_name_also_starts_at_the_default_reply_length() {
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
    assert_eq!(report.model.max_tokens, 1024);
    assert_eq!(report.largest_predicted, 80, "the phrases still ran");
    renamed.handle.shutdown().await;

    // A different file with the default's name gets the same start.
    let impostor = fixture([], CONFIG);
    impostor
        .launcher
        .files
        .lock()
        .unwrap()
        .insert("granite-4.2-3b-Q4_K_M.gguf".into(), Ok(file(&sha('d'))));
    ready(&impostor.assistant).await;
    let report = passed(impostor.try_and_wait("granite-4.2-3b-Q4_K_M.gguf").await);
    assert_eq!(report.model.max_tokens, 1024);
    assert_eq!(report.model.sha256, sha('d'));
    impostor.handle.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_truncated_or_invalid_reply_fails_the_trial_and_the_previous_model_restarts() {
    for (fault, expected) in [
        (Fault::TruncatedPhrase, "cut off at 1024 tokens"),
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
async fn a_model_too_slow_for_the_ceiling_fails_with_the_numbers() {
    let fixture = fixture(
        [
            Start::Ok(Behaviour::default()),
            // 0.6 s per generated token: a default reply of 1,024 tokens takes about 680 s
            // with the slowdown, R over 850 s, past the 600 s ceiling (the 64-token samples
            // take 38 s each).
            Start::Ok(Behaviour {
                g: 0.6,
                ..Behaviour::default()
            }),
        ],
        CONFIG,
    );
    let why = failure(fixture.trial().await);
    assert!(why.contains("too slow"), "{why}");
    assert!(why.contains("600 s"), "{why}");
    assert_eq!(fixture.assistant.state(), RuntimeState::Ready);
    assert_eq!(
        fixture.launcher.selected.lock().unwrap().last(),
        Some(&granite())
    );
}

#[tokio::test(start_paused = true)]
async fn a_phrase_past_its_bound_stops_the_candidate_and_restores_the_previous_model() {
    // The bound is twice R of a default reply on the candidate's measured speed, at most the
    // ceiling; a default reply is 1,134.1 tokens with the CPU slowdown, 1,060.7 with the
    // GPU's. g 0.01: T about 11.9 s on the CPU, R 15 s, bound 30 s; 12.2 s on the GPU (its
    // prompt term is 1.6 s), R 20 s, bound 40 s. g 0.1: T about 114 s, R 145 s, bound 290 s.
    // g 0.4: T about 454 s, R 570 s, twice that over the ceiling: 600 s.
    for (config, g, seconds) in [
        (CONFIG, 0.01, 30),
        (gpu_config(), 0.01, 40),
        (CONFIG, 0.1, 290),
        (CONFIG, 0.4, 600),
    ] {
        let fixture = fixture(
            [
                Start::Ok(Behaviour::default()),
                Start::Ok(Behaviour {
                    g,
                    phrase_delay: Duration::from_secs(seconds + 5),
                    ..Behaviour::default()
                }),
            ],
            config,
        );
        let why = failure(fixture.trial().await);
        assert!(
            why.contains(&format!("took more than {seconds} s")),
            "{g}: {why}"
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
async fn a_phrase_inside_its_bound_passes() {
    // Bound 30 s at g 0.01; a phrase of 29 s passes.
    let fixture = fixture(
        [
            Start::Ok(Behaviour::default()),
            Start::Ok(Behaviour {
                phrase_delay: Duration::from_secs(29),
                ..Behaviour::default()
            }),
        ],
        CONFIG,
    );
    passed(fixture.trial().await);
}

#[tokio::test(start_paused = true)]
async fn the_whole_trial_is_bounded_hashing_and_the_candidates_start_included() {
    // Hashing alone may not take the whole bound.
    let slow_disk = fixture([], CONFIG);
    *slow_disk.launcher.hash_delay.lock().unwrap() = Some(Duration::from_mins(100));
    let why = failure(slow_disk.trial().await);
    assert!(why.contains("longer than its 90 minutes"), "{why}");
    assert_eq!(slow_disk.assistant.state(), RuntimeState::Ready);
    slow_disk.handle.shutdown().await;

    // The bound is the same on the GPU path, and a start that takes longer fails the same way.
    let slow_start = fixture([], gpu_config());
    ready(&slow_start.assistant).await;
    // Only the candidate's host check is slow; the previous model's own start is not.
    slow_start
        .launcher
        .preflight_delays
        .lock()
        .unwrap()
        .push_back(Duration::from_mins(95));
    let why = failure(slow_start.try_and_wait("other.gguf").await);
    assert!(why.contains("longer than its 90 minutes"), "{why}");
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
    assert_eq!(
        fixture.handle.cancel_trial(1),
        CancelTrial::NoTrial,
        "no trial runs yet"
    );
    fixture
        .handle
        .try_model(7, "local:other.gguf".into(), "other.gguf".into());
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        matches!(
            handle.status().trial,
            Some(TrialState::Running {
                phase: Phase::Phrase(1),
                ..
            })
        )
    })
    .await;
    assert_eq!(fixture.handle.cancel_trial(1), CancelTrial::Done);
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
    assert_eq!(fixture.handle.cancel_trial(1), CancelTrial::NoTrial);
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
                phase: Phase::Phrase(1),
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
                phase: Phase::Phrase(1),
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
    // The new model restarted and was primed and calibrated for its recorded max_tokens.
    assert_eq!(
        fixture.launcher.selected.lock().unwrap().last(),
        Some(&report.model)
    );
    assert_eq!(fixture.launcher.primed.load(Ordering::SeqCst), primed + 1);
    assert_eq!(fixture.assistant.budgets().unwrap().max_tokens, 1024);
    // The stored trial is the record of what was measured.
    let stored = fixture.records.stored.lock().unwrap().clone();
    let trial: serde_json::Value =
        serde_json::from_str(stored.active.unwrap().trial.as_deref().unwrap()).unwrap();
    // The calibration ran at the default reply length, whatever length the file will use.
    assert_eq!(trial["measured_max_tokens"], 1024);
    assert!(trial.get("max_tokens").is_none());
    // The prompt the trial ran under is recorded, and a trial under the current one is not
    // flagged as older.
    assert_eq!(
        trial["prefix_sha256"],
        crate::assistant::contract::prefix_sha256()
    );
    assert!(!status.model.older_prompt.is_older());
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
    assert_eq!(fixture.assistant.budgets().unwrap().max_tokens, 1024);
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
    assert_eq!(fixture.handle.cancel_trial(1), CancelTrial::Done);
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

/// Waits until the first phrase of the running trial is being sent.
async fn candidate_running(handle: &Handle) {
    let handle = handle.clone();
    until(Duration::from_secs(600), move || {
        matches!(
            handle.status().trial,
            Some(TrialState::Running {
                phase: Phase::Phrase(1),
                ..
            })
        )
    })
    .await;
}

fn slow_phrases() -> [Start; 2] {
    [
        Start::Ok(Behaviour::default()),
        Start::Ok(Behaviour {
            phrase_delay: Duration::from_secs(100),
            ..Behaviour::default()
        }),
    ]
}

#[tokio::test(start_paused = true)]
async fn an_interrupted_trial_stops_its_running_candidate_before_it_settles() {
    // The launcher's settle waits for the child's exit watcher (as the real one does), so
    // settling first would wait for a child nothing stops.
    for trigger in ["cancel", "restart", "shutdown"] {
        let fixture = fixture(slow_phrases(), CONFIG);
        ready(&fixture.assistant).await;
        fixture
            .handle
            .try_model(7, "local:other.gguf".into(), "other.gguf".into());
        candidate_running(&fixture.handle).await;
        match trigger {
            "cancel" => assert_eq!(fixture.handle.cancel_trial(1), CancelTrial::Done),
            "restart" => assert_eq!(fixture.handle.restart(), Restart::Accepted),
            _ => fixture.handle.shutdown().await,
        }
        if trigger == "shutdown" {
            assert_eq!(
                fixture.launcher.alive.load(Ordering::SeqCst),
                0,
                "{trigger}"
            );
        } else {
            let handle = fixture.handle.clone();
            until(Duration::from_secs(600), move || {
                handle.status().trial.is_some_and(|trial| !trial.running())
            })
            .await;
            ready(&fixture.assistant).await;
            assert_eq!(
                fixture.launcher.alive.load(Ordering::SeqCst),
                1,
                "{trigger}"
            );
            fixture.handle.shutdown().await;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn an_interrupted_or_expired_hash_stops_its_blocking_work_first() {
    use super::super::tests::HashProbe;
    for expire in [false, true] {
        let fixture = fixture([], CONFIG);
        let probe = Arc::new(HashProbe::default());
        *fixture.launcher.hash_probe.lock().unwrap() = Some(Arc::clone(&probe));
        ready(&fixture.assistant).await;
        fixture
            .handle
            .try_model(7, "local:other.gguf".into(), "other.gguf".into());
        // Yielding, not sleeping: a paused clock does not advance while blocking work runs.
        while fixture.launcher.hashed.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
        if expire {
            // The trial's bound passes while the blocking work still runs.
            tokio::time::advance(Duration::from_mins(91)).await;
        } else {
            assert_eq!(fixture.handle.cancel_trial(1), CancelTrial::Done);
        }
        let handle = fixture.handle.clone();
        until(Duration::from_hours(3), move || {
            handle.status().trial.is_some_and(|trial| !trial.running())
        })
        .await;
        ready(&fixture.assistant).await;
        // The blocking work saw the cancel; it did not run on to its own end, which the
        // next session's settle would have waited for.
        assert!(
            probe.cancelled_seen.load(Ordering::SeqCst),
            "expire {expire}"
        );
        assert!(!probe.timed_out.load(Ordering::SeqCst), "expire {expire}");
        fixture.handle.shutdown().await;
    }
}

#[tokio::test(start_paused = true)]
async fn model_changes_are_admitted_one_at_a_time() {
    let fixture = fixture([], CONFIG);
    let report = passed(fixture.trial().await);
    // A Switch, then everything else is refused while it is carried out.
    assert_eq!(fixture.handle.switch(7, &report.key), Chosen::Accepted);
    assert_eq!(fixture.handle.switch(7, &report.key), Chosen::Pending);
    assert_eq!(fixture.handle.switch_back(7), Chosen::Pending);
    assert_eq!(fixture.handle.keep(), Chosen::Pending);
    assert_eq!(
        fixture
            .handle
            .try_model(7, "local:third.gguf".into(), "third.gguf".into()),
        TryModel::Pending
    );
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        handle.status().model.active.spec == "local:other.gguf"
            && handle.status().state == State::Ready
    })
    .await;
    // A double-clicked Switch back swaps once, not twice.
    assert_eq!(fixture.handle.switch_back(7), Chosen::Accepted);
    assert_eq!(fixture.handle.switch_back(7), Chosen::Pending);
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        handle.status().model.active == granite() && handle.status().state == State::Ready
    })
    .await;
    tokio::time::sleep(Duration::from_mins(5)).await;
    assert_eq!(
        fixture.handle.status().model.active,
        granite(),
        "not undone"
    );
    assert_eq!(
        fixture.records.log.lock().unwrap().len(),
        2,
        "one switch, one swap"
    );
    // A Restart in progress holds model changes off too.
    assert_eq!(fixture.handle.restart(), Restart::Accepted);
    assert_eq!(
        fixture
            .handle
            .try_model(7, "local:other.gguf".into(), "other.gguf".into()),
        TryModel::Pending
    );
    assert_eq!(fixture.handle.switch_back(7), Chosen::Pending);
    // Once it is over they are admitted again.
    ready(&fixture.assistant).await;
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        !handle.status().restarting
    })
    .await;
    assert!(matches!(
        fixture
            .handle
            .try_model(7, "local:other.gguf".into(), "other.gguf".into()),
        TryModel::Accepted(_)
    ));
}

#[tokio::test(start_paused = true)]
async fn a_cancel_from_a_page_showing_an_older_trial_cancels_nothing() {
    let fixture = fixture(
        [
            Start::Ok(Behaviour::default()),
            Start::Ok(Behaviour::default()),
            Start::Ok(Behaviour::default()),
            Start::Ok(Behaviour {
                phrase_delay: Duration::from_secs(25),
                ..Behaviour::default()
            }),
        ],
        CONFIG,
    );
    passed(fixture.trial().await);
    fixture
        .handle
        .try_model(7, "local:other.gguf".into(), "other.gguf".into());
    candidate_running(&fixture.handle).await;
    // The tab still shows trial 1.
    assert_eq!(fixture.handle.cancel_trial(1), CancelTrial::Stale);
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert!(fixture.handle.status().trial.is_some_and(|t| t.running()));
    let handle = fixture.handle.clone();
    until(Duration::from_secs(3600), move || {
        handle.status().trial.is_some_and(|trial| !trial.running())
    })
    .await;
    assert!(matches!(
        fixture.handle.status().trial,
        Some(TrialState::Passed(_))
    ));
}

#[tokio::test(start_paused = true)]
async fn only_restart_and_the_apps_start_reset_the_breaker() {
    let fixture = fixture([], CONFIG);
    let report = passed(fixture.trial().await);
    let resets = || fixture.assistant.resets.load(Ordering::SeqCst);
    assert_eq!(resets(), 1, "the app's start");
    fixture.handle.switch(7, &report.key);
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        handle.status().model.active.spec == "local:other.gguf"
            && handle.status().state == State::Ready
    })
    .await;
    assert_eq!(resets(), 1, "a trial and a Switch leave the breaker alone");
    fixture.handle.switch_back(7);
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        handle.status().model.active == granite() && handle.status().state == State::Ready
    })
    .await;
    assert_eq!(resets(), 1, "so does a Switch back");
    assert_eq!(fixture.handle.restart(), Restart::Accepted);
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        !handle.status().restarting
    })
    .await;
    assert_eq!(resets(), 2, "a Restart resets it");
}

#[tokio::test(start_paused = true)]
async fn a_file_over_8_gib_or_empty_is_refused_with_its_reason_before_anything_starts() {
    let fixture = fixture([], CONFIG);
    for (name, size, reason) in [
        ("huge.gguf", (8 << 30) + 1, "larger than 8 GiB"),
        ("empty.gguf", 0, "is empty"),
    ] {
        fixture.launcher.files.lock().unwrap().insert(
            name.into(),
            Ok(ModelFile {
                sha256: sha('e'),
                size,
            }),
        );
        ready(&fixture.assistant).await;
        let why = failure(fixture.try_and_wait(name).await);
        assert!(why.contains(reason), "{why}");
    }
    // Only the serving sessions started, never a candidate.
    assert_eq!(fixture.launcher.starts.lock().unwrap().len(), 3);
    fixture.launcher.files.lock().unwrap().insert(
        "exact.gguf".into(),
        Ok(ModelFile {
            sha256: sha('f'),
            size: 8 << 30,
        }),
    );
    passed(fixture.try_and_wait("exact.gguf").await);
}

#[tokio::test(start_paused = true)]
async fn a_model_measured_with_an_older_prompt_is_flagged_until_a_new_trial_replaces_it() {
    // The active model's stored trial predates the prompt in force; the previous one's too.
    let fixture = fixture_older([], CONFIG, (true, true));
    assert!(fixture.handle.status().model.older_prompt.is_older());
    // A trial of another file runs under the current prompt; switching to it clears the flag
    // and the old active model, now the previous one, keeps its own.
    let report = passed(fixture.trial().await);
    assert!(
        fixture.handle.status().model.older_prompt.is_older(),
        "a trial alone changes nothing"
    );
    fixture.handle.switch(7, &report.key);
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        handle.status().model.active.spec == "local:other.gguf"
            && handle.status().state == State::Ready
    })
    .await;
    assert!(!fixture.handle.status().model.older_prompt.is_older());
    // Switching back returns to the model measured with the older prompt: flagged again.
    let generation = fixture.handle.status().generation;
    assert_eq!(fixture.handle.switch_back(7), Chosen::Accepted);
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        handle.status().generation > generation && handle.status().state == State::Ready
    })
    .await;
    assert!(fixture.handle.status().model.older_prompt.is_older());
    // And forward again to the model measured with the current one.
    assert_eq!(fixture.handle.switch_back(7), Chosen::Accepted);
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        handle.status().model.active.spec == "local:other.gguf"
            && handle.status().state == State::Ready
    })
    .await;
    assert!(!fixture.handle.status().model.older_prompt.is_older());
}

fn other() -> Model {
    Model::test("other.gguf", &sha('b'), 1024)
}

#[tokio::test(start_paused = true)]
async fn trying_the_active_file_again_keeps_the_previous_models_flag() {
    // Active: other.gguf, measured under the current prompt; previous: Granite, measured
    // under an older one. other.gguf's file hash is the active model's, so a new trial of it
    // replaces the active model and keeps the previous one (no second previous is stored).
    let older = OlderPrompt {
        active: false,
        previous: true,
    };
    let fixture = fixture_models([], CONFIG, (other(), Some(granite())), older);
    let report = passed(fixture.trial().await);
    fixture.handle.switch(7, &report.key);
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        handle.status().generation >= 4 && handle.status().state == State::Ready
    })
    .await;
    assert_eq!(
        *fixture.records.log.lock().unwrap(),
        ["activate local:other.gguf (previous kept) by 7"]
    );
    let status = fixture.handle.status();
    assert_eq!(status.model.previous, Some(granite()));
    assert!(!status.model.older_prompt.is_older());
    // The previous model is still the one measured with the older prompt.
    let generation = status.generation;
    assert_eq!(fixture.handle.switch_back(7), Chosen::Accepted);
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        handle.status().generation > generation && handle.status().state == State::Ready
    })
    .await;
    assert_eq!(fixture.handle.status().model.active, granite());
    assert!(fixture.handle.status().model.older_prompt.is_older());
}

#[tokio::test(start_paused = true)]
async fn switching_back_moves_each_models_flag_with_it() {
    // Active Granite is current, previous other.gguf is flagged; the other way round after
    // one Switch back, and back again.
    let older = OlderPrompt {
        active: false,
        previous: true,
    };
    let fixture = fixture_models([], CONFIG, (granite(), Some(other())), older);
    ready(&fixture.assistant).await;
    assert!(!fixture.handle.status().model.older_prompt.is_older());
    for (flagged, spec) in [(true, "local:other.gguf"), (false, granite().spec.as_str())] {
        let generation = fixture.handle.status().generation;
        assert_eq!(fixture.handle.switch_back(7), Chosen::Accepted);
        let handle = fixture.handle.clone();
        until(Duration::from_secs(600), move || {
            handle.status().generation > generation && handle.status().state == State::Ready
        })
        .await;
        let status = fixture.handle.status();
        assert_eq!(status.model.active.spec, spec);
        assert_eq!(status.model.older_prompt.is_older(), flagged, "{spec}");
    }
}

// The reply length ("Longest reply", ASSIST-59).

use super::super::SetLength;

async fn ready_at(fixture: &Fixture, generation: u64) {
    let handle = fixture.handle.clone();
    until(Duration::from_mins(30), move || {
        let status = handle.status();
        status.generation >= generation && status.state == State::Ready && !status.restarting
    })
    .await;
}

/// How many times the breaker has been reset (the app's start counts as one).
fn resets(fixture: &Fixture) -> usize {
    fixture.assistant.resets.load(Ordering::SeqCst)
}

/// The reply lengths stored per file, as the fake records hold them.
fn stored_length(fixture: &Fixture, sha256: &str) -> Option<u32> {
    fixture.records.lengths.lock().unwrap().get(sha256).copied()
}

#[tokio::test(start_paused = true)]
async fn the_reply_length_is_stored_once_and_the_server_restarts_once_calibrated_for_it() {
    let fixture = fixture([], CONFIG);
    ready(&fixture.assistant).await;
    assert_eq!(fixture.handle.status().model.active.max_tokens, 1024);
    let primed = fixture.launcher.primed.load(Ordering::SeqCst);
    assert_eq!(
        fixture.handle.set_length(7, 1536).await,
        SetLength::Accepted
    );
    // Admitted alone: everything else waits until the restart is over.
    assert_eq!(fixture.handle.set_length(7, 2048).await, SetLength::Pending);
    assert_eq!(fixture.handle.switch_back(7), Chosen::Pending);
    assert_eq!(
        fixture
            .handle
            .try_model(7, "local:other.gguf".into(), "other.gguf".into()),
        TryModel::Pending
    );
    ready_at(&fixture, 2).await;
    tokio::time::sleep(Duration::from_mins(10)).await;
    // One row of the file stored with the owner's account, one restart, one priming, no
    // hashing, no trial, none of the five phrases.
    assert_eq!(
        *fixture.records.log.lock().unwrap(),
        ["set_length 1536 by 7"]
    );
    assert_eq!(stored_length(&fixture, DEFAULT_MODEL_SHA256), Some(1536));
    assert_eq!(fixture.launcher.starts.lock().unwrap().len(), 2);
    assert_eq!(fixture.launcher.primed.load(Ordering::SeqCst), primed + 1);
    assert!(fixture.launcher.hashed.lock().unwrap().is_empty());
    let status = fixture.handle.status();
    assert_eq!(status.trial, None);
    assert_eq!(status.model.next_trial, 1);
    assert_eq!(status.generation, 2);
    // The same file with the new length; no model row, no previous model, no choice made.
    let mut expected = granite();
    expected.max_tokens = 1536;
    assert_eq!(status.model.active, expected);
    assert_eq!(status.model.previous, None);
    assert!(!status.model.from_owner_setting);
    assert_eq!(*fixture.records.stored.lock().unwrap(), Stored::default());
    assert_eq!(
        fixture.launcher.selected.lock().unwrap().last(),
        Some(&expected)
    );
    // Re-calibrated: the budgets are derived for the new length.
    assert_eq!(fixture.assistant.budgets().unwrap().max_tokens, 1536);
    assert_eq!(status.model.note, None);
    assert_eq!(resets(&fixture), 1, "the breaker is left alone");
}

#[tokio::test(start_paused = true)]
async fn a_longer_reply_lengthens_the_run_deadline_from_the_measured_speed() {
    // g 0.15 s a token: R is 1.25 × (0.54 + 0.15 × L × (1 + 2.1e-4 × L / 2)) up to 5 s.
    let slow = || Behaviour {
        g: 0.15,
        ..Behaviour::default()
    };
    let fixture = fixture([Start::Ok(slow()), Start::Ok(slow())], CONFIG);
    ready(&fixture.assistant).await;
    // 1,024 tokens: T 170.7 s, R 213.3 s, so 215 s.
    assert_eq!(fixture.handle.status().budgets.unwrap().run.as_secs(), 215);
    assert_eq!(
        fixture.handle.set_length(7, 2048).await,
        SetLength::Accepted
    );
    ready_at(&fixture, 2).await;
    // 2,048 tokens: T 373.8 s, R 467.3 s, so 470 s; the total wait doubles it, and the queue
    // carries the slack for the server still busy after an expiry.
    let budgets = fixture.handle.status().budgets.unwrap();
    assert_eq!(budgets.run.as_secs(), 470);
    assert_eq!(
        budgets.queue_wait,
        Duration::from_secs(470) + budgets.busy_after_expiry
    );
}

#[tokio::test(start_paused = true)]
async fn crafted_unchanged_busy_and_unsupported_reply_lengths_start_nothing() {
    let fixture = fixture([], CONFIG);
    ready(&fixture.assistant).await;
    for refused in [
        0,
        1,
        255,
        257,
        1000,
        1023,
        1025,
        2561,
        2688,
        4096,
        100_000,
        u32::MAX,
    ] {
        assert_eq!(
            fixture.handle.set_length(7, refused).await,
            SetLength::Invalid,
            "{refused}"
        );
    }
    // The active value again changes nothing.
    assert_eq!(
        fixture.handle.set_length(7, 1024).await,
        SetLength::Unchanged
    );
    tokio::time::sleep(Duration::from_mins(5)).await;
    assert!(fixture.records.log.lock().unwrap().is_empty());
    assert_eq!(fixture.launcher.starts.lock().unwrap().len(), 1);
    assert!(!fixture.handle.status().restarting);
    // The bounds themselves are accepted, one at a time.
    assert_eq!(fixture.handle.set_length(7, 256).await, SetLength::Accepted);
    ready_at(&fixture, 2).await;
    assert_eq!(
        fixture.handle.set_length(7, 2560).await,
        SetLength::Accepted
    );
    ready_at(&fixture, 3).await;
    assert_eq!(fixture.handle.status().model.active.max_tokens, 2560);
    // A manager without a model setting (the development endpoint, fixed models) refuses.
    let launcher = FakeLauncher::scripted([]);
    let (assistant, handle) = super::super::tests::manager(&launcher);
    ready(&assistant).await;
    assert_eq!(handle.set_length(7, 1536).await, SetLength::Unsupported);
}

#[tokio::test(start_paused = true)]
async fn the_reply_length_waits_for_a_running_trial_and_a_restart() {
    let fixture = fixture([], CONFIG);
    ready(&fixture.assistant).await;
    fixture
        .handle
        .try_model(7, "local:other.gguf".into(), "other.gguf".into());
    // The trial is running from the moment it is admitted.
    assert_eq!(fixture.handle.set_length(7, 1536).await, SetLength::Busy);
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        matches!(
            handle.status().trial,
            Some(TrialState::Running {
                phase: Phase::Phrase(2),
                ..
            })
        )
    })
    .await;
    assert_eq!(fixture.handle.set_length(7, 1536).await, SetLength::Busy);
    ready(&fixture.assistant).await;
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        handle.status().trial.is_some_and(|trial| !trial.running())
    })
    .await;
    ready(&fixture.assistant).await;
    // A Restart in progress holds it off too.
    assert_eq!(fixture.handle.restart(), Restart::Accepted);
    assert_eq!(fixture.handle.set_length(7, 1536).await, SetLength::Pending);
    assert!(fixture.records.log.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_reply_length_that_cannot_be_stored_changes_nothing_restarts_nothing_and_says_why() {
    let fixture = fixture([], CONFIG);
    ready(&fixture.assistant).await;
    *fixture.records.fail.lock().unwrap() = Some("the reply length cannot be stored".into());
    assert_eq!(
        fixture.handle.set_length(7, 1536).await,
        SetLength::NotStored
    );
    let status = fixture.handle.status();
    // Refused before anything stopped: the server keeps serving, nothing is pending, and the
    // note does not say "stored" twice.
    assert_eq!(status.state, State::Ready);
    assert!(!status.restarting && !status.model.pending);
    assert_eq!(
        status.model.note.as_deref(),
        Some("the reply length cannot be stored; the current setting stays")
    );
    assert_eq!(status.model.active, granite());
    assert_eq!(status.model.previous, None);
    tokio::time::sleep(Duration::from_mins(10)).await;
    assert_eq!(fixture.launcher.starts.lock().unwrap().len(), 1);
    assert_eq!(fixture.handle.status().generation, 1);
    assert_eq!(fixture.assistant.budgets().unwrap().max_tokens, 1024);
    // Once the store works again the same request is admitted.
    *fixture.records.fail.lock().unwrap() = None;
    assert_eq!(
        fixture.handle.set_length(7, 1536).await,
        SetLength::Accepted
    );
}

#[tokio::test(start_paused = true)]
async fn a_start_that_fails_for_another_reason_keeps_the_new_length_and_retries_with_it() {
    let fixture = fixture(
        [
            Start::Ok(Behaviour::default()),
            Start::Other("the model could not be loaded"),
            Start::Ok(Behaviour::default()),
        ],
        CONFIG,
    );
    ready(&fixture.assistant).await;
    assert_eq!(
        fixture.handle.set_length(7, 2048).await,
        SetLength::Accepted
    );
    ready_at(&fixture, 3).await;
    // Not a reply-length outcome: the normal counted retry runs, at the new length.
    assert_eq!(
        *fixture.records.log.lock().unwrap(),
        ["set_length 2048 by 7"]
    );
    let status = fixture.handle.status();
    assert_eq!(status.model.active.max_tokens, 2048);
    assert_eq!(status.model.note, None);
    assert_eq!(fixture.assistant.budgets().unwrap().max_tokens, 2048);
    assert_eq!(stored_length(&fixture, DEFAULT_MODEL_SHA256), Some(2048));
    assert_eq!(fixture.launcher.starts.lock().unwrap().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn a_sandbox_failure_or_a_changed_model_file_keeps_the_new_length_and_follows_its_own_path() {
    for (failure, expected) in [
        (Start::Sandbox("the sandbox refused"), "the sandbox refused"),
        (
            Start::Other(
                "the model file changed: it no longer has the SHA-256 recorded for it; try the model again from the Model field",
            ),
            "the model file changed",
        ),
    ] {
        let fixture = fixture([Start::Ok(Behaviour::default()), failure], CONFIG);
        ready(&fixture.assistant).await;
        assert_eq!(
            fixture.handle.set_length(7, 1536).await,
            SetLength::Accepted
        );
        let why = unavailable(&fixture.handle).await;
        assert!(why.contains(expected), "{why}");
        let status = fixture.handle.status();
        assert_eq!(status.model.active.max_tokens, 1536, "{expected}");
        assert_eq!(
            *fixture.records.log.lock().unwrap(),
            ["set_length 1536 by 7"],
            "{expected}: no revert"
        );
        assert_eq!(stored_length(&fixture, DEFAULT_MODEL_SHA256), Some(1536));
        fixture.handle.shutdown().await;
    }
}

#[tokio::test(start_paused = true)]
async fn a_calibration_sample_timeout_keeps_the_new_length() {
    // The samples take about 520 s whatever the reply length: not a length outcome.
    let slow = Behaviour {
        p: 0.05,
        g: 8.0,
        ..Behaviour::default()
    };
    let fixture = fixture([Start::Ok(Behaviour::default()), Start::Ok(slow)], CONFIG);
    ready(&fixture.assistant).await;
    assert_eq!(
        fixture.handle.set_length(7, 1536).await,
        SetLength::Accepted
    );
    let handle = fixture.handle.clone();
    until(Duration::from_mins(30), move || {
        matches!(handle.status().state, State::Unavailable(_))
    })
    .await;
    let status = fixture.handle.status();
    let State::Unavailable(why) = &status.state else {
        panic!("{:?}", status.state)
    };
    assert!(
        why.contains("a calibration sample took more than 480 s"),
        "{why}"
    );
    assert_eq!(status.model.active.max_tokens, 1536);
    assert_eq!(
        *fixture.records.log.lock().unwrap(),
        ["set_length 1536 by 7"]
    );
}

fn slow_at_0_3() -> Start {
    Start::Ok(Behaviour {
        g: 0.3,
        ..Behaviour::default()
    })
}

#[tokio::test(start_paused = true)]
async fn a_new_length_that_is_too_slow_goes_back_on_both_paths_without_resetting_the_breaker() {
    // g 0.3 s: 1,024 tokens fit (R 430 s on the CPU, 400 s on the GPU), 2,560 do not.
    for (config, path) in [(CONFIG, Path::Cpu), (gpu_config(), Path::Gpu)] {
        let fixture = fixture([slow_at_0_3(), slow_at_0_3(), slow_at_0_3()], config);
        ready(&fixture.assistant).await;
        assert_eq!(
            fixture.handle.set_length(7, 2560).await,
            SetLength::Accepted
        );
        ready_at(&fixture, 3).await;
        let status = fixture.handle.status();
        assert_eq!(status.model.active.max_tokens, 1024, "{path:?}");
        let note = status.model.note.unwrap();
        assert!(
            note.contains("2560") && note.contains("too slow") && note.contains("back to 1024"),
            "{path:?}: {note}"
        );
        // No model row or previous model came of it.
        assert_eq!(status.model.previous, None);
        assert_eq!(*fixture.records.stored.lock().unwrap(), Stored::default());
        // The file had no stored length: the revert deletes the row.
        assert_eq!(stored_length(&fixture, DEFAULT_MODEL_SHA256), None);
        // The GPU path was tried again at the old length, not left for the CPU.
        assert_eq!(*fixture.launcher.paths.lock().unwrap(), [path; 3]);
        assert_eq!(
            *fixture.records.log.lock().unwrap(),
            ["set_length 2560 by 7", "set_length none by 7"]
        );
        // Only Restart and the app's start reset the breaker.
        assert_eq!(resets(&fixture), 1, "{path:?}");
        assert_eq!(status.failures, 0, "not counted against the model server");
        fixture.handle.shutdown().await;
    }
}

#[tokio::test(start_paused = true)]
async fn a_revert_restores_the_files_earlier_length_not_the_default() {
    let fixture = fixture(
        [
            slow_at_0_3(),
            slow_at_0_3(),
            slow_at_0_3(),
            slow_at_0_3(),
            slow_at_0_3(),
        ],
        CONFIG,
    );
    ready(&fixture.assistant).await;
    // 1,280 tokens still fit at g 0.3 s (R about 480 s); 2,560 do not.
    assert_eq!(
        fixture.handle.set_length(7, 1280).await,
        SetLength::Accepted
    );
    ready_at(&fixture, 2).await;
    assert_eq!(
        fixture.handle.set_length(7, 2560).await,
        SetLength::Accepted
    );
    ready_at(&fixture, 4).await;
    assert_eq!(fixture.handle.status().model.active.max_tokens, 1280);
    assert_eq!(stored_length(&fixture, DEFAULT_MODEL_SHA256), Some(1280));
    assert_eq!(
        *fixture.records.log.lock().unwrap(),
        [
            "set_length 1280 by 7",
            "set_length 2560 by 7",
            "set_length 1280 by 7"
        ]
    );
    assert_eq!(fixture.assistant.budgets().unwrap().max_tokens, 1280);
    assert_eq!(resets(&fixture), 1);
    // A Restart still resets it.
    assert_eq!(fixture.handle.restart(), Restart::Accepted);
    ready_at(&fixture, 5).await;
    assert_eq!(resets(&fixture), 2);
}

#[tokio::test(start_paused = true)]
async fn a_reply_too_large_for_the_context_during_a_length_change_goes_back_with_its_note() {
    // A prefix of 5,600 tokens leaves room for 1,024 but not for 2,560 at any bound
    // (5,600 + at least 146 + 2,560 + 64 is over 8,192); the first start is the short prefix.
    let big = Behaviour {
        prefix_tokens: 5600.0,
        ..Behaviour::default()
    };
    for config in [CONFIG, gpu_config()] {
        let fixture = fixture(
            [
                Start::Ok(Behaviour::default()),
                Start::Ok(big.clone()),
                Start::Ok(Behaviour::default()),
            ],
            config,
        );
        ready(&fixture.assistant).await;
        assert_eq!(
            fixture.handle.set_length(7, 2560).await,
            SetLength::Accepted
        );
        ready_at(&fixture, 3).await;
        let status = fixture.handle.status();
        assert_eq!(status.model.active.max_tokens, 1024);
        let note = status.model.note.unwrap();
        assert!(
            note.contains("2560")
                && note.contains("reply length too large for the context")
                && note.contains("back to 1024"),
            "{note}"
        );
        assert_eq!(
            *fixture.records.log.lock().unwrap(),
            ["set_length 2560 by 7", "set_length none by 7"]
        );
        assert_eq!(stored_length(&fixture, DEFAULT_MODEL_SHA256), None);
        assert_eq!(resets(&fixture), 1);
        // The GPU was not blamed: no fallback to the CPU.
        assert!(status.gpu_note.is_none(), "{:?}", status.gpu_note);
        fixture.handle.shutdown().await;
    }
}

#[tokio::test(start_paused = true)]
async fn a_reply_too_large_for_the_context_on_the_gpu_is_held_not_blamed_on_the_gpu() {
    // Even the default length does not fit a prefix of 8,100 tokens: the GPU is not at fault,
    // and the CPU would not fit it either.
    let big = Behaviour {
        prefix_tokens: 8100.0,
        ..Behaviour::default()
    };
    let fixture = fixture([Start::Ok(big)], gpu_config());
    let why = unavailable(&fixture.handle).await;
    assert!(
        why.starts_with("reply length too large for the context"),
        "{why}"
    );
    let status = fixture.handle.status();
    assert!(status.gpu_note.is_none(), "{:?}", status.gpu_note);
    assert_eq!(status.path, Path::Gpu);
    tokio::time::sleep(Duration::from_mins(10)).await;
    assert_eq!(*fixture.launcher.paths.lock().unwrap(), [Path::Gpu]);
    assert_eq!(fixture.launcher.starts.lock().unwrap().len(), 1);
    // Too slow, by contrast, is the GPU's failure and falls back to the CPU as before.
    let slow = Behaviour {
        p: 0.01,
        g: 0.6,
        ..Behaviour::default()
    };
    let fixture = self::fixture([Start::Ok(slow)], gpu_config());
    ready(&fixture.assistant).await;
    assert_eq!(
        *fixture.launcher.paths.lock().unwrap(),
        [Path::Gpu, Path::Cpu]
    );
}

#[tokio::test(start_paused = true)]
async fn a_length_change_whose_first_start_was_ready_is_kept_through_a_later_failure() {
    // After a ready start the change is kept: a later failure restarts at the same length
    // and never reverts it.
    let fixture = fixture(
        [
            Start::Ok(Behaviour::default()),
            Start::Ok(Behaviour::default()),
            Start::Other("the model could not be loaded"),
            Start::Ok(Behaviour::default()),
        ],
        CONFIG,
    );
    ready(&fixture.assistant).await;
    assert_eq!(
        fixture.handle.set_length(7, 1536).await,
        SetLength::Accepted
    );
    ready_at(&fixture, 2).await;
    assert_eq!(fixture.handle.restart(), Restart::Accepted);
    tokio::time::sleep(Duration::from_mins(10)).await;
    assert_eq!(fixture.handle.status().model.active.max_tokens, 1536);
    assert_eq!(
        *fixture.records.log.lock().unwrap(),
        ["set_length 1536 by 7"],
        "no revert after a start that was ready"
    );
}

#[tokio::test(start_paused = true)]
async fn each_file_keeps_its_own_length_through_switch_switch_back_and_a_retrial() {
    let fixture = fixture([], CONFIG);
    ready(&fixture.assistant).await;
    // Granite gets 1,536; the trial of the other file shows what it will use: the default.
    assert_eq!(
        fixture.handle.set_length(7, 1536).await,
        SetLength::Accepted
    );
    ready_at(&fixture, 2).await;
    let report = passed(fixture.try_and_wait("other.gguf").await);
    assert_eq!(report.model.max_tokens, 1024);
    assert_eq!(
        report.budgets.max_tokens, 1024,
        "the trial measures the default"
    );
    fixture.handle.switch(7, &report.key);
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        handle.status().model.active.spec == "local:other.gguf"
            && handle.status().state == State::Ready
    })
    .await;
    let status = fixture.handle.status();
    assert_eq!(status.model.active.max_tokens, 1024);
    assert_eq!(status.model.previous.unwrap().max_tokens, 1536);
    assert_eq!(fixture.assistant.budgets().unwrap().max_tokens, 1024);
    // The new active file gets its own length; Granite's stays.
    let generation = fixture.handle.status().generation;
    assert_eq!(
        fixture.handle.set_length(7, 2048).await,
        SetLength::Accepted
    );
    ready_at(&fixture, generation + 1).await;
    assert_eq!(stored_length(&fixture, DEFAULT_MODEL_SHA256), Some(1536));
    assert_eq!(stored_length(&fixture, &sha('b')), Some(2048));
    // Switch back serves Granite with its own length; and again with the other file's.
    let generation = fixture.handle.status().generation;
    assert_eq!(fixture.handle.switch_back(7), Chosen::Accepted);
    ready_at(&fixture, generation + 1).await;
    assert_eq!(fixture.handle.status().model.active.spec, granite().spec);
    assert_eq!(fixture.handle.status().model.active.max_tokens, 1536);
    assert_eq!(fixture.assistant.budgets().unwrap().max_tokens, 1536);
    assert_eq!(
        fixture
            .launcher
            .selected
            .lock()
            .unwrap()
            .last()
            .map(|m| m.max_tokens),
        Some(1536)
    );
    let generation = fixture.handle.status().generation;
    assert_eq!(fixture.handle.switch_back(7), Chosen::Accepted);
    ready_at(&fixture, generation + 1).await;
    assert_eq!(fixture.handle.status().model.active.max_tokens, 2048);
    assert_eq!(fixture.assistant.budgets().unwrap().max_tokens, 2048);
    // A re-trial of the active file shows and keeps the length it has; its phrases still run
    // at the default.
    let report = passed(fixture.try_and_wait("other.gguf").await);
    assert_eq!(report.model.max_tokens, 2048);
    assert_eq!(report.budgets.max_tokens, 1024);
    fixture.handle.switch(7, &report.key);
    let handle = fixture.handle.clone();
    let generation = fixture.handle.status().generation;
    until(Duration::from_secs(600), move || {
        handle.status().generation > generation && handle.status().state == State::Ready
    })
    .await;
    assert_eq!(fixture.handle.status().model.active.max_tokens, 2048);
    assert_eq!(fixture.assistant.budgets().unwrap().max_tokens, 2048);
    assert_eq!(stored_length(&fixture, &sha('b')), Some(2048));
    assert_eq!(stored_length(&fixture, DEFAULT_MODEL_SHA256), Some(1536));
}

#[tokio::test(start_paused = true)]
async fn a_length_stored_for_another_file_than_the_active_one_changes_nothing_and_reverts_nothing()
{
    // The handle stored the length for the file it read as active (say the 9B); by the time the
    // command arrives another file runs (a GPU fallback to Granite). The row is the 9B's: the
    // active model's length stays, nothing is reverted, and no revert writes the 9B's earlier
    // value into Granite's row. The restart that follows is slow enough that an applied
    // 2,560 would have been reverted.
    let fixture = fixture([Start::Ok(Behaviour::default()), slow_at_0_3()], CONFIG);
    ready(&fixture.assistant).await;
    fixture
        .records
        .lengths
        .lock()
        .unwrap()
        .insert(DEFAULT_MODEL_SHA256.to_owned(), 1280);
    fixture.handle.status();
    let _ = fixture
        .handle
        .commands
        .send(Command::Model(ModelCommand::SetLength {
            max_tokens: 2560,
            by: 7,
            sha256: sha('a'),
            from: Some(1536),
        }));
    ready_at(&fixture, 2).await;
    let status = fixture.handle.status();
    assert_eq!(status.model.active.max_tokens, 1024);
    assert_eq!(status.model.note, None);
    assert!(
        fixture.records.log.lock().unwrap().is_empty(),
        "nothing written"
    );
    assert_eq!(stored_length(&fixture, DEFAULT_MODEL_SHA256), Some(1280));
    assert_eq!(stored_length(&fixture, &sha('a')), None);
    assert_eq!(fixture.assistant.budgets().unwrap().max_tokens, 1024);
}

#[tokio::test(start_paused = true)]
async fn the_session_a_length_change_starts_stays_being_carried_out_even_if_ready_cleared_the_flag()
{
    // The change is admitted while the first start is still going and its store is slow: the
    // manager becomes ready meanwhile, which clears `restarting`. The session the command
    // starts is gated; during it a second length change and a Restart are still refused.
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let fixture = fixture(
        [
            Start::Ok(Behaviour::default()),
            Start::Ok(Behaviour {
                gate: Some(Arc::clone(&gate)),
                ..Behaviour::default()
            }),
        ],
        CONFIG,
    );
    let store_gate = Arc::new(tokio::sync::Semaphore::new(0));
    *fixture.records.set_gate.lock().unwrap() = Some(Arc::clone(&store_gate));
    let handle = fixture.handle.clone();
    let first = tokio::spawn(async move { handle.set_length(7, 1536).await });
    ready(&fixture.assistant).await;
    assert!(!fixture.handle.status().restarting, "ready cleared it");
    store_gate.add_permits(1);
    assert_eq!(first.await.unwrap(), SetLength::Accepted);
    let handle = fixture.handle.clone();
    until(Duration::from_secs(600), move || {
        handle.status().generation >= 2
    })
    .await;
    assert!(fixture.handle.status().restarting);
    assert_eq!(fixture.handle.set_length(7, 2048).await, SetLength::Pending);
    assert_eq!(fixture.handle.restart(), Restart::AlreadyRunning);
    gate.add_permits(1);
    ready_at(&fixture, 2).await;
    assert_eq!(fixture.handle.status().model.active.max_tokens, 1536);
}

#[tokio::test(start_paused = true)]
async fn a_revert_that_cannot_be_stored_keeps_the_tried_length_and_says_so() {
    let fixture = fixture(
        [
            Start::Ok(Behaviour::default()),
            slow_at_0_3(),
            Start::Ok(Behaviour::default()),
        ],
        CONFIG,
    );
    ready(&fixture.assistant).await;
    assert_eq!(
        fixture.handle.set_length(7, 2560).await,
        SetLength::Accepted
    );
    // The store worked for the change; it fails for the revert.
    *fixture.records.fail.lock().unwrap() = Some("disk full".into());
    ready_at(&fixture, 3).await;
    let status = fixture.handle.status();
    assert_eq!(
        status.model.active.max_tokens, 2560,
        "the tried length stays"
    );
    let note = status.model.note.unwrap();
    assert!(
        note.contains("2560")
            && note.contains("too slow")
            && note.contains("could not be restored")
            && note.contains("disk full"),
        "{note}"
    );
    assert_eq!(stored_length(&fixture, DEFAULT_MODEL_SHA256), Some(2560));
    // The session started again (without resetting the breaker) with what is stored.
    assert_eq!(fixture.launcher.starts.lock().unwrap().len(), 3);
    assert_eq!(fixture.assistant.budgets().unwrap().max_tokens, 2560);
    assert_eq!(resets(&fixture), 1);
}
