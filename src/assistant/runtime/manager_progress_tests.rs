//! The steps the page follows (`docs/model-runtime.md`, "Live progress"): every entry point
//! walks its list in order with exact step and step count, the label of a running trial is
//! the progress's own, deadlines give "at most" and "last time" comes only from a successful
//! run of the same file, path and kind. A fake launcher, fake children and Tokio's paused
//! clock; slow steps are given a delay so a sampler can see them.
use super::super::progress::StartEvent;
use super::tests::{Behaviour, CONFIG, FakeLauncher, Start, manager, ready, until};
use super::*;
use crate::assistant::DEFAULT_MODEL_SHA256;
use crate::assistant::records::tests::MemRecords;
use crate::assistant::runtime::models::{ModelFile, OlderPrompt};

type Seen = (usize, usize, Phase, Option<Qualifier>);

fn delayed() -> Behaviour {
    Behaviour {
        prime_delay: Duration::from_millis(50),
        tokenize_delay: Duration::from_millis(50),
        ..Behaviour::default()
    }
}

/// A launcher whose slow steps last long enough to be sampled.
fn launcher(script: impl IntoIterator<Item = Start>) -> Arc<FakeLauncher> {
    let launcher = FakeLauncher::scripted(script);
    *launcher.preflight_delay.lock().unwrap() = Some(Duration::from_millis(50));
    *launcher.start_delay.lock().unwrap() = Some(Duration::from_millis(50));
    *launcher.verify_delay.lock().unwrap() = Some(Duration::from_millis(50));
    *launcher.hash_delay.lock().unwrap() = Some(Duration::from_millis(50));
    launcher
}

fn progress_of(status: &Status) -> Option<&Progress> {
    status.state.starting()
}

/// Samples the status every 5 ms of virtual time until `done`, recording each new step. A
/// running trial must show the progress's own phase.
async fn walk(handle: &Handle, mut done: impl FnMut(&Status) -> bool) -> Vec<Seen> {
    let mut seen: Vec<Seen> = Vec::new();
    let end = Instant::now() + Duration::from_hours(2);
    loop {
        let status = handle.status();
        if let Some(progress) = progress_of(&status) {
            if let Some(TrialState::Running { phase, .. }) = &status.trial {
                assert_eq!(*phase, progress.phase, "the trial and the progress agree");
            }
            let item = (
                progress.step(),
                progress.steps(),
                progress.phase,
                progress.qualifier,
            );
            if seen.last() != Some(&item) {
                seen.push(item);
            }
        }
        if done(&status) {
            return seen;
        }
        assert!(Instant::now() < end, "not done: {seen:?}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn is_ready(status: &Status) -> bool {
    status.state == State::Ready && !status.restarting
}

fn fresh(steps: usize, from: usize, qualifier: Option<Qualifier>) -> Vec<Seen> {
    let phases = [
        Phase::Stopping,
        Phase::Checking,
        Phase::StartingServer,
        Phase::Verifying,
        Phase::Loading,
        Phase::Priming,
        Phase::Sample(1),
        Phase::Sample(2),
        Phase::Reminders,
    ];
    let phases = &phases[phases.len() - steps..];
    // A CPU start with reminders off does not take the reminder prime, from its first step.
    phases
        .iter()
        .enumerate()
        .skip(from)
        .map(|(at, phase)| (at + 1, steps, *phase, qualifier))
        .collect()
}

fn retry(qualifier: Option<Qualifier>) -> Vec<Seen> {
    [
        Phase::StartingServer,
        Phase::Verifying,
        Phase::Loading,
        Phase::Priming,
        Phase::Sample(1),
        Phase::Sample(2),
        Phase::Reminders,
    ]
    .iter()
    .enumerate()
    .map(|(at, phase)| (at + 1, 7, *phase, qualifier))
    .collect()
}

#[tokio::test(start_paused = true)]
async fn a_fresh_start_walks_its_steps_in_order() {
    let launcher = launcher([Start::Ok(delayed())]);
    let (_assistant, handle) = manager(&launcher);
    let seen = walk(&handle, is_ready).await;
    assert_eq!(seen, fresh(8, 0, None));
    handle.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn the_bytes_hashed_and_a_remembered_hash_show_in_the_verifying_step() {
    for cached in [false, true] {
        let launcher = launcher([Start::Ok(delayed())]);
        *launcher.verify_cached.lock().unwrap() = cached;
        let (_assistant, handle) = manager(&launcher);
        let probe = handle.clone();
        until(Duration::from_secs(60), move || {
            probe
                .status()
                .state
                .starting()
                .is_some_and(|p| p.phase == Phase::Verifying)
        })
        .await;
        let status = handle.status();
        let progress = progress_of(&status).unwrap();
        if cached {
            assert_eq!(progress.bytes, None);
            // The remembered hash held: nothing was read, and it is said at once.
            let probe = handle.clone();
            until(Duration::from_secs(60), move || {
                probe
                    .status()
                    .state
                    .starting()
                    .is_some_and(|p| p.detail == Some("verified (unchanged)"))
            })
            .await;
        } else {
            assert_eq!(progress.bytes, Some((1 << 20, 4 << 20)));
            assert!(progress.time_line().contains("read 1 of 4 MiB"));
        }
        ready_state(&handle).await;
        handle.shutdown().await;
    }
}

async fn ready_state(handle: &Handle) {
    let probe = handle.clone();
    until(Duration::from_mins(30), move || is_ready(&probe.status())).await;
}

#[tokio::test(start_paused = true)]
async fn a_restart_while_ready_starts_with_the_stopping_step() {
    let launcher = launcher([Start::Ok(delayed()), Start::Ok(delayed())]);
    let (assistant, handle) = manager(&launcher);
    ready(&assistant).await;
    ready_state(&handle).await;
    assert_eq!(handle.restart(), Restart::Accepted);
    // At once, before the manager has taken the command.
    let seen = walk(&handle, is_ready).await;
    assert_eq!(seen, fresh(9, 0, None));
    handle.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_restart_while_unavailable_has_no_stopping_step() {
    let launcher = launcher([Start::Sandbox("refused"), Start::Ok(delayed())]);
    let (_assistant, handle) = manager(&launcher);
    super::tests::unavailable(&handle).await;
    assert_eq!(handle.restart(), Restart::Accepted);
    let seen = walk(&handle, is_ready).await;
    assert_eq!(seen, fresh(8, 0, None));
    handle.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_failed_attempt_waits_with_its_exact_time_left_then_walks_the_retry_list() {
    let launcher = launcher([Start::Other("boom"), Start::Ok(delayed())]);
    let (_assistant, handle) = manager(&launcher);
    let probe = handle.clone();
    until(Duration::from_secs(60), move || {
        probe
            .status()
            .state
            .starting()
            .is_some_and(|p| p.phase == Phase::WaitingRetry)
    })
    .await;
    let status = handle.status();
    let waiting = progress_of(&status).unwrap();
    assert_eq!(
        waiting.describe(),
        "waiting to retry (attempt 2 of up to 4)"
    );
    assert!(
        waiting.time_line().contains("2 min left"),
        "{}",
        waiting.time_line()
    );
    tokio::time::sleep(Duration::from_secs(30)).await;
    let status = handle.status();
    let line = progress_of(&status).unwrap().time_line();
    assert!(line.contains("1 min 30 s left"), "{line}");
    let seen = walk(&handle, is_ready).await;
    let mut expected = vec![(1, 1, Phase::WaitingRetry, Some(Qualifier::Attempt(2)))];
    expected.extend(retry(Some(Qualifier::Attempt(2))));
    assert_eq!(seen, expected);
    handle.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_restart_after_a_crash_walks_the_retry_list_labelled_with_the_counted_attempt() {
    let launcher = launcher([Start::Ok(delayed()), Start::Ok(delayed())]);
    let (assistant, handle) = manager(&launcher);
    ready(&assistant).await;
    ready_state(&handle).await;
    launcher.child(0).crash();
    let seen = walk(&handle, |status| is_ready(status) && status.generation == 2).await;
    assert_eq!(seen, retry(Some(Qualifier::Attempt(2))));
    handle.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn the_cpu_after_a_gpu_failure_walks_the_fresh_list_from_checking_the_host() {
    let launcher = launcher([Start::Other("the GPU died"), Start::Ok(delayed())]);
    *launcher.device.lock().unwrap() = Some(DeviceView {
        pci: "0000:0c:00.0".into(),
        node: "renderD128".into(),
        name: "Vulkan0".into(),
        description: "a GPU".into(),
        total_mib: 8176,
    });
    let gate = Arc::new(tokio::sync::Notify::new());
    *launcher.cpu_probe_gate.lock().unwrap() = Some(Arc::clone(&gate));
    let assistant = Assistant::new();
    let handle = spawn(
        Arc::clone(&assistant),
        Arc::clone(&launcher) as Arc<dyn Launcher>,
        Config {
            path: Path::Gpu,
            levels: USED_LEVELS,
        },
    );
    let probe = handle.clone();
    until(Duration::from_secs(60), move || {
        probe
            .status()
            .state
            .starting()
            .is_some_and(|p| p.qualifier == Some(Qualifier::GpuFallback))
    })
    .await;
    let status = handle.status();
    let held = progress_of(&status).unwrap();
    assert_eq!(
        (held.step(), held.steps(), held.phase),
        (1, 8, Phase::Checking)
    );
    assert_eq!(
        held.describe(),
        "step 1 of 8, checking the host (on the CPU after a GPU failure)"
    );
    gate.notify_one();
    let seen = walk(&handle, is_ready).await;
    assert_eq!(seen, fresh(8, 0, Some(Qualifier::GpuFallback)));
    handle.shutdown().await;
}

fn slow_at_0_3() -> Behaviour {
    Behaviour {
        g: 0.3,
        ..Behaviour::default()
    }
}

#[tokio::test(start_paused = true)]
async fn a_reply_length_change_stops_the_server_and_a_revert_says_it_went_back() {
    let launcher = launcher([
        Start::Ok(delayed()),
        Start::Ok(slow_at_0_3()),
        Start::Ok(delayed()),
    ]);
    let assistant = Assistant::new();
    let records = Arc::new(MemRecords::default());
    let handle = spawn_with(
        Arc::clone(&assistant),
        Arc::clone(&launcher) as Arc<dyn Launcher>,
        CONFIG,
        ModelSetup {
            active: Model::test("granite-4.2-3b-Q4_K_M.gguf", DEFAULT_MODEL_SHA256, 1024),
            previous: None,
            records: Some(records as Arc<dyn Records>),
            from_owner_setting: false,
            device_default: false,
            older_prompt: OlderPrompt::default(),
        },
    );
    ready(&assistant).await;
    ready_state(&handle).await;
    assert_eq!(handle.set_length(7, 2560).await, SetLength::Accepted);
    let seen = walk(&handle, |status| is_ready(status) && status.generation == 3).await;
    // The change's first start is too slow at 2,560 tokens: back to the earlier length, the
    // fresh list again (no stop) with the reason.
    // The change itself walked the list with the stop step.
    assert!(seen.contains(&(2, 9, Phase::Checking, None)), "{seen:?}");
    let again: Vec<Seen> = seen
        .iter()
        .copied()
        .filter(|item| item.3 == Some(Qualifier::EarlierLength))
        .collect();
    assert_eq!(again, fresh(8, 0, Some(Qualifier::EarlierLength)));
    handle.shutdown().await;
}

fn trial_fixture(
    script: impl IntoIterator<Item = Start>,
) -> (Arc<FakeLauncher>, Arc<Assistant>, Handle) {
    let launcher = launcher(script);
    launcher.files.lock().unwrap().insert(
        "other.gguf".into(),
        Ok(ModelFile {
            sha256: "b".repeat(64),
            size: 1 << 30,
        }),
    );
    let assistant = Assistant::new();
    let handle = spawn_with(
        Arc::clone(&assistant),
        Arc::clone(&launcher) as Arc<dyn Launcher>,
        CONFIG,
        ModelSetup {
            active: Model::test("granite-4.2-3b-Q4_K_M.gguf", DEFAULT_MODEL_SHA256, 1024),
            previous: None,
            records: Some(Arc::new(MemRecords::default()) as Arc<dyn Records>),
            from_owner_setting: false,
            device_default: false,
            older_prompt: OlderPrompt::default(),
        },
    );
    (launcher, assistant, handle)
}

fn trial_done(status: &Status) -> bool {
    status.trial.as_ref().is_some_and(|trial| !trial.running()) && is_ready(status)
}

#[tokio::test(start_paused = true)]
async fn a_trial_walks_its_steps_per_sample_and_per_phrase_then_the_previous_model_starts() {
    let (_launcher, assistant, handle) = trial_fixture([
        Start::Ok(delayed()),
        Start::Ok(delayed()),
        Start::Ok(delayed()),
    ]);
    ready(&assistant).await;
    ready_state(&handle).await;
    assert!(matches!(
        handle.try_model(7, "local:other.gguf".into(), "other.gguf".into()),
        TryModel::Accepted(1)
    ));
    let seen = walk(&handle, trial_done).await;
    let trial: Vec<Phase> = [
        Phase::WaitingStop,
        Phase::Hashing,
        Phase::Checking,
        Phase::StartingServer,
        Phase::Loading,
        Phase::Priming,
        Phase::Sample(1),
        Phase::Sample(2),
        Phase::Phrase(1),
        Phase::Phrase(2),
        Phase::Phrase(3),
        Phase::Phrase(4),
        Phase::Phrase(5),
    ]
    .to_vec();
    let expected: Vec<Seen> = trial
        .iter()
        .enumerate()
        .map(|(at, phase)| (at + 1, 13, *phase, None))
        .collect();
    assert_eq!(&seen[..13], expected.as_slice());
    // The previous model's fresh session is its own progress, from the end of the trial's
    // candidate: the stop step first.
    // The stop itself takes no time with a fake server, so no sample sees it.
    assert_eq!(&seen[13..], fresh(9, 1, None).as_slice());
    handle.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn hashing_bytes_show_in_a_trials_hashing_step() {
    let (launcher, assistant, handle) = trial_fixture([Start::Ok(delayed())]);
    *launcher.hash_delay.lock().unwrap() = Some(Duration::from_secs(10));
    ready(&assistant).await;
    ready_state(&handle).await;
    assert!(matches!(
        handle.try_model(7, "local:other.gguf".into(), "other.gguf".into()),
        TryModel::Accepted(1)
    ));
    let probe = handle.clone();
    until(Duration::from_secs(60), move || {
        probe
            .status()
            .state
            .starting()
            .is_some_and(|p| p.phase == Phase::Hashing)
    })
    .await;
    let observer = launcher.observer.lock().unwrap().clone().unwrap();
    observer(StartEvent::Hashed {
        done: 5 << 20,
        total: 1 << 30,
    });
    let status = handle.status();
    let progress = progress_of(&status).unwrap();
    assert_eq!(progress.bytes, Some((5 << 20, 1 << 30)));
    assert!(progress.time_line().contains("read 5 of 1024 MiB"));
    // A report outside the steps that read the file changes nothing.
    ready_state(&handle).await;
    observer(StartEvent::Hashed { done: 1, total: 2 });
    assert_eq!(handle.status().state, State::Ready);
    handle.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_first_run_gives_deadlines_only_and_the_next_run_of_the_same_file_adds_last_time() {
    let launcher = launcher([Start::Ok(delayed()), Start::Ok(delayed())]);
    let (assistant, handle) = manager(&launcher);
    let probe = handle.clone();
    until(Duration::from_secs(60), move || {
        probe
            .status()
            .state
            .starting()
            .is_some_and(|p| p.phase == Phase::Loading)
    })
    .await;
    let status = handle.status();
    let first = progress_of(&status).unwrap().time_line();
    // `/health` 60 s and the 5 s `loaded()` lets the readers settle.
    assert!(
        first.contains("at most 1 min 5 s left in this step"),
        "{first}"
    );
    assert!(!first.contains("last time"), "{first}");
    ready(&assistant).await;
    ready_state(&handle).await;
    assert_eq!(handle.restart(), Restart::Accepted);
    let probe = handle.clone();
    until(Duration::from_secs(60), move || {
        probe
            .status()
            .state
            .starting()
            .is_some_and(|p| p.phase == Phase::Loading)
    })
    .await;
    let status = handle.status();
    let second = progress_of(&status).unwrap().time_line();
    assert!(second.contains("last time"), "{second}");
    handle.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_failed_attempts_partial_times_are_never_last_time() {
    // The first start gets as far as priming and fails; a Restart then reaches ready. Had the
    // failed attempt's loading time been kept, the Restart's loading step would show it.
    let failing = Behaviour {
        prime_fails: true,
        ..delayed()
    };
    let launcher = launcher([Start::Ok(failing), Start::Ok(delayed())]);
    let (_assistant, handle) = manager(&launcher);
    let probe = handle.clone();
    until(Duration::from_mins(5), move || {
        probe
            .status()
            .state
            .starting()
            .is_some_and(|p| p.phase == Phase::WaitingRetry)
    })
    .await;
    assert_eq!(handle.restart(), Restart::Accepted);
    let probe = handle.clone();
    until(Duration::from_mins(5), move || {
        probe
            .status()
            .state
            .starting()
            .is_some_and(|p| p.phase == Phase::Loading)
    })
    .await;
    let line = handle.status().state.starting().unwrap().time_line();
    assert!(!line.contains("last time"), "{line}");
    ready_state(&handle).await;
    handle.shutdown().await;
}

const TWO_LEVELS: &[Level] = &[Level::Sandbox, Level::Unconfined];

#[tokio::test(start_paused = true)]
async fn only_counted_failures_number_an_attempt() {
    // A level skipped after an uncounted sandbox failure walks the retry list unnumbered.
    let first = launcher([Start::Sandbox("refused"), Start::Ok(delayed())]);
    let assistant = Assistant::new();
    let handle = spawn(
        Arc::clone(&assistant),
        Arc::clone(&first) as Arc<dyn Launcher>,
        Config {
            path: Path::Cpu,
            levels: TWO_LEVELS,
        },
    );
    let seen = walk(&handle, is_ready).await;
    let retried: Vec<Seen> = seen.iter().copied().filter(|s| s.1 == 7).collect();
    assert_eq!(retried, retry(None));
    handle.shutdown().await;
    // A crash after the ready window is not counted either.
    let second = launcher([Start::Ok(delayed()), Start::Ok(delayed())]);
    let (assistant, handle) = manager(&second);
    ready(&assistant).await;
    ready_state(&handle).await;
    tokio::time::sleep(READY_WINDOW + Duration::from_secs(1)).await;
    second.child(0).crash();
    let seen = walk(&handle, |status| is_ready(status) && status.generation == 2).await;
    assert_eq!(seen, retry(None));
    handle.shutdown().await;
}

fn stopping_now(handle: &Handle) -> bool {
    handle
        .status()
        .state
        .starting()
        .is_some_and(|p| p.kind == (Kind::Fresh { stopping: true }) && p.phase == Phase::Stopping)
}

#[tokio::test(start_paused = true)]
async fn a_restart_while_starting_and_the_end_of_a_trial_show_the_stop_not_a_stale_step() {
    // A Restart while the server is priming.
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let gated = Behaviour {
        gate: Some(Arc::clone(&gate)),
        ..delayed()
    };
    let launcher = launcher([Start::Ok(gated), Start::Ok(delayed())]);
    let (_assistant, handle) = manager(&launcher);
    let probe = handle.clone();
    until(Duration::from_secs(60), move || {
        probe
            .status()
            .state
            .starting()
            .is_some_and(|p| p.phase == Phase::Priming)
    })
    .await;
    let release = Arc::new(tokio::sync::Notify::new());
    *launcher.unsettled.lock().unwrap() = Some(Arc::clone(&release));
    assert_eq!(handle.restart(), Restart::Accepted);
    let probe = handle.clone();
    until(Duration::from_secs(60), move || stopping_now(&probe)).await;
    let status = handle.status();
    let p = progress_of(&status).unwrap();
    assert_eq!((p.step(), p.steps()), (1, 9));
    release.notify_one();
    *launcher.unsettled.lock().unwrap() = None;
    let seen = walk(&handle, is_ready).await;
    assert_eq!(
        seen[..2],
        [(1, 9, Phase::Stopping, None), (2, 9, Phase::Checking, None)]
    );
    handle.shutdown().await;

    // The end of a trial: its candidate is stopped, the previous model's start follows.
    let (launcher, assistant, handle) = trial_fixture([
        Start::Ok(delayed()),
        Start::Ok(delayed()),
        Start::Ok(delayed()),
    ]);
    ready(&assistant).await;
    ready_state(&handle).await;
    let release = Arc::new(tokio::sync::Notify::new());
    *launcher.unsettled.lock().unwrap() = Some(Arc::clone(&release));
    // The trial's own settle passes; the session after it waits.
    release.notify_one();
    assert!(matches!(
        handle.try_model(7, "local:other.gguf".into(), "other.gguf".into()),
        TryModel::Accepted(1)
    ));
    let probe = handle.clone();
    until(Duration::from_hours(1), move || stopping_now(&probe)).await;
    assert!(!handle.status().trial.unwrap().running());
    release.notify_one();
    *launcher.unsettled.lock().unwrap() = None;
    ready_state(&handle).await;
    handle.shutdown().await;
}

async fn assert_stopping_then_checking(handle: &Handle) {
    let status = handle.status();
    let p = progress_of(&status).unwrap();
    assert_eq!(
        (p.step(), p.steps(), p.phase, p.kind),
        (1, 9, Phase::Stopping, Kind::Fresh { stopping: true })
    );
    let seen = walk(handle, is_ready).await;
    assert_eq!(seen[0], (1, 9, Phase::Stopping, None));
    assert_eq!(
        seen[1],
        (2, 9, Phase::Checking, None),
        "no second stop step"
    );
}

#[tokio::test(start_paused = true)]
async fn switch_switch_back_and_try_the_gpu_again_start_with_one_stopping_step() {
    let (_launcher, assistant, handle) = trial_fixture([
        Start::Ok(delayed()),
        Start::Ok(delayed()),
        Start::Ok(delayed()),
        Start::Ok(delayed()),
        Start::Ok(delayed()),
    ]);
    ready(&assistant).await;
    ready_state(&handle).await;
    assert!(matches!(
        handle.try_model(7, "local:other.gguf".into(), "other.gguf".into()),
        TryModel::Accepted(1)
    ));
    let probe = handle.clone();
    until(Duration::from_hours(1), move || trial_done(&probe.status())).await;
    let key = match handle.status().trial {
        Some(TrialState::Passed(report)) => report.key,
        other => panic!("{other:?}"),
    };
    assert_eq!(handle.switch(7, &key), Chosen::Accepted);
    assert_stopping_then_checking(&handle).await;
    assert_eq!(handle.switch_back(7), Chosen::Accepted);
    assert_stopping_then_checking(&handle).await;
    handle.shutdown().await;

    // Try the GPU again, after a GPU failure left the CPU running.
    let launcher = launcher([
        Start::Other("the GPU died"),
        Start::Ok(delayed()),
        Start::Ok(delayed()),
    ]);
    *launcher.device.lock().unwrap() = Some(DeviceView {
        pci: "0000:0c:00.0".into(),
        node: "renderD128".into(),
        name: "Vulkan0".into(),
        description: "a GPU".into(),
        total_mib: 8176,
    });
    let handle = spawn(
        Assistant::new(),
        Arc::clone(&launcher) as Arc<dyn Launcher>,
        Config {
            path: Path::Gpu,
            levels: USED_LEVELS,
        },
    );
    ready_state(&handle).await;
    assert_eq!(handle.try_gpu(), TryGpu::Accepted);
    assert_stopping_then_checking(&handle).await;
    handle.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_trial_shows_its_deadlines_and_last_time_only_after_a_passed_trial_of_the_file() {
    let (_launcher, assistant, handle) = trial_fixture([
        Start::Ok(delayed()),
        Start::Ok(delayed()),
        Start::Ok(delayed()),
        Start::Ok(delayed()),
        Start::Ok(delayed()),
    ]);
    ready(&assistant).await;
    ready_state(&handle).await;
    let mut lines = Vec::new();
    for _ in 0..2 {
        assert!(matches!(
            handle.try_model(7, "local:other.gguf".into(), "other.gguf".into()),
            TryModel::Accepted(_)
        ));
        let probe = handle.clone();
        until(Duration::from_hours(1), move || {
            probe
                .status()
                .state
                .starting()
                .is_some_and(|p| p.phase == Phase::Phrase(1))
        })
        .await;
        lines.push(handle.status().state.starting().unwrap().time_line());
        let probe = handle.clone();
        until(Duration::from_hours(1), move || trial_done(&probe.status())).await;
        // Dismiss the result so another trial is admitted.
        handle.keep();
    }
    let [first, second] = lines.as_slice() else {
        unreachable!()
    };
    assert!(first.contains("the trial ends in at most"), "{first}");
    assert!(first.contains("left in this step"), "{first}");
    assert!(!first.contains("last time"), "{first}");
    assert!(second.contains("last time"), "{second}");
    handle.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_trial_cancelled_while_hashing_has_no_stop_step_and_a_ran_candidate_does() {
    let (launcher, assistant, handle) = trial_fixture([Start::Ok(delayed()), Start::Ok(delayed())]);
    *launcher.hash_delay.lock().unwrap() = Some(Duration::from_secs(600));
    ready(&assistant).await;
    ready_state(&handle).await;
    assert!(matches!(
        handle.try_model(7, "local:other.gguf".into(), "other.gguf".into()),
        TryModel::Accepted(1)
    ));
    let probe = handle.clone();
    until(Duration::from_secs(60), move || {
        probe
            .status()
            .state
            .starting()
            .is_some_and(|p| p.phase == Phase::Hashing)
    })
    .await;
    let release = Arc::new(tokio::sync::Notify::new());
    *launcher.unsettled.lock().unwrap() = Some(Arc::clone(&release));
    // The cancel's own settle passes; the session after it waits.
    release.notify_one();
    assert_eq!(handle.cancel_trial(1), CancelTrial::Done);
    let probe = handle.clone();
    until(Duration::from_secs(60), move || {
        probe
            .status()
            .state
            .starting()
            .is_some_and(|p| p.kind != Kind::Trial)
    })
    .await;
    let status = handle.status();
    let held = progress_of(&status).unwrap();
    // Nothing ran: no model server to stop.
    assert_eq!(
        (held.step(), held.steps(), held.phase, held.kind),
        (1, 8, Phase::Checking, Kind::Fresh { stopping: false })
    );
    release.notify_one();
    *launcher.unsettled.lock().unwrap() = None;
    ready_state(&handle).await;
    handle.shutdown().await;
}
