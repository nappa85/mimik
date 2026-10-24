//! The transitions table, one test per row, with a fake launcher, fake children and Tokio's
//! paused clock.
use super::super::server::Timings;
use super::*;
use crate::assistant::interpreter::stub::StubInterpreter;
use crate::assistant::interpreter::{Failure, Interpreter};
use crate::assistant::jobs::RuntimeState;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::watch;

/// How a fake server answers.
#[derive(Clone)]
struct Behaviour {
    /// `/health` answers before it is ready.
    loading: usize,
    prime_fails: bool,
    /// Per-token prompt and generation seconds of the samples.
    p: f64,
    g: f64,
    /// Holds priming until a permit is added.
    gate: Option<Arc<tokio::sync::Semaphore>>,
    /// What `loaded` answers (the GPU offload check).
    loaded: Result<(), String>,
    landlock: Option<super::super::hako::Landlock>,
    /// A malfunction of the fake server, if any.
    fault: Fault,
}

/// What a fake server gets wrong.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    /// `/health` is refused by the endpoint check.
    RefuseHealth,
    /// `/tokenize` answers HTTP 500.
    TokenizeFails,
    /// The samples come back without their prompt timing.
    MissingTiming,
}

impl Default for Behaviour {
    fn default() -> Self {
        Self {
            loading: 2,
            prime_fails: false,
            p: 0.001,
            g: 0.01,
            gate: None,
            loaded: Ok(()),
            landlock: None,
            fault: Fault::None,
        }
    }
}

struct FakeServer {
    behaviour: Behaviour,
    health_calls: AtomicUsize,
    primed: Arc<AtomicUsize>,
    interpreter: Arc<StubInterpreter>,
}

#[async_trait]
impl Server for FakeServer {
    async fn health(&self) -> Health {
        if self.behaviour.fault == Fault::RefuseHealth {
            return Health::Refused("the endpoint check refused the connection".into());
        }
        if self.health_calls.fetch_add(1, Ordering::SeqCst) < self.behaviour.loading {
            Health::Loading
        } else {
            Health::Ready
        }
    }

    async fn tokens(&self, text: &str) -> Result<usize, Failure> {
        if self.behaviour.fault == Fault::TokenizeFails {
            return Err(Failure::Status(500));
        }
        Ok(text.len().div_ceil(3))
    }

    async fn plain(&self, user: &str, max_tokens: u32, _: bool) -> Result<Timings, Failure> {
        if max_tokens == 1 {
            if let Some(gate) = &self.behaviour.gate {
                gate.acquire().await.unwrap().forget();
            }
            if self.behaviour.prime_fails {
                return Err(Failure::Status(500));
            }
            self.primed.fetch_add(1, Ordering::SeqCst);
            return Ok(Timings {
                prompt_n: Some(1460.0),
                prompt_ms: Some(1460.0),
                cache_n: Some(0.0),
                predicted_n: Some(1.0),
                predicted_ms: Some(1.0),
            });
        }
        let n = f64::from(u32::try_from(user.len().div_ceil(3)).unwrap());
        let prompt_n = 200.0 + n;
        let predicted = f64::from(max_tokens);
        let prompt_ms = self.behaviour.p * prompt_n * 1000.0;
        let predicted_ms = self.behaviour.g * predicted * 1000.0;
        tokio::time::sleep(Duration::from_secs_f64((prompt_ms + predicted_ms) / 1000.0)).await;
        Ok(Timings {
            prompt_n: Some(prompt_n),
            prompt_ms: (self.behaviour.fault != Fault::MissingTiming).then_some(prompt_ms),
            cache_n: Some(1290.0),
            predicted_n: Some(predicted),
            predicted_ms: Some(predicted_ms),
        })
    }

    fn interpreter(&self) -> Arc<dyn Interpreter> {
        Arc::clone(&self.interpreter) as Arc<dyn Interpreter>
    }
}

struct FakeChild {
    server: Arc<FakeServer>,
    exit: watch::Sender<Option<Fail>>,
    stopped: AtomicBool,
    alive: Arc<AtomicUsize>,
    stops: Arc<AtomicUsize>,
}

#[async_trait]
impl Child for FakeChild {
    fn landlock(&self) -> Option<super::super::hako::Landlock> {
        self.server.behaviour.landlock
    }

    fn server(&self) -> Arc<dyn Server> {
        Arc::clone(&self.server) as Arc<dyn Server>
    }

    async fn exited(&self) -> Fail {
        let mut exit = self.exit.subscribe();
        loop {
            if let Some(fail) = exit.borrow_and_update().clone() {
                return fail;
            }
            if exit.changed().await.is_err() {
                return std::future::pending().await;
            }
        }
    }

    async fn stop(&self) {
        if !self.stopped.swap(true, Ordering::SeqCst) {
            self.stops.fetch_add(1, Ordering::SeqCst);
            self.gone();
        }
    }

    async fn loaded(&self) -> Result<(), String> {
        self.server.behaviour.loaded.clone()
    }
}

impl FakeChild {
    fn gone(&self) {
        let already = self.exit.borrow().is_some();
        if !already {
            self.alive.fetch_sub(1, Ordering::SeqCst);
            let _ = self.exit.send(Some(Fail::Other("exited".into())));
        }
    }

    /// The child exits by itself.
    fn crash(&self) {
        self.gone();
    }
}

#[derive(Clone)]
enum Start {
    Ok(Behaviour),
    Sandbox(&'static str),
    Other(&'static str),
}

type Probed = Vec<(Level, Result<(), String>)>;

#[derive(Default)]
struct FakeLauncher {
    preflight: Mutex<Option<String>>,
    preflight_delay: Mutex<Option<Duration>>,
    preflights: Mutex<usize>,
    /// While set, `settle` waits (as for a blocking start still finishing).
    unsettled: Mutex<Option<Arc<tokio::sync::Notify>>>,
    cancels: AtomicUsize,
    probe: Mutex<Option<Probed>>,
    /// The GPU path's probe (default: level 1 passes).
    gpu_probe: Mutex<Option<Probed>>,
    /// The paths probed, in order.
    probed: Mutex<Vec<Path>>,
    /// The path of each start, in order.
    paths: Mutex<Vec<Path>>,
    device: Mutex<Option<DeviceView>>,
    device_notes: Mutex<Vec<String>>,
    /// Holds the CPU probe until notified.
    cpu_probe_gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
    script: Mutex<VecDeque<Start>>,
    starts: Mutex<Vec<(u64, Level, Instant)>>,
    children: Mutex<Vec<Arc<FakeChild>>>,
    alive: Arc<AtomicUsize>,
    stops: Arc<AtomicUsize>,
    primed: Arc<AtomicUsize>,
    coexisted: AtomicBool,
}

#[async_trait]
impl Launcher for FakeLauncher {
    async fn preflight(&self, _: Path) -> Result<(), String> {
        *self.preflights.lock().unwrap() += 1;
        let delay = *self.preflight_delay.lock().unwrap();
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        match self.preflight.lock().unwrap().clone() {
            Some(why) => Err(why),
            None => Ok(()),
        }
    }

    fn cancel(&self) {
        self.cancels.fetch_add(1, Ordering::SeqCst);
    }

    async fn settle(&self) {
        let gate = self.unsettled.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.notified().await;
        }
    }

    async fn probe(&self, path: Path) -> Vec<(Level, Result<(), String>)> {
        self.probed.lock().unwrap().push(path);
        if path == Path::Gpu {
            return self
                .gpu_probe
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| vec![(Level::Bubblewrap, Ok(())), (Level::Unconfined, Ok(()))]);
        }
        let gate = self.cpu_probe_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.notified().await;
        }
        self.probe
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| vec![(Level::Bubblewrap, Ok(())), (Level::Unconfined, Ok(()))])
    }

    fn device(&self) -> Option<DeviceView> {
        self.device.lock().unwrap().clone()
    }

    fn device_notes(&self) -> Vec<String> {
        self.device_notes.lock().unwrap().clone()
    }

    async fn start(
        &self,
        generation: u64,
        level: Level,
        path: Path,
    ) -> Result<Arc<dyn Child>, Fail> {
        self.paths.lock().unwrap().push(path);
        self.starts
            .lock()
            .unwrap()
            .push((generation, level, Instant::now()));
        if self.alive.load(Ordering::SeqCst) != 0 {
            self.coexisted.store(true, Ordering::SeqCst);
        }
        let script = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Start::Ok(Behaviour::default()));
        let behaviour = match script {
            Start::Ok(behaviour) => behaviour,
            Start::Sandbox(why) => return Err(Fail::Sandbox(why.into())),
            Start::Other(why) => return Err(Fail::Other(why.into())),
        };
        self.alive.fetch_add(1, Ordering::SeqCst);
        let child = Arc::new(FakeChild {
            server: Arc::new(FakeServer {
                behaviour,
                health_calls: AtomicUsize::new(0),
                primed: Arc::clone(&self.primed),
                interpreter: Arc::new(StubInterpreter::default()),
            }),
            exit: watch::channel(None).0,
            stopped: AtomicBool::new(false),
            alive: Arc::clone(&self.alive),
            stops: Arc::clone(&self.stops),
        });
        self.children.lock().unwrap().push(Arc::clone(&child));
        Ok(child as Arc<dyn Child>)
    }
}

impl FakeLauncher {
    fn scripted(script: impl IntoIterator<Item = Start>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into_iter().collect()),
            ..Self::default()
        })
    }

    fn levels(&self) -> Vec<Level> {
        self.starts
            .lock()
            .unwrap()
            .iter()
            .map(|(_, l, _)| *l)
            .collect()
    }

    fn child(&self, n: usize) -> Arc<FakeChild> {
        Arc::clone(&self.children.lock().unwrap()[n])
    }
}

const CONFIG: Config = Config {
    path: Path::Cpu,
    max_tokens: 352,
    levels: USED_LEVELS,
};

fn manager(launcher: &Arc<FakeLauncher>) -> (Arc<Assistant>, Handle) {
    let assistant = Assistant::new();
    let handle = spawn(
        Arc::clone(&assistant),
        Arc::clone(launcher) as Arc<dyn Launcher>,
        CONFIG,
    );
    (assistant, handle)
}

/// Lets the paused clock run until `done` holds (at most `limit` of virtual time).
async fn until(limit: Duration, mut done: impl FnMut() -> bool) {
    let end = Instant::now() + limit;
    while !done() {
        assert!(Instant::now() < end, "not reached within {limit:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn ready(assistant: &Arc<Assistant>) {
    let assistant = Arc::clone(assistant);
    until(Duration::from_secs(600), move || {
        assistant.state() == RuntimeState::Ready
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn a_start_reaches_ready_after_health_priming_and_calibration() {
    let launcher = FakeLauncher::scripted([]);
    let (assistant, handle) = manager(&launcher);
    ready(&assistant).await;
    let status = handle.status();
    assert_eq!(status.state, State::Ready);
    assert_eq!(status.level, Some(Level::Bubblewrap));
    assert_eq!(status.generation, 1);
    assert_eq!(
        status.skipped,
        vec![(
            Level::Unconfined,
            "not used in this release (ASSIST-43)".to_owned()
        )]
    );
    let budgets = status.budgets.unwrap();
    assert_eq!(budgets.bounds.bytes, 1024);
    assert_eq!(budgets.run, Duration::from_secs(15));
    assert_eq!(assistant.budgets(), Some(budgets));
    assert!(status.calibration.is_some());
    assert_eq!(launcher.primed.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn a_failed_restart_never_shows_the_previous_generations_landlock_report() {
    for fail_preflight in [true, false] {
        let report = Some(super::super::hako::Landlock::Full { abi: 11 });
        let launcher = FakeLauncher::scripted([Start::Ok(Behaviour {
            landlock: report,
            ..Behaviour::default()
        })]);
        let (assistant, handle) = manager(&launcher);
        ready(&assistant).await;
        assert_eq!(handle.status().landlock, report);
        if fail_preflight {
            *launcher.preflight.lock().unwrap() = Some("runtime inaccessible".into());
        } else {
            *launcher.probe.lock().unwrap() =
                Some(vec![(Level::Bubblewrap, Err("namespaces disabled".into()))]);
        }
        assert_eq!(handle.restart(), Restart::Accepted);
        unavailable(&handle).await;
        let status = handle.status();
        assert_eq!(status.level, None);
        assert_eq!(status.landlock, None);
        handle.shutdown().await;
    }
}

#[tokio::test(start_paused = true)]
async fn before_priming_and_calibration_complete_a_job_start_answers_starting() {
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let launcher = FakeLauncher::scripted([Start::Ok(Behaviour {
        gate: Some(Arc::clone(&gate)),
        ..Behaviour::default()
    })]);
    let (assistant, handle) = manager(&launcher);
    let status = handle.clone();
    until(Duration::from_secs(60), move || {
        status.status().state == State::Starting("priming")
    })
    .await;
    assert_eq!(assistant.state(), RuntimeState::Starting);
    gate.add_permits(1);
    ready(&assistant).await;
}

/// Waits until the assistant is unavailable; its reason.
async fn unavailable(handle: &Handle) -> String {
    let probe = handle.clone();
    until(Duration::from_secs(60), move || {
        matches!(probe.status().state, State::Unavailable(_))
    })
    .await;
    match handle.status().state {
        State::Unavailable(why) => why,
        other => panic!("{other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn a_level_1_sandbox_failure_makes_the_assistant_unavailable_never_level_3() {
    let launcher = FakeLauncher::scripted([Start::Sandbox("bwrap: setup failed")]);
    let (_assistant, handle) = manager(&launcher);
    let why = unavailable(&handle).await;
    // Level 3 passed its (fake) probe, but it is never started (ASSIST-43).
    assert_eq!(launcher.levels(), [Level::Bubblewrap]);
    assert!(why.starts_with(LEVEL_1_NEEDED), "{why}");
    assert!(why.contains("bwrap: setup failed"), "{why}");
    let status = handle.status();
    assert_eq!(status.failures, 0);
    assert!(
        status
            .skipped
            .contains(&(Level::Bubblewrap, "bwrap: setup failed".into()))
    );
}

#[tokio::test(start_paused = true)]
async fn a_level_1_probe_failure_starts_nothing_and_says_how_to_fix_it() {
    let launcher = FakeLauncher::scripted([]);
    *launcher.probe.lock().unwrap() = Some(vec![
        (
            Level::Bubblewrap,
            Err("user namespaces are disabled".into()),
        ),
        (Level::Unconfined, Ok(())),
    ]);
    let (_assistant, handle) = manager(&launcher);
    let why = unavailable(&handle).await;
    assert_eq!(launcher.levels(), Vec::<Level>::new());
    assert!(why.contains("Install bubblewrap"), "{why}");
    assert!(why.contains("user namespaces are disabled"), "{why}");
    assert_eq!(
        handle.status().skipped,
        vec![
            (Level::Bubblewrap, "user namespaces are disabled".to_owned()),
            (
                Level::Unconfined,
                "not used in this release (ASSIST-43)".to_owned()
            ),
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn other_failures_retry_after_two_minutes_and_four_make_the_assistant_unavailable() {
    let launcher = FakeLauncher::scripted([
        Start::Other("model load failed"),
        Start::Other("hash mismatch"),
        Start::Other("exited after exec"),
        Start::Other("model load failed"),
    ]);
    let (assistant, handle) = manager(&launcher);
    let probe = handle.clone();
    until(Duration::from_mins(15), move || {
        matches!(probe.status().state, State::Unavailable(_))
    })
    .await;
    let starts = launcher.starts.lock().unwrap().clone();
    assert_eq!(starts.len(), 4);
    for pair in starts.windows(2) {
        let gap = pair[1].2 - pair[0].2;
        assert!(
            gap >= RETRY_AFTER && gap < RETRY_AFTER + Duration::from_secs(1),
            "{gap:?}"
        );
        // The level is kept.
        assert_eq!(pair[1].1, Level::Bubblewrap);
    }
    let status = handle.status();
    assert_eq!(status.failures, 4);
    assert!(matches!(&status.state, State::Unavailable(why) if why.contains("model load failed")));
    assert!(matches!(assistant.state(), RuntimeState::Unavailable(_)));
    // No fifth attempt without Restart.
    tokio::time::sleep(Duration::from_secs(3600)).await;
    assert_eq!(launcher.starts.lock().unwrap().len(), 4);
    // Priming and calibration failures never touch the breaker.
    assert!(assistant.breaker_closed());
}

#[tokio::test(start_paused = true)]
async fn a_priming_failure_is_a_counted_attempt_failure_outside_the_breaker() {
    let launcher = FakeLauncher::scripted([Start::Ok(Behaviour {
        prime_fails: true,
        ..Behaviour::default()
    })]);
    let (assistant, handle) = manager(&launcher);
    ready(&assistant).await;
    assert_eq!(launcher.starts.lock().unwrap().len(), 2);
    assert_eq!(handle.status().failures, 1);
    assert_eq!(
        handle.status().last_failure.as_deref(),
        Some("priming failed (the priming request answered HTTP 500)")
    );
    assert!(assistant.breaker_closed());
    // The failed child was stopped before the next one started.
    assert!(!launcher.coexisted.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn a_child_exit_while_ready_restarts_at_once_and_counts_only_within_ten_minutes() {
    let launcher = FakeLauncher::scripted([]);
    let (assistant, handle) = manager(&launcher);
    ready(&assistant).await;
    launcher.child(0).crash();
    let probe = Arc::clone(&launcher);
    until(Duration::from_secs(5), move || {
        probe.starts.lock().unwrap().len() == 2
    })
    .await;
    ready(&assistant).await;
    assert_eq!(handle.status().failures, 1);
    assert_eq!(handle.status().generation, 2);
    // The restarted child re-primed before admitting jobs.
    assert_eq!(launcher.primed.load(Ordering::SeqCst), 2);
    // After ten ready minutes the count is back to 0, and a later exit does not count.
    tokio::time::sleep(READY_WINDOW + Duration::from_secs(1)).await;
    assert_eq!(handle.status().failures, 0);
    launcher.child(1).crash();
    let probe = Arc::clone(&launcher);
    until(Duration::from_secs(5), move || {
        probe.starts.lock().unwrap().len() == 3
    })
    .await;
    ready(&assistant).await;
    assert_eq!(handle.status().failures, 0);
    assert!(!launcher.coexisted.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn two_consecutive_expiries_stop_the_child_as_hung_and_restart() {
    let launcher = FakeLauncher::scripted([]);
    let (assistant, handle) = manager(&launcher);
    ready(&assistant).await;
    // An expiry, a completed exchange, an expiry: not two in a row.
    assistant.signal_for_tests(Signal::Expired { generation: 1 });
    assistant.signal_for_tests(Signal::Completed { generation: 1 });
    assistant.signal_for_tests(Signal::Expired { generation: 1 });
    // A stale generation's expiry is ignored.
    assistant.signal_for_tests(Signal::Expired { generation: 0 });
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(launcher.starts.lock().unwrap().len(), 1);
    assistant.signal_for_tests(Signal::Expired { generation: 1 });
    let probe = Arc::clone(&launcher);
    until(Duration::from_secs(5), move || {
        probe.starts.lock().unwrap().len() == 2
    })
    .await;
    assert_eq!(launcher.stops.load(Ordering::SeqCst), 1);
    ready(&assistant).await;
    let status = handle.status();
    assert_eq!(status.failures, 1);
    assert!(status.last_failure.unwrap().contains("hung"));
    // Re-measured.
    assert_eq!(launcher.primed.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn too_slow_is_unavailable_with_the_numbers_and_no_retry() {
    // Each sample completes (about 76 s, both within the 180 s calibration limit), but
    // even B 512 needs 1.25 × 74 s, over the 90 s cap.
    let slow = Behaviour {
        p: 0.01,
        g: 0.2,
        ..Behaviour::default()
    };
    let launcher = FakeLauncher::scripted([Start::Ok(slow)]);
    let (assistant, handle) = manager(&launcher);
    let probe = handle.clone();
    until(Duration::from_mins(15), move || {
        matches!(probe.status().state, State::Unavailable(_))
    })
    .await;
    let status = handle.status();
    assert!(
        matches!(&status.state, State::Unavailable(why) if why.starts_with("too slow on this machine: about ") && why.ends_with("the cap is 90 s")),
        "{:?}",
        status.state
    );
    assert!(status.calibration.is_some());
    tokio::time::sleep(Duration::from_secs(3600)).await;
    assert_eq!(launcher.starts.lock().unwrap().len(), 1);
    assert_eq!(launcher.alive.load(Ordering::SeqCst), 0);
    assert!(matches!(assistant.state(), RuntimeState::Unavailable(_)));
}

#[tokio::test(start_paused = true)]
async fn a_sample_past_the_run_cap_is_too_slow_not_a_counted_failure() {
    // Each sample would take about 140 s: past the 90 s cap, the attempt ends "too slow" with
    // what it saw instead of retrying.
    let slow = Behaviour {
        p: 0.05,
        g: 0.35,
        ..Behaviour::default()
    };
    let launcher = FakeLauncher::scripted([Start::Ok(slow)]);
    let (_assistant, handle) = manager(&launcher);
    let probe = handle.clone();
    until(Duration::from_mins(15), move || {
        matches!(probe.status().state, State::Unavailable(_))
    })
    .await;
    let status = handle.status();
    assert_eq!(
        status.state,
        State::Unavailable(
            "too slow on this machine: a calibration sample took more than 90 s, the cap is 90 s"
                .into()
        )
    );
    assert_eq!(status.failures, 0);
    assert_eq!(launcher.starts.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_restart_press_is_deduplicated_until_ready() {
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let launcher = FakeLauncher::scripted([
        Start::Ok(Behaviour::default()),
        Start::Ok(Behaviour {
            gate: Some(Arc::clone(&gate)),
            ..Behaviour::default()
        }),
    ]);
    let (assistant, handle) = manager(&launcher);
    ready(&assistant).await;
    assert_eq!(handle.restart(), Restart::Accepted);
    let probe = handle.clone();
    until(Duration::from_secs(60), move || {
        probe.status().state == State::Starting("priming")
    })
    .await;
    // The second generation is priming: the Restart is still running.
    assert_eq!(handle.restart(), Restart::AlreadyRunning);
    gate.add_permits(1);
    ready(&assistant).await;
    assert_eq!(handle.restart(), Restart::Accepted);
}

#[tokio::test(start_paused = true)]
async fn restart_and_shutdown_do_not_wait_behind_a_slow_preflight() {
    let launcher = FakeLauncher::scripted([]);
    *launcher.preflight_delay.lock().unwrap() = Some(Duration::from_secs(3600));
    let (_assistant, handle) = manager(&launcher);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(handle.restart(), Restart::Accepted);
    tokio::time::sleep(Duration::from_secs(1)).await;
    // The Restart started a new session at once (a second preflight began).
    assert_eq!(*launcher.preflights.lock().unwrap(), 2);
    let stopping = tokio::time::timeout(Duration::from_secs(5), handle.shutdown()).await;
    assert!(stopping.is_ok(), "shutdown waited behind the preflight");
}

#[tokio::test(start_paused = true)]
async fn restart_resets_count_and_breaker_and_returns_to_level_1() {
    let launcher = FakeLauncher::scripted([
        Start::Other("the model server exited"),
        Start::Ok(Behaviour::default()),
    ]);
    let (assistant, handle) = manager(&launcher);
    // The counted failure waits two minutes before the retry; Restart cuts that short.
    let probe = handle.clone();
    until(Duration::from_secs(5), move || probe.status().failures == 1).await;
    assert_eq!(handle.restart(), Restart::Accepted);
    // A second press while the transition runs is no second transition.
    assert_eq!(handle.restart(), Restart::AlreadyRunning);
    ready(&assistant).await;
    let status = handle.status();
    assert_eq!(status.level, Some(Level::Bubblewrap));
    assert_eq!(status.failures, 0);
    assert!(!status.restarting);
    assert_eq!(launcher.levels(), [Level::Bubblewrap, Level::Bubblewrap]);
    assert!(!launcher.coexisted.load(Ordering::SeqCst));
    assert!(assistant.breaker_closed());
}

#[tokio::test(start_paused = true)]
async fn a_generation_change_ends_running_and_queued_jobs_as_restarted() {
    use crate::assistant::contract::{EnvelopeTarget, TARGET_ALIAS};
    use crate::assistant::jobs::{Answer, Origin, Poll};
    use crate::assistant::request::Draft;
    let launcher = FakeLauncher::scripted([]);
    let (assistant, _handle) = manager(&launcher);
    tokio::spawn(Arc::clone(&assistant).work());
    ready(&assistant).await;
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let stub = Arc::clone(&launcher.child(0).server.interpreter);
    *stub.gate.lock().unwrap() = Some(Arc::clone(&gate));
    let draft = || Draft {
        phrase: "heal 1".into(),
        target: EnvelopeTarget {
            alias: TARGET_ALIAS,
            id: "hp".into(),
            label: None,
            value: Some(3),
        },
        target_address: "field:hp".into(),
        items: Vec::new(),
    };
    let start = |account: i64, sheet: i64| {
        let registration = assistant.register(sheet, 5);
        assistant.start(
            &registration,
            account,
            Origin {
                sheet,
                target: "field:hp".into(),
                revision: 5,
            },
            draft(),
        )
    };
    let (running, _) = start(1, 1).unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    let waiting = start(2, 2);
    launcher.child(0).crash();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(assistant.poll(1, 1, running), Poll::Done(Answer::Restarted));
    if let Ok((waiting, _)) = waiting {
        assert_eq!(assistant.poll(2, 2, waiting), Poll::Done(Answer::Restarted));
    }
    assert!(assistant.breaker_closed());
    gate.add_permits(4);
}

#[tokio::test(start_paused = true)]
async fn a_preflight_refusal_keeps_the_assistant_unavailable_with_its_reason() {
    let launcher = FakeLauncher::scripted([]);
    *launcher.preflight.lock().unwrap() =
        Some("another Mimik uses this data directory's assistant".into());
    let (assistant, handle) = manager(&launcher);
    let probe = handle.clone();
    until(Duration::from_secs(5), move || {
        matches!(probe.status().state, State::Unavailable(_))
    })
    .await;
    assert!(launcher.starts.lock().unwrap().is_empty());
    assert_eq!(
        assistant.state(),
        RuntimeState::Unavailable("another Mimik uses this data directory's assistant".into())
    );
}

#[tokio::test(start_paused = true)]
async fn shutdown_stops_the_child() {
    let launcher = FakeLauncher::scripted([]);
    let (assistant, handle) = manager(&launcher);
    ready(&assistant).await;
    handle.shutdown().await;
    assert_eq!(launcher.alive.load(Ordering::SeqCst), 0);
    assert_eq!(launcher.stops.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn a_restart_starts_the_next_session_only_after_the_launchers_work_settled() {
    let launcher = FakeLauncher::scripted([]);
    *launcher.preflight_delay.lock().unwrap() = Some(Duration::from_secs(3600));
    let (_assistant, handle) = manager(&launcher);
    tokio::time::sleep(Duration::from_secs(1)).await;
    // The interrupted preflight's blocking work is still finishing.
    let gate = Arc::new(tokio::sync::Notify::new());
    *launcher.unsettled.lock().unwrap() = Some(Arc::clone(&gate));
    assert_eq!(handle.restart(), Restart::Accepted);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(launcher.cancels.load(Ordering::SeqCst), 1);
    assert_eq!(
        *launcher.preflights.lock().unwrap(),
        1,
        "the next session overlapped"
    );
    *launcher.unsettled.lock().unwrap() = None;
    gate.notify_waiters();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(*launcher.preflights.lock().unwrap(), 2);
    let stopping = tokio::time::timeout(Duration::from_secs(5), handle.shutdown()).await;
    assert!(stopping.is_ok());
}

// The GPU path (ASSIST-27): GPU first, any GPU failure moves to the CPU at once, uncounted,
// until Mimik restarts or the owner presses "Try the GPU again".

const GPU_CONFIG: Config = Config {
    path: Path::Gpu,
    max_tokens: 352,
    levels: USED_LEVELS,
};

fn gpu_manager(launcher: &Arc<FakeLauncher>) -> (Arc<Assistant>, Handle) {
    let assistant = Assistant::new();
    let handle = spawn(
        Arc::clone(&assistant),
        Arc::clone(launcher) as Arc<dyn Launcher>,
        GPU_CONFIG,
    );
    (assistant, handle)
}

fn rx6650() -> DeviceView {
    DeviceView {
        pci: "0000:0c:00.0".into(),
        node: "renderD128".into(),
        name: "Vulkan0".into(),
        description: "AMD Radeon RX 6650 XT (RADV NAVI23)".into(),
        total_mib: 8176,
    }
}

#[tokio::test(start_paused = true)]
async fn the_gpu_path_reaches_ready_with_gpu_budgets() {
    let launcher = FakeLauncher::scripted([]);
    *launcher.device.lock().unwrap() = Some(rx6650());
    let (assistant, handle) = gpu_manager(&launcher);
    ready(&assistant).await;
    let status = handle.status();
    assert_eq!(status.path, Path::Gpu);
    assert_eq!(status.level, Some(Level::Bubblewrap));
    assert_eq!(status.device, Some(rx6650()));
    assert_eq!(status.gpu_note, None);
    let budgets = status.budgets.unwrap();
    assert_eq!(budgets.bounds.bytes, 4096);
    assert_eq!(budgets.run, Duration::from_secs(15));
    assert_eq!(budgets.depth, 4);
    assert_eq!(*launcher.paths.lock().unwrap(), [Path::Gpu]);
    assert_eq!(*launcher.probed.lock().unwrap(), [Path::Gpu]);
}

/// Waits for `n` starts.
async fn starts(launcher: &Arc<FakeLauncher>, n: usize) {
    let probe = Arc::clone(launcher);
    until(Duration::from_mins(15), move || {
        probe.starts.lock().unwrap().len() >= n
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn a_gpu_failure_at_each_phase_moves_to_the_cpu_at_once_uncounted() {
    let too_slow = Behaviour {
        p: 0.01,
        g: 0.2,
        ..Behaviour::default()
    };
    let not_loaded = Behaviour {
        loaded: Err("only 30 of 41 layers were placed on the GPU".into()),
        ..Behaviour::default()
    };
    let no_priming = Behaviour {
        prime_fails: true,
        ..Behaviour::default()
    };
    // Never ready: the 60 s health deadline.
    let never_ready = Behaviour {
        loading: usize::MAX,
        ..Behaviour::default()
    };
    let refused = Behaviour {
        fault: Fault::RefuseHealth,
        ..Behaviour::default()
    };
    let no_tokenize = Behaviour {
        fault: Fault::TokenizeFails,
        ..Behaviour::default()
    };
    let no_timing = Behaviour {
        fault: Fault::MissingTiming,
        ..Behaviour::default()
    };
    // Each sample (52 s) is under the 60 s cap, but no candidate bound fits a deadline of
    // 1.25 times its estimate: the budgets do not derive.
    let no_budgets = Behaviour {
        p: 0.001,
        g: 0.148,
        ..Behaviour::default()
    };
    for (gpu, why) in [
        (
            Start::Other("the model could not be loaded"),
            "the model could not be loaded",
        ),
        (
            Start::Sandbox("bubblewrap could not set up the sandbox"),
            "bubblewrap could not set up",
        ),
        (
            Start::Other(
                "llama-server needs libvulkan.so.1: install libvulkan1 (Debian/Ubuntu) or your distribution's Vulkan loader",
            ),
            "needs libvulkan.so.1",
        ),
        (Start::Ok(not_loaded), "only 30 of 41 layers"),
        (Start::Ok(no_priming), "priming failed"),
        (Start::Ok(too_slow), "a calibration sample took more than"),
        (Start::Ok(never_ready), "did not answer /health within 60 s"),
        (Start::Ok(refused), "endpoint check refused"),
        (Start::Ok(no_tokenize), "/tokenize answered HTTP 500"),
        (
            Start::Ok(no_timing),
            "timing is missing, zero or not finite",
        ),
        (Start::Ok(no_budgets), "about"),
    ] {
        let launcher = FakeLauncher::scripted([gpu, Start::Ok(Behaviour::default())]);
        *launcher.device.lock().unwrap() = Some(rx6650());
        *launcher.device_notes.lock().unwrap() = vec!["0000:0d:00.0 (renderD129): x".into()];
        let (assistant, handle) = gpu_manager(&launcher);
        ready(&assistant).await;
        let status = handle.status();
        assert!(status.device_notes.is_empty(), "{why}");
        assert_eq!(
            *launcher.paths.lock().unwrap(),
            [Path::Gpu, Path::Cpu],
            "{why}"
        );
        assert_eq!(status.path, Path::Cpu, "{why}");
        assert_eq!(status.device, None, "{why}: the page shows no GPU in use");
        assert_eq!(status.failures, 0, "{why}");
        assert_eq!(status.level, Some(Level::Bubblewrap), "{why}");
        assert!(
            status
                .gpu_note
                .as_deref()
                .is_some_and(|note| note.contains(why)),
            "{why}: {:?}",
            status.gpu_note
        );
        // CPU budgets, and the CPU start followed at once (no 2-minute retry).
        assert_eq!(status.budgets.unwrap().bounds.bytes, 1024, "{why}");
        let starts = launcher.starts.lock().unwrap().clone();
        assert!(starts[1].2 - starts[0].2 < RETRY_AFTER, "{why}");
        assert!(!launcher.coexisted.load(Ordering::SeqCst), "{why}");
        assert!(assistant.breaker_closed(), "{why}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_gpu_child_exit_or_hang_while_ready_moves_to_the_cpu_uncounted() {
    for hang in [false, true] {
        let launcher = FakeLauncher::scripted([]);
        let (assistant, handle) = gpu_manager(&launcher);
        ready(&assistant).await;
        if hang {
            assistant.signal_for_tests(Signal::Expired { generation: 1 });
            assistant.signal_for_tests(Signal::Expired { generation: 1 });
        } else {
            launcher.child(0).crash();
        }
        // The moment the CPU path is current, the page no longer says Ready (the GPU child's
        // state) while the CPU server is still starting.
        let probe = handle.clone();
        until(Duration::from_mins(15), move || {
            probe.status().path == Path::Cpu
        })
        .await;
        assert!(
            !matches!(handle.status().state, State::Ready) || handle.status().budgets.is_some(),
            "{:?}",
            handle.status()
        );
        starts(&launcher, 2).await;
        ready(&assistant).await;
        let status = handle.status();
        assert_eq!(*launcher.paths.lock().unwrap(), [Path::Gpu, Path::Cpu]);
        assert_eq!(status.path, Path::Cpu);
        assert_eq!(status.failures, 0);
        assert!(status.gpu_note.is_some());
        assert!(!launcher.coexisted.load(Ordering::SeqCst));
    }
}

#[tokio::test(start_paused = true)]
async fn the_page_state_leaves_ready_the_moment_the_gpu_fails_over_to_the_cpu() {
    let launcher = FakeLauncher::scripted([]);
    let (assistant, handle) = gpu_manager(&launcher);
    ready(&assistant).await;
    // The CPU probe is held pending, so the state right after the fallback stays observable.
    let gate = Arc::new(tokio::sync::Notify::new());
    *launcher.cpu_probe_gate.lock().unwrap() = Some(Arc::clone(&gate));
    launcher.child(0).crash();
    let probe = handle.clone();
    until(Duration::from_mins(15), move || {
        probe.status().path == Path::Cpu
    })
    .await;
    let status = handle.status();
    assert!(matches!(status.state, State::Starting(_)), "{status:?}");
    assert_eq!(status.budgets, None, "{status:?}");
    gate.notify_one();
    starts(&launcher, 2).await;
    ready(&assistant).await;
    assert_eq!(handle.status().path, Path::Cpu);
}

#[tokio::test(start_paused = true)]
async fn no_usable_gpu_level_runs_the_cpu_path_at_its_own_level() {
    // The GPU build's probe fails at level 1 (for example no hardware GPU, or the render node
    // refused); the CPU build's passes: each path at its own level.
    let launcher = FakeLauncher::scripted([]);
    *launcher.gpu_probe.lock().unwrap() = Some(vec![(
        Level::Bubblewrap,
        Err("no hardware GPU is listed (software renderers are not used)".into()),
    )]);
    let (assistant, handle) = gpu_manager(&launcher);
    ready(&assistant).await;
    let status = handle.status();
    assert_eq!(*launcher.paths.lock().unwrap(), [Path::Cpu]);
    assert_eq!(*launcher.probed.lock().unwrap(), [Path::Gpu, Path::Cpu]);
    assert_eq!(status.path, Path::Cpu);
    assert_eq!(status.level, Some(Level::Bubblewrap));
    assert!(status.gpu_note.unwrap().contains("no hardware GPU"));
}

#[tokio::test(start_paused = true)]
async fn the_cpu_choice_never_probes_or_starts_the_gpu() {
    let launcher = FakeLauncher::scripted([]);
    let (assistant, handle) = manager(&launcher);
    ready(&assistant).await;
    assert_eq!(*launcher.probed.lock().unwrap(), [Path::Cpu]);
    assert_eq!(*launcher.paths.lock().unwrap(), [Path::Cpu]);
    assert_eq!(handle.try_gpu(), TryGpu::NotApplicable);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(launcher.starts.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn restart_keeps_the_cpu_after_a_gpu_failure_and_try_the_gpu_again_goes_back() {
    let launcher = FakeLauncher::scripted([Start::Other("device lost")]);
    let (assistant, handle) = gpu_manager(&launcher);
    ready(&assistant).await;
    assert_eq!(handle.status().path, Path::Cpu);
    // Restart keeps the CPU path.
    assert_eq!(handle.restart(), Restart::Accepted);
    starts(&launcher, 3).await;
    ready(&assistant).await;
    assert_eq!(
        *launcher.paths.lock().unwrap(),
        [Path::Gpu, Path::Cpu, Path::Cpu]
    );
    assert_eq!(handle.try_gpu(), TryGpu::Accepted);
    // A second press while that transition runs is no second transition.
    assert_eq!(handle.try_gpu(), TryGpu::AlreadyRunning);
    assert_eq!(handle.restart(), Restart::AlreadyRunning);
    starts(&launcher, 4).await;
    ready(&assistant).await;
    let status = handle.status();
    assert_eq!(
        *launcher.paths.lock().unwrap(),
        [Path::Gpu, Path::Cpu, Path::Cpu, Path::Gpu]
    );
    assert_eq!(status.path, Path::Gpu);
    assert_eq!(status.gpu_note, None);
    assert!(!status.restarting);
    assert_eq!(handle.try_gpu(), TryGpu::NotApplicable);
    assert!(!launcher.coexisted.load(Ordering::SeqCst));
}

#[test]
fn a_manager_whose_handles_are_all_dropped_while_ready_does_not_spin() {
    // Regression: the ready loop took a closed command channel's `None` as "nothing" and
    // polled it again at once, spinning. On one thread with the paused clock a spinning
    // manager keeps the runtime busy, so the clock never advances and the sleep never ends.
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .unwrap();
        runtime.block_on(async {
            let launcher = FakeLauncher::scripted([]);
            let (assistant, handle) = manager(&launcher);
            ready(&assistant).await;
            drop(handle);
            tokio::time::sleep(Duration::from_secs(60)).await;
        });
        let _ = done.send(());
    });
    assert!(
        finished.recv_timeout(Duration::from_secs(30)).is_ok(),
        "the manager spun once its handles were gone"
    );
}
