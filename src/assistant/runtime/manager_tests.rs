//! The transitions table, one test per row, with a fake launcher, fake children and Tokio's
//! paused clock.
use super::super::progress::StartEvent;
use super::super::server::{Structured, Timings};
use super::*;
use crate::assistant::interpreter::stub::StubInterpreter;
use crate::assistant::interpreter::{Failure, Interpreter};
use crate::assistant::jobs::RuntimeState;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::watch;

/// How a fake server answers.
#[derive(Clone)]
pub(crate) struct Behaviour {
    /// `/health` answers before it is ready.
    pub(crate) loading: usize,
    pub(crate) prime_fails: bool,
    /// Per-token prompt and generation seconds of the samples.
    pub(crate) p: f64,
    pub(crate) g: f64,
    /// Holds priming until a permit is added.
    pub(crate) gate: Option<Arc<tokio::sync::Semaphore>>,
    /// What `loaded` answers (the GPU offload check).
    pub(crate) loaded: Result<(), String>,
    pub(crate) landlock: Option<super::super::hako::Landlock>,
    /// A malfunction of the fake server, if any.
    pub(crate) fault: Fault,
    /// Generated tokens of the trial phrases' replies, in order.
    pub(crate) phrase_tokens: [f64; 5],
    /// How long each trial phrase takes.
    pub(crate) phrase_delay: Duration,
    /// The fixed prefix's tokens as priming reports them.
    pub(crate) prefix_tokens: f64,
    /// How long priming takes.
    pub(crate) prime_delay: Duration,
    /// How long each `/tokenize` call takes.
    pub(crate) tokenize_delay: Duration,
    /// How the reminder prime (a reminder request through the interpreter) goes.
    pub(crate) reminder_prime: Prime,
}

/// How a fake server answers the reminder prime.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Prime {
    Answers,
    Fails,
    /// Takes far longer than any run deadline.
    Hangs,
}

/// What a fake server gets wrong.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fault {
    None,
    /// `/health` is refused by the endpoint check.
    RefuseHealth,
    /// `/tokenize` answers HTTP 500.
    TokenizeFails,
    /// The samples come back without their prompt timing.
    MissingTiming,
    /// The third trial phrase's reply stops at the default reply length.
    TruncatedPhrase,
    /// The second trial phrase's reply is not valid against the schema.
    InvalidPhrase,
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
            phrase_tokens: [60.0, 70.0, 80.0, 65.0, 75.0],
            phrase_delay: Duration::from_millis(100),
            prefix_tokens: 1460.0,
            prime_delay: Duration::ZERO,
            tokenize_delay: Duration::ZERO,
            reminder_prime: Prime::Answers,
        }
    }
}

pub(crate) struct FakeServer {
    behaviour: Behaviour,
    /// Trial phrases answered so far.
    phrases: AtomicUsize,
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
        if !self.behaviour.tokenize_delay.is_zero() {
            tokio::time::sleep(self.behaviour.tokenize_delay).await;
        }
        Ok(text.len().div_ceil(3))
    }

    async fn plain(&self, user: &str, max_tokens: u32, _: bool) -> Result<Timings, Failure> {
        if max_tokens == 1 {
            if let Some(gate) = &self.behaviour.gate {
                gate.acquire().await.unwrap().forget();
            }
            if !self.behaviour.prime_delay.is_zero() {
                tokio::time::sleep(self.behaviour.prime_delay).await;
            }
            if self.behaviour.prime_fails {
                return Err(Failure::Status(500));
            }
            self.primed.fetch_add(1, Ordering::SeqCst);
            return Ok(Timings {
                prompt_n: Some(self.behaviour.prefix_tokens),
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
            cache_n: Some(self.behaviour.prefix_tokens * 0.9),
            predicted_n: Some(predicted),
            predicted_ms: Some(predicted_ms),
        })
    }

    async fn structured(
        &self,
        _: &str,
        _: &[String],
        max_tokens: u32,
    ) -> Result<Structured, Failure> {
        assert_eq!(
            max_tokens, 1024,
            "phrases go out at the default reply length"
        );
        let n = self.phrases.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.behaviour.phrase_delay).await;
        Ok(Structured {
            text: if self.behaviour.fault == Fault::InvalidPhrase && n == 1 {
                r#"{"kind":"nonsense"}"#.into()
            } else {
                r#"{"kind":"options","options":[]}"#.into()
            },
            truncated: self.behaviour.fault == Fault::TruncatedPhrase && n == 2,
            predicted_n: Some(self.behaviour.phrase_tokens[n.min(4)]),
        })
    }

    fn interpreter(&self) -> Arc<dyn Interpreter> {
        Arc::clone(&self.interpreter) as Arc<dyn Interpreter>
    }
}

pub(crate) struct FakeChild {
    pub(crate) server: Arc<FakeServer>,
    pub(crate) exit: watch::Sender<Option<Fail>>,
    pub(crate) stopped: AtomicBool,
    pub(crate) alive: Arc<AtomicUsize>,
    pub(crate) stops: Arc<AtomicUsize>,
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
    pub(crate) fn crash(&self) {
        self.gone();
    }
}

#[derive(Clone)]
pub(crate) enum Start {
    Ok(Behaviour),
    Sandbox(&'static str),
    Other(&'static str),
}

type Probed = Vec<(Level, Result<(), String>)>;

#[derive(Default)]
pub(crate) struct FakeLauncher {
    pub(crate) preflight: Mutex<Option<String>>,
    pub(crate) preflight_delay: Mutex<Option<Duration>>,
    /// Delays of the next preflights, one each, before `preflight_delay` applies.
    pub(crate) preflight_delays: Mutex<VecDeque<Duration>>,
    pub(crate) preflights: Mutex<usize>,
    /// The path each preflight was for, in order.
    pub(crate) preflight_paths: Mutex<Vec<Path>>,
    /// While set, `settle` waits (as for a blocking start still finishing).
    pub(crate) unsettled: Mutex<Option<Arc<tokio::sync::Notify>>>,
    pub(crate) cancels: AtomicUsize,
    pub(crate) probe: Mutex<Option<Probed>>,
    /// The GPU path's probe (default: level 1 passes).
    pub(crate) gpu_probe: Mutex<Option<Probed>>,
    /// The paths probed, in order.
    pub(crate) probed: Mutex<Vec<Path>>,
    /// The path of each start, in order.
    pub(crate) paths: Mutex<Vec<Path>>,
    pub(crate) device: Mutex<Option<DeviceView>>,
    pub(crate) device_notes: Mutex<Vec<String>>,
    /// Holds the CPU probe until notified.
    pub(crate) cpu_probe_gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
    pub(crate) script: Mutex<VecDeque<Start>>,
    pub(crate) starts: Mutex<Vec<(u64, Level, Instant)>>,
    pub(crate) children: Mutex<Vec<Arc<FakeChild>>>,
    pub(crate) alive: Arc<AtomicUsize>,
    pub(crate) stops: Arc<AtomicUsize>,
    pub(crate) primed: Arc<AtomicUsize>,
    pub(crate) coexisted: AtomicBool,
    /// What the advisory library check says per path (CPU, GPU).
    pub(crate) hints: Mutex<[Option<String>; 2]>,
    /// The models selected for each start, in order.
    pub(crate) selected: Mutex<Vec<Model>>,
    /// What hashing each model file gives.
    pub(crate) files: Mutex<std::collections::BTreeMap<String, Result<ModelFile, String>>>,
    /// The files hashed, in order.
    pub(crate) hashed: Mutex<Vec<String>>,
    pub(crate) hash_delay: Mutex<Option<Duration>>,
    /// Blocking work that holds a ticket (the hash of a large file).
    pub(crate) work: Tickets,
    pub(crate) hash_probe: Mutex<Option<Arc<HashProbe>>>,
    /// Model files missing from the models folder (all others are present).
    pub(crate) absent: Mutex<Vec<String>>,
    /// Where the manager wants the verification reported.
    pub(crate) observer: Mutex<Option<Observer>>,
    /// How long a start takes before it verifies the model file, and how long it verifies.
    pub(crate) start_delay: Mutex<Option<Duration>>,
    pub(crate) verify_delay: Mutex<Option<Duration>>,
    /// The verification finds the hash remembered: nothing is read.
    pub(crate) verify_cached: Mutex<bool>,
}

/// The launcher's in-flight work, as the real one keeps it.
pub(crate) struct Tickets(pub(crate) Arc<super::super::launch::Work>);

impl Default for Tickets {
    fn default() -> Self {
        Self(super::super::launch::Work::new())
    }
}

/// What blocking hash work saw.
#[derive(Default)]
pub(crate) struct HashProbe {
    pub(crate) cancelled_seen: AtomicBool,
    pub(crate) timed_out: AtomicBool,
}

#[async_trait]
impl Launcher for FakeLauncher {
    fn failure_hint(&self, path: Path) -> Option<String> {
        self.hints.lock().unwrap()[usize::from(path == Path::Gpu)].clone()
    }

    async fn preflight(&self, path: Path) -> Result<(), String> {
        *self.preflights.lock().unwrap() += 1;
        self.preflight_paths.lock().unwrap().push(path);
        let once = self.preflight_delays.lock().unwrap().pop_front();
        let delay = once.or(*self.preflight_delay.lock().unwrap());
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
        self.work.0.cancel();
    }

    /// As the real launcher's: waits for every ticket (blocking work) and for every child
    /// to be gone (a child's exit watcher holds its ticket until then). A settle called
    /// while a child still runs would wait forever, so it fails the test instead.
    async fn settle(&self) {
        let gate = self.unsettled.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.notified().await;
        }
        self.work.0.settle().await;
        let began = Instant::now();
        while self.alive.load(Ordering::SeqCst) != 0 {
            assert!(
                began.elapsed() < Duration::from_hours(1),
                "settle deadlocked: a child is still running (stop it first)"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
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
                .unwrap_or_else(|| vec![(Level::Sandbox, Ok(())), (Level::Unconfined, Ok(()))]);
        }
        let gate = self.cpu_probe_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.notified().await;
        }
        self.probe
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| vec![(Level::Sandbox, Ok(())), (Level::Unconfined, Ok(()))])
    }

    fn device(&self) -> Option<DeviceView> {
        self.device.lock().unwrap().clone()
    }

    fn observe(&self, observer: Observer) {
        *self.observer.lock().unwrap() = Some(observer);
    }

    fn select(&self, model: &Model) {
        self.selected.lock().unwrap().push(model.clone());
    }

    fn models_folder(&self) -> String {
        "/srv/models".to_owned()
    }

    async fn model_present(&self, name: &str) -> bool {
        !self.absent.lock().unwrap().iter().any(|n| n == name)
    }

    async fn hash_model(&self, name: &str) -> Result<ModelFile, String> {
        self.hashed.lock().unwrap().push(name.to_owned());
        let probe = self.hash_probe.lock().unwrap().clone();
        if let Some(probe) = probe {
            // Blocking work holding a ticket, as hashing a large file does: it stops only
            // when the launcher's work is cancelled.
            let ticket = self.work.0.enter();
            return tokio::task::spawn_blocking(move || {
                for _ in 0..5000 {
                    if ticket.cancelled() {
                        probe.cancelled_seen.store(true, Ordering::SeqCst);
                        return Err("cancelled".to_owned());
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                probe.timed_out.store(true, Ordering::SeqCst);
                Err("the hash ran on".to_owned())
            })
            .await
            .unwrap_or_else(|_| Err("hash task failed".into()));
        }
        let delay = *self.hash_delay.lock().unwrap();
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        self.files
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .unwrap_or_else(|| Err(format!("the model: {name} is missing")))
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
        // As the real launcher reports its verification of the model file.
        let delay = *self.start_delay.lock().unwrap();
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        let observer = self.observer.lock().unwrap().clone();
        let report = |event| {
            if let Some(observer) = &observer {
                observer(event);
            }
        };
        report(StartEvent::Verifying);
        let delay = *self.verify_delay.lock().unwrap();
        let cached = *self.verify_cached.lock().unwrap();
        if let Some(delay) = delay {
            if !cached {
                report(StartEvent::Hashed {
                    done: 1 << 20,
                    total: 4 << 20,
                });
            }
            tokio::time::sleep(delay).await;
        }
        report(StartEvent::Verified {
            unchanged: delay.is_none() || cached,
        });
        // The rest of the start (the spawn) takes as long, so the note can be seen.
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        let behaviour = match script {
            Start::Ok(behaviour) => behaviour,
            Start::Sandbox(why) => return Err(Fail::Sandbox(why.into())),
            Start::Other(why) => return Err(Fail::Other(why.into())),
        };
        self.alive.fetch_add(1, Ordering::SeqCst);
        let interpreter = StubInterpreter::default();
        match behaviour.reminder_prime {
            Prime::Answers => interpreter
                .answers
                .lock()
                .unwrap()
                .push_back(Ok("{}".into())),
            Prime::Fails => interpreter
                .answers
                .lock()
                .unwrap()
                .push_back(Err(Failure::Status(500))),
            Prime::Hangs => {
                interpreter
                    .answers
                    .lock()
                    .unwrap()
                    .push_back(Ok("{}".into()));
                interpreter
                    .delays
                    .lock()
                    .unwrap()
                    .push_back(Duration::from_secs(100_000));
            }
        }
        let child = Arc::new(FakeChild {
            server: Arc::new(FakeServer {
                behaviour,
                phrases: AtomicUsize::new(0),
                health_calls: AtomicUsize::new(0),
                primed: Arc::clone(&self.primed),
                interpreter: Arc::new(interpreter),
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
    pub(crate) fn scripted(script: impl IntoIterator<Item = Start>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into_iter().collect()),
            ..Self::default()
        })
    }

    pub(crate) fn levels(&self) -> Vec<Level> {
        self.starts
            .lock()
            .unwrap()
            .iter()
            .map(|(_, l, _)| *l)
            .collect()
    }

    pub(crate) fn child(&self, n: usize) -> Arc<FakeChild> {
        Arc::clone(&self.children.lock().unwrap()[n])
    }
}

pub(crate) const CONFIG: Config = Config {
    path: Path::Cpu,
    levels: USED_LEVELS,
};

pub(crate) fn manager(launcher: &Arc<FakeLauncher>) -> (Arc<Assistant>, Handle) {
    let assistant = Assistant::new();
    let handle = spawn(
        Arc::clone(&assistant),
        Arc::clone(launcher) as Arc<dyn Launcher>,
        CONFIG,
    );
    (assistant, handle)
}

/// Lets the paused clock run until `done` holds (at most `limit` of virtual time).
pub(crate) async fn until(limit: Duration, mut done: impl FnMut() -> bool) {
    let end = Instant::now() + limit;
    while !done() {
        assert!(Instant::now() < end, "not reached within {limit:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub(crate) async fn ready(assistant: &Arc<Assistant>) {
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
    assert_eq!(status.level, Some(Level::Sandbox));
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
                Some(vec![(Level::Sandbox, Err("namespaces disabled".into()))]);
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
    until(
        Duration::from_secs(60),
        move || matches!(status.status().state, State::Starting(p) if p.phase == Phase::Priming),
    )
    .await;
    assert_eq!(assistant.state(), RuntimeState::Starting);
    gate.add_permits(1);
    ready(&assistant).await;
}

/// Waits until the assistant is unavailable; its reason.
pub(crate) async fn unavailable(handle: &Handle) -> String {
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
    let launcher = FakeLauncher::scripted([Start::Sandbox("mimik sandbox: setup failed")]);
    let (_assistant, handle) = manager(&launcher);
    let why = unavailable(&handle).await;
    // Level 3 passed its (fake) probe, but it is never started (ASSIST-43).
    assert_eq!(launcher.levels(), [Level::Sandbox]);
    assert!(why.starts_with(LEVEL_1_NEEDED), "{why}");
    assert!(why.contains("mimik sandbox: setup failed"), "{why}");
    let status = handle.status();
    assert_eq!(status.failures, 0);
    assert!(
        status
            .skipped
            .contains(&(Level::Sandbox, "mimik sandbox: setup failed".into()))
    );
}

#[tokio::test(start_paused = true)]
async fn a_level_1_probe_failure_starts_nothing_and_says_how_to_fix_it() {
    let launcher = FakeLauncher::scripted([]);
    *launcher.probe.lock().unwrap() = Some(vec![
        (Level::Sandbox, Err("user namespaces are disabled".into())),
        (Level::Unconfined, Ok(())),
    ]);
    let (_assistant, handle) = manager(&launcher);
    let why = unavailable(&handle).await;
    assert_eq!(launcher.levels(), Vec::<Level>::new());
    assert!(why.contains("assistant-apparmor-profile"), "{why}");
    assert!(why.contains("user namespaces are disabled"), "{why}");
    assert_eq!(
        handle.status().skipped,
        vec![
            (Level::Sandbox, "user namespaces are disabled".to_owned()),
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
        assert_eq!(pair[1].1, Level::Sandbox);
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
    // Each sample completes (about 40 s, well within its 480 s timeout), but a reply of the
    // default 1,024 tokens at 0.6 s each puts even B 512 over the 600 s ceiling.
    let slow = Behaviour {
        p: 0.01,
        g: 0.6,
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
        matches!(&status.state, State::Unavailable(why) if why.starts_with("too slow on this machine: about ") && why.ends_with("the ceiling is 600 s")),
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
async fn a_sample_past_its_timeout_is_too_slow_not_a_counted_failure() {
    // Each sample would take about 520 s: past the 480 s timeout, the attempt ends "too slow"
    // instead of retrying.
    let slow = Behaviour {
        p: 0.05,
        g: 8.0,
        ..Behaviour::default()
    };
    let launcher = FakeLauncher::scripted([Start::Ok(slow)]);
    let (_assistant, handle) = manager(&launcher);
    let probe = handle.clone();
    until(Duration::from_mins(30), move || {
        matches!(probe.status().state, State::Unavailable(_))
    })
    .await;
    let status = handle.status();
    assert_eq!(
        status.state,
        State::Unavailable(
            "too slow on this machine: a calibration sample took more than 480 s, so a reply could take longer than the 600 s ceiling"
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
    until(
        Duration::from_secs(60),
        move || matches!(probe.status().state, State::Starting(p) if p.phase == Phase::Priming),
    )
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
    assert_eq!(status.level, Some(Level::Sandbox));
    assert_eq!(status.failures, 0);
    assert!(!status.restarting);
    assert_eq!(launcher.levels(), [Level::Sandbox, Level::Sandbox]);
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
                kind: crate::assistant::jobs::Kind::Interpretation,
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
    assert_eq!(status.level, Some(Level::Sandbox));
    assert_eq!(status.device, Some(rx6650()));
    assert_eq!(status.gpu_note, None);
    let budgets = status.budgets.unwrap();
    assert_eq!(budgets.bounds.bytes, 4096);
    // 10 ms per token over 1,024 × (1 + 7e-5 × 512) tokens plus 1.6 s of prompt: T 12.2 s.
    assert_eq!(budgets.run, Duration::from_secs(20));
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
        p: 0.001,
        g: 8.0,
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
    // Each sample (38 s) is under its timeout, but a default-length reply at 0.6 s a token
    // puts every candidate bound over the 600 s ceiling: the budgets do not derive.
    let no_budgets = Behaviour {
        p: 0.001,
        g: 0.6,
        ..Behaviour::default()
    };
    for (gpu, why) in [
        (
            Start::Other("the model could not be loaded"),
            "the model could not be loaded",
        ),
        (
            Start::Sandbox("hakoniwa could not set up the sandbox"),
            "hakoniwa could not set up",
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
        assert_eq!(status.level, Some(Level::Sandbox), "{why}");
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
        // A sample that times out at 480 s is the attempt itself taking that long, not a retry
        // pause.
        let limit = if why.starts_with("a calibration sample") {
            Duration::from_secs(500)
        } else {
            RETRY_AFTER
        };
        assert!(starts[1].2 - starts[0].2 < limit, "{why}");
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
        Level::Sandbox,
        Err("no hardware GPU is listed (software renderers are not used)".into()),
    )]);
    let (assistant, handle) = gpu_manager(&launcher);
    ready(&assistant).await;
    let status = handle.status();
    assert_eq!(*launcher.paths.lock().unwrap(), [Path::Cpu]);
    assert_eq!(*launcher.probed.lock().unwrap(), [Path::Gpu, Path::Cpu]);
    assert_eq!(status.path, Path::Cpu);
    assert_eq!(status.level, Some(Level::Sandbox));
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

#[tokio::test]
async fn a_failed_gpu_start_gets_the_libraries_hint_and_falls_back() {
    let hint = "llama-server may need libvulkan.so.1: install libvulkan1 (Debian/Ubuntu) or your distribution's Vulkan loader";
    let launcher = FakeLauncher::scripted([
        Start::Other("the model server exited before it was ready"),
        Start::Ok(Behaviour::default()),
    ]);
    launcher.hints.lock().unwrap()[1] = Some(hint.into());
    *launcher.device.lock().unwrap() = Some(rx6650());
    let (assistant, handle) = gpu_manager(&launcher);
    ready(&assistant).await;
    let status = handle.status();
    // The GPU start was attempted, failed, and the reason names the library.
    assert_eq!(*launcher.paths.lock().unwrap(), [Path::Gpu, Path::Cpu]);
    let note = status.gpu_note.unwrap();
    assert!(note.contains("exited before it was ready"), "{note}");
    assert!(note.contains(hint), "{note}");
    assert_eq!(status.path, Path::Cpu);
    assert_eq!(status.level, Some(Level::Sandbox));
}

#[tokio::test]
async fn a_hint_for_a_library_never_changes_a_start_that_succeeds() {
    // A false "missing": the start works, and nothing of the hint shows.
    let launcher = FakeLauncher::scripted([Start::Ok(Behaviour::default())]);
    launcher.hints.lock().unwrap()[0] = Some("llama-server may need libgomp.so.1: x".into());
    let (assistant, handle) = manager(&launcher);
    ready(&assistant).await;
    let status = handle.status();
    assert_eq!(status.state, State::Ready);
    assert_eq!(status.level, Some(Level::Sandbox));
    assert_eq!(status.failures, 0);
    assert_eq!(status.last_failure, None);
    assert!(
        status.skipped.is_empty()
            || status
                .skipped
                .iter()
                .all(|(_, why)| !why.contains("libgomp"))
    );
}

#[tokio::test]
async fn a_failed_cpu_start_is_counted_with_the_libraries_hint() {
    let launcher = FakeLauncher::scripted([Start::Other("the model server did not answer")]);
    launcher.hints.lock().unwrap()[0] =
        Some("llama-server may need libgomp.so.1: install libgomp1".into());
    let (_assistant, handle) = manager(&launcher);
    let deadline = Instant::now() + Duration::from_secs(5);
    let last = loop {
        if let Some(last) = handle.status().last_failure {
            break last;
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(last.contains("did not answer"), "{last}");
    assert!(last.contains("may need libgomp.so.1"), "{last}");
    handle.shutdown().await;
}

const GOMP_HINT: &str = "llama-server may need libgomp.so.1: install libgomp1 (Debian/Ubuntu) or your distribution's OpenMP runtime";

#[tokio::test(start_paused = true)]
async fn a_start_that_succeeds_shows_the_hint_nowhere_even_with_the_real_launchers_level_3_error() {
    let launcher = FakeLauncher::scripted([]);
    // What the real launcher answers for level 3 (`Launch::probe`).
    *launcher.probe.lock().unwrap() = Some(vec![
        (Level::Sandbox, Ok(())),
        (
            Level::Unconfined,
            Err("not used in this release (ASSIST-43)".into()),
        ),
    ]);
    launcher.hints.lock().unwrap()[0] = Some(GOMP_HINT.into());
    let (assistant, handle) = manager(&launcher);
    ready(&assistant).await;
    let status = handle.status();
    assert_eq!(status.state, State::Ready);
    assert_eq!(status.skipped.len(), 1, "{status:?}");
    assert!(
        status
            .skipped
            .iter()
            .all(|(_, why)| !why.contains("libgomp")),
        "{status:?}"
    );
    assert_eq!(status.last_failure, None);
    assert_eq!(status.gpu_note, None);
}

#[tokio::test(start_paused = true)]
async fn the_exit_of_a_server_that_was_ready_carries_no_hint() {
    // CPU: an exit after ready is counted, with its own reason only.
    let launcher = FakeLauncher::scripted([]);
    launcher.hints.lock().unwrap()[0] = Some(GOMP_HINT.into());
    let (assistant, handle) = manager(&launcher);
    ready(&assistant).await;
    launcher.child(0).crash();
    let probe = handle.clone();
    until(Duration::from_mins(15), move || {
        probe.status().last_failure.is_some()
    })
    .await;
    let last = handle.status().last_failure.unwrap();
    assert!(!last.contains("libgomp"), "{last}");
    // GPU: the fallback after a ready GPU child's exit says nothing of libraries either.
    let launcher = FakeLauncher::scripted([]);
    launcher.hints.lock().unwrap()[1] = Some(GOMP_HINT.into());
    let (assistant, handle) = gpu_manager(&launcher);
    ready(&assistant).await;
    launcher.child(0).crash();
    let probe = handle.clone();
    until(Duration::from_mins(15), move || {
        probe.status().path == Path::Cpu
    })
    .await;
    let note = handle.status().gpu_note.unwrap();
    assert!(!note.contains("libgomp"), "{note}");
}

#[tokio::test(start_paused = true)]
async fn a_failure_after_health_answered_carries_no_library_hint() {
    // CPU: /health answered, then calibration fails (a timing is missing): the server loaded,
    // so no library is missing and the hint would mislead.
    let timing = Behaviour {
        fault: Fault::MissingTiming,
        ..Behaviour::default()
    };
    let launcher = FakeLauncher::scripted([Start::Ok(timing)]);
    launcher.hints.lock().unwrap()[0] = Some(GOMP_HINT.into());
    let (_assistant, handle) = manager(&launcher);
    let probe = handle.clone();
    until(Duration::from_mins(15), move || {
        probe.status().last_failure.is_some()
    })
    .await;
    let last = handle.status().last_failure.unwrap();
    assert!(last.contains("timing is missing"), "{last}");
    assert!(!last.contains("libgomp"), "{last}");
    handle.shutdown().await;
    // GPU: priming fails after /health: the fallback reason has no hint either.
    let priming = Behaviour {
        prime_fails: true,
        ..Behaviour::default()
    };
    let launcher = FakeLauncher::scripted([Start::Ok(priming), Start::Ok(Behaviour::default())]);
    *launcher.device.lock().unwrap() = Some(rx6650());
    launcher.hints.lock().unwrap()[1] = Some(GOMP_HINT.into());
    let (assistant, handle) = gpu_manager(&launcher);
    ready(&assistant).await;
    let note = handle.status().gpu_note.unwrap();
    assert!(note.contains("priming failed"), "{note}");
    assert!(!note.contains("libgomp"), "{note}");
}

#[test]
fn an_older_cancel_never_replaces_a_newer_one() {
    // Cancel A validated against trial 1 publishes after Cancel B for trial 2 (overlapping
    // requests across trial turnover): trial 2's cancel must survive.
    let cancel = Cancel::default();
    cancel.publish(2);
    cancel.publish(1);
    assert_eq!(cancel.trial.load(Ordering::SeqCst), 2);
    cancel.publish(3);
    assert_eq!(cancel.trial.load(Ordering::SeqCst), 3);
}

/// The memory preflight is for the path that starts: a GPU session
/// checks the GPU path, and the GPU-to-CPU fallback checks the CPU path again, so a host that
/// cannot hold the cache says why instead of starting.
#[tokio::test(start_paused = true)]
async fn the_preflight_is_for_the_path_that_starts_and_the_fallback_checks_the_cpu_again() {
    let launcher = FakeLauncher::scripted([]);
    let (assistant, handle) = gpu_manager(&launcher);
    ready(&assistant).await;
    assert_eq!(*launcher.preflight_paths.lock().unwrap(), [Path::Gpu]);
    *launcher.preflight.lock().unwrap() = Some("not enough memory for this model".into());
    launcher.child(0).crash();
    let probe = handle.clone();
    until(Duration::from_mins(15), move || {
        matches!(probe.status().state, State::Unavailable(_))
    })
    .await;
    assert_eq!(
        *launcher.preflight_paths.lock().unwrap(),
        [Path::Gpu, Path::Cpu]
    );
    drop(assistant);
}

// --- The reminder prime before Ready (plan 3c-1c, step 1) ------------------------------------

/// The reminder requests the fake server's interpreter got (system message kind, `max_tokens`).
fn reminder_requests(launcher: &Arc<FakeLauncher>) -> Vec<(bool, u32)> {
    let child = Arc::clone(&launcher.children.lock().unwrap()[0]);
    let requests = child.server.interpreter.requests.lock().unwrap();
    requests
        .iter()
        .map(|request| {
            (
                matches!(
                    request.kind,
                    crate::assistant::interpreter::RequestKind::Reminder { .. }
                ),
                request.max_tokens,
            )
        })
        .collect()
}

/// A manager over `launcher` for `path` whose assistant has the stored reminder choice `choice`
/// from the start.
fn manager_with_choice(
    launcher: &Arc<FakeLauncher>,
    path: Path,
    choice: Option<bool>,
) -> (Arc<Assistant>, Handle) {
    let assistant = Assistant::new();
    assistant.set_reminder_choice(choice);
    let handle = spawn(
        Arc::clone(&assistant),
        Arc::clone(launcher) as Arc<dyn Launcher>,
        Config {
            path,
            levels: USED_LEVELS,
        },
    );
    (assistant, handle)
}

#[tokio::test(start_paused = true)]
async fn the_gpu_path_always_primes_the_reminder_prompt_and_the_cpu_path_only_with_reminders_on() {
    for (path, choice, primes) in [
        (Path::Cpu, Some(true), true),
        (Path::Cpu, Some(false), false),
        (Path::Cpu, None, false),
        (Path::Gpu, Some(false), true),
        (Path::Gpu, None, true),
    ] {
        let launcher = FakeLauncher::scripted([]);
        *launcher.device.lock().unwrap() = Some(rx6650());
        let (assistant, handle) = manager_with_choice(&launcher, path, choice);
        ready(&assistant).await;
        let requests = reminder_requests(&launcher);
        if primes {
            // One reminder request with one token, before Ready.
            assert_eq!(requests, [(true, 1)], "{path:?} {choice:?}");
            assert!(matches!(
                assistant.reminder_generation(),
                Some((_, Reminders::Available(_)))
            ));
            assert!(assistant.cache_check_armed_for_tests());
        } else {
            assert!(requests.is_empty(), "{path:?} {choice:?}: {requests:?}");
            // Not prepared: off whatever the setting says now, until a Restart.
            assert!(matches!(
                assistant.reminder_generation(),
                Some((Path::Cpu, Reminders::NotPrepared))
            ));
            assistant.set_reminder_choice(Some(true));
            assert!(!assistant.reminders_effective());
            assert!(!assistant.cache_check_armed_for_tests());
        }
        handle.shutdown().await;
    }
}

#[tokio::test(start_paused = true)]
async fn a_failed_reminder_prime_leaves_the_generation_ready_with_reminders_unavailable() {
    let launcher = FakeLauncher::scripted([Start::Ok(Behaviour {
        reminder_prime: Prime::Fails,
        ..Behaviour::default()
    })]);
    let (assistant, handle) = manager_with_choice(&launcher, Path::Cpu, Some(true));
    ready(&assistant).await;
    let Some((_, Reminders::Unavailable(why))) = assistant.reminder_generation() else {
        panic!("{:?}", assistant.reminder_generation())
    };
    assert!(why.contains("could not be prepared"), "{why}");
    assert!(!assistant.reminders_effective());
    // A failure is not a busy server, and no cache check is armed.
    assert_eq!(assistant.busy_for_tests(), Duration::ZERO);
    assert!(!assistant.cache_check_armed_for_tests());
    assert_eq!(handle.status().state, State::Ready);
    handle.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_reminder_prime_that_times_out_presumes_the_server_busy_for_its_bound() {
    let launcher = FakeLauncher::scripted([Start::Ok(Behaviour {
        reminder_prime: Prime::Hangs,
        ..Behaviour::default()
    })]);
    let (assistant, handle) = manager_with_choice(&launcher, Path::Cpu, Some(true));
    ready(&assistant).await;
    let Some((_, Reminders::Unavailable(why))) = assistant.reminder_generation() else {
        panic!("{:?}", assistant.reminder_generation())
    };
    assert!(why.contains("could not be prepared"), "{why}");
    // The bound is the reminder's run deadline, counted from Ready (a little of it passed).
    let busy = assistant.busy_for_tests();
    assert!(busy > Duration::from_secs(10), "{busy:?}");
    assert_eq!(handle.status().state, State::Ready);
    handle.shutdown().await;
}
