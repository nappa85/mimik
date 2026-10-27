//! The owner's model requests (`docs/model-runtime.md`, "Owner model setting"): the trial of a candidate
//! file, "Switch" and "Switch back". Each runs with no model server of the active model
//! running (the session that held it has ended), inside the one manager task, so two model
//! servers never coexist; every trial ends by stopping its candidate, and a session with the
//! previous (or, after a switch, the new) model starts next.
use super::{
    Arc, AtomicBool, Attempt, Child, Command, DeviceDefault, EndReason, Fail, Future, Instant,
    Launcher, Level, Manager, Model, ModelCommand, Mutex, Next, NotCalibrated, Ordering, Path,
    Record, State, Status, TrialKey, TrialReport, TrialRequest, TrialState, calibrate_budgets, log,
    measured, phrase_bound, trial, trial_bound, warm,
};
use crate::assistant::model::{default_alias, trial_max_tokens};

/// Why a trial stopped before it finished.
enum Stopped {
    /// The owner's Restart, Shutdown or another command arrived.
    Command(Command),
    /// The owner's Cancel.
    Cancelled,
}

/// One phase name per phrase, for the page.
const PHRASE_PHASES: [&str; 5] = [
    "trial phrase 1 of 5",
    "trial phrase 2 of 5",
    "trial phrase 3 of 5",
    "trial phrase 4 of 5",
    "trial phrase 5 of 5",
];

fn too_long(spec: &str, path: Path) -> String {
    let minutes = trial_bound(path).as_secs() / 60;
    format!("the trial of {spec} took longer than its {minutes} minutes")
}

/// What a running trial needs, apart from the manager.
struct Plan {
    number: u64,
    by: i64,
    spec: String,
    generation: u64,
    level: Level,
    path: Path,
    /// The candidate, with the measurement cap as `max_tokens`.
    candidate: Model,
}

impl Manager {
    /// Carries out a model request with no model server running. `Some(Next::Stop)` when
    /// Mimik is shutting down meanwhile; otherwise the caller starts a session.
    pub(super) async fn model_command(&mut self, command: ModelCommand) -> Option<Next> {
        match command {
            ModelCommand::Trial(request) => self.trial(request).await,
            ModelCommand::Switch { key, by } => {
                self.switch(&key, by).await;
                None
            }
            ModelCommand::SwitchBack { by } => {
                self.switch_back(by).await;
                None
            }
        }
    }

    fn set_trial(&self, state: TrialState) {
        self.update(|status| status.trial = Some(state));
    }

    fn note(&self, text: String) {
        log(&text);
        self.update(|status| status.model.note = Some(text));
    }

    /// Stops the launcher's in-flight work (a blocking hash, a start) and waits until it has
    /// ended, so it does not delay the next session.
    async fn stop_work(&self) {
        self.launcher.cancel();
        self.launcher.settle().await;
    }

    /// Whether the owner pressed Cancel for the trial being carried out.
    fn cancelled(&self) -> bool {
        self.cancel.trial.load(Ordering::SeqCst) == self.trial
    }

    /// Runs `work` unless a command or the owner's Cancel for this trial comes first.
    async fn guarded<T>(&mut self, work: impl Future<Output = T>) -> Result<T, Stopped> {
        tokio::pin!(work);
        loop {
            if self.cancelled() {
                return Err(Stopped::Cancelled);
            }
            tokio::select! {
                done = &mut work => return Ok(done),
                command = self.commands.recv() => match command {
                    Some(command) => return Err(Stopped::Command(command)),
                    None => std::future::pending().await,
                },
                // A wake-up for another trial changes nothing: look again.
                () = self.cancel.wake.notified() => {}
            }
        }
    }

    /// Records how a stopped trial ended and what the caller does next.
    fn stopped(&mut self, number: u64, spec: String, stopped: Stopped) -> Option<Next> {
        let (why, next) = match stopped {
            Stopped::Cancelled => ("cancelled by the owner", None),
            Stopped::Command(Command::Restart { gpu }) => {
                ("ended by a Restart", Some(Next::Fresh { gpu }))
            }
            Stopped::Command(Command::Shutdown(reply)) => {
                ("ended by Mimik shutting down", Some(Next::Stop(reply)))
            }
            // The handle refuses these while a trial runs.
            Stopped::Command(Command::Model(_)) => ("ended by another model request", None),
        };
        log(&format!("trial {number} of {spec}: {why}"));
        self.set_trial(TrialState::Cancelled { number, spec, why });
        next
    }

    fn failed_trial(&self, number: u64, spec: String, why: String) {
        log(&format!("trial {number} of {spec} failed: {why}"));
        self.set_trial(TrialState::Failed { number, spec, why });
    }

    /// The trial: hash the file, start the candidate on the current path, prime it, send the
    /// phrases, derive `max_tokens`, calibrate. Whatever happens, the candidate is stopped
    /// and the caller starts a session with the previous model.
    async fn trial(&mut self, request: TrialRequest) -> Option<Next> {
        let TrialRequest {
            number,
            by,
            spec,
            name,
        } = request;
        // Nothing of the earlier session runs beside the candidate.
        self.launcher.settle().await;
        self.trial = number;
        let path = self.status().path;
        // The whole trial, hashing included, is bounded.
        let deadline = Instant::now() + trial_bound(path);
        log(&format!("trial {number} of {spec} on the {path:?} path"));
        self.update(|status| {
            status.state = State::Starting("trying a model");
            status.budgets = None;
        });
        let candidate = match self
            .hash_candidate(number, &spec, name, path, deadline)
            .await
        {
            Ok(candidate) => candidate,
            Err(next) => return next,
        };
        self.run_candidate(number, by, spec, path, candidate, deadline)
            .await
    }

    /// The candidate with the hash of its file, or how the trial ended instead (`Some` when
    /// Mimik is shutting down).
    async fn hash_candidate(
        &mut self,
        number: u64,
        spec: &str,
        name: String,
        path: Path,
        deadline: Instant,
    ) -> Result<Model, Option<Next>> {
        self.set_trial(TrialState::Running {
            number,
            spec: spec.to_owned(),
            phase: "hashing the model file",
        });
        let launcher = Arc::clone(&self.launcher);
        let hashed = self
            .guarded(tokio::time::timeout_at(
                deadline,
                launcher.hash_model(&name),
            ))
            .await;
        let failed = |manager: &Self, why: String| {
            manager.failed_trial(number, spec.to_owned(), why);
            Err(None)
        };
        let file = match hashed {
            Ok(Ok(Ok(file))) => file,
            Ok(Ok(Err(why))) => return failed(self, why),
            Ok(Err(_)) => {
                self.stop_work().await;
                return failed(self, too_long(spec, path));
            }
            Err(stopped) => {
                self.stop_work().await;
                return Err(self.stopped(number, spec.to_owned(), stopped));
            }
        };
        if file.size == 0 || file.size > crate::assistant::model::MAX_MODEL_BYTES {
            let what = if file.size == 0 {
                "is empty".to_owned()
            } else {
                "is larger than 8 GiB, the most a model file may be".to_owned()
            };
            return failed(self, format!("{name} {what}"));
        }
        match default_alias(spec) {
            Ok(alias) => Ok(Model {
                spec: spec.to_owned(),
                name,
                sha256: file.sha256,
                alias,
                max_tokens: crate::assistant::model::TRIAL_MEASURE_TOKENS,
                size: Some(file.size),
            }),
            Err(why) => failed(self, why),
        }
    }

    /// Starts `candidate`, measures it, stops it, and records how the trial ended.
    async fn run_candidate(
        &mut self,
        number: u64,
        by: i64,
        spec: String,
        path: Path,
        candidate: Model,
        deadline: Instant,
    ) -> Option<Next> {
        let Some(&level) = self.config.levels.first() else {
            self.failed_trial(
                number,
                spec,
                "no confinement level may start a model".into(),
            );
            return None;
        };
        let launcher = Arc::clone(&self.launcher);
        launcher.select(&candidate);
        let generation = self.begin(level);
        let plan = Plan {
            number,
            by,
            spec: spec.clone(),
            generation,
            level,
            path,
            candidate,
        };
        let slot: Arc<Mutex<Option<Arc<dyn Child>>>> = Arc::default();
        let work = trial_work(
            Arc::clone(&launcher),
            Arc::clone(&self.status),
            plan,
            Arc::clone(&slot),
        );
        let outcome = self.guarded(tokio::time::timeout_at(deadline, work)).await;
        let interrupted = !matches!(outcome, Ok(Ok(_)));
        if interrupted {
            // The start or a probe may still be running: tell it to stop at its next check.
            launcher.cancel();
        }
        let child = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(child) = child {
            child.stop().await;
        }
        if interrupted {
            // Only now: the launcher's exit watcher holds its ticket until the child is gone,
            // so settling before the stop would wait for a child nothing stops.
            launcher.settle().await;
        }
        self.ended(generation, EndReason::Exited);
        match outcome {
            Ok(Ok(Ok(report))) => {
                log(&format!(
                    "trial {number} of {spec} passed: {}",
                    summary(&report)
                ));
                self.set_trial(TrialState::Passed(Box::new(report)));
                None
            }
            Ok(Ok(Err(why))) => {
                self.failed_trial(number, spec, why);
                None
            }
            Ok(Err(_)) => {
                let why = too_long(&spec, path);
                self.failed_trial(number, spec, why);
                None
            }
            Err(stopped) => self.stopped(number, spec, stopped),
        }
    }

    /// "Switch": the latest passed trial becomes the active model, stored first.
    async fn switch(&mut self, key: &TrialKey, by: i64) {
        let report = match self.status().trial {
            Some(TrialState::Passed(report)) if report.key == *key => report,
            _ => {
                self.note("that trial is not the latest finished one; try the model again".into());
                return;
            }
        };
        let Some(records) = self.records.clone() else {
            return;
        };
        let new = report.model.clone();
        let old = self.active.clone();
        // Trying the file that is already active again keeps the model before it.
        let keeps_previous = old.sha256 == new.sha256 && old.name == new.name;
        let stored = records
            .activate(
                &Record {
                    model: new.clone(),
                    trial: Some(trial_json(&report)),
                },
                (!keeps_previous).then_some(&Record {
                    model: old.clone(),
                    trial: None,
                }),
                by,
            )
            .await;
        if let Err(why) = stored {
            self.note(format!(
                "the switch could not be stored ({why}); the current model stays"
            ));
            return;
        }
        log(&format!(
            "model switched to {} (max_tokens {})",
            new.spec, new.max_tokens
        ));
        if !keeps_previous {
            self.previous = Some(old);
        }
        self.active = new;
        self.device_default = false;
        self.publish_models();
        // Only the trial this switch consumed.
        self.update(|status| {
            if matches!(&status.trial, Some(TrialState::Passed(r)) if r.key == *key) {
                status.trial = None;
            }
        });
    }

    /// "Switch back": the previous model becomes the active one.
    async fn switch_back(&mut self, by: i64) {
        let (Some(previous), Some(records)) = (self.previous.clone(), self.records.clone()) else {
            return;
        };
        if let Err(why) = records.swap(by).await {
            self.note(format!(
                "switching back could not be stored ({why}); the current model stays"
            ));
            return;
        }
        log(&format!("model switched back to {}", previous.spec));
        self.previous = Some(std::mem::replace(&mut self.active, previous));
        self.device_default = false;
        self.publish_models();
    }

    fn publish_models(&self) {
        let (active, previous) = (self.active.clone(), self.previous.clone());
        self.update(|status| {
            status.model.active = active;
            status.model.previous = previous;
            status.model.from_owner_setting = true;
            status.model.device_default = DeviceDefault::Off;
        });
    }
}

/// The trial's work, within the manager's guard: start the candidate, then measure it.
async fn trial_work(
    launcher: Arc<dyn Launcher>,
    status: Arc<Mutex<Status>>,
    plan: Plan,
    slot: Arc<Mutex<Option<Arc<dyn Child>>>>,
) -> Result<TrialReport, String> {
    let phase = |phase: &'static str| {
        let mut status = status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        status.state = State::Starting(phase);
        status.trial = Some(TrialState::Running {
            number: plan.number,
            spec: plan.spec.clone(),
            phase,
        });
    };
    phase("checking the host");
    launcher.preflight(Path::Cpu).await?;
    let probed = launcher.probe(plan.path).await;
    match probed.into_iter().find(|(level, _)| *level == plan.level) {
        Some((_, Ok(()))) => {}
        Some((_, Err(why))) => return Err(format!("the sandbox cannot be used: {why}")),
        None => return Err("the sandbox cannot be used".into()),
    }
    phase("starting the model server");
    let child = launcher
        .start(plan.generation, plan.level, plan.path)
        .await
        .map_err(|fail| match fail {
            Fail::Sandbox(why) | Fail::Other(why) => why,
        })?;
    *slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&child));
    tokio::select! {
        measured = measure_trial(&child, &plan, &phase) => measured,
        fail = child.exited() => Err(match fail {
            Fail::Sandbox(why) | Fail::Other(why) => why,
        }),
    }
}

/// Health, priming, the five phrases, `max_tokens`, calibration.
async fn measure_trial(
    child: &Arc<dyn Child>,
    plan: &Plan,
    phase: &(dyn Fn(&'static str) + Sync),
) -> Result<TrialReport, String> {
    let began = Instant::now();
    let answered = AtomicBool::new(false);
    let (prefix, priming) = warm(child, phase, &answered)
        .await
        .map_err(|failed| match failed {
            Attempt::Failed(Fail::Sandbox(why) | Fail::Other(why)) | Attempt::TooSlow(why) => why,
            Attempt::Ready(..) | Attempt::Interrupted(_) => "interrupted".into(),
        })?;
    let server = child.server();
    let phrases = trial::run(server.as_ref(), phrase_bound(plan.path), &|n| {
        phase(PHRASE_PHASES[n - 1]);
    })
    .await
    .map_err(|failure| failure.describe())?;
    let largest = trial::largest_predicted(&phrases);
    let max_tokens = trial_max_tokens(&plan.candidate.sha256, largest);
    phase("calibrating");
    let (calibration, budgets) = calibrate_budgets(server.as_ref(), plan.path, max_tokens, prefix)
        .await
        .map_err(|failed| match failed {
            NotCalibrated::TooSlow(why, _) | NotCalibrated::Failed(why) => why,
        })?;
    let model = Model {
        max_tokens,
        ..plan.candidate.clone()
    };
    Ok(TrialReport {
        key: TrialKey {
            number: plan.number,
            generation: plan.generation,
            sha256: model.sha256.clone(),
        },
        by: plan.by,
        model,
        path: plan.path,
        priming,
        phrases,
        largest_predicted: largest,
        calibration,
        budgets,
        elapsed: began.elapsed(),
    })
}

/// One log line of a passed trial.
fn summary(report: &TrialReport) -> String {
    format!(
        "max_tokens {} (largest phrase reply {} tokens), {}",
        report.model.max_tokens,
        report.largest_predicted,
        measured(&report.calibration, Some(&report.budgets))
    )
}

/// The trial as the model record keeps it.
fn trial_json(report: &TrialReport) -> String {
    let samples: Vec<_> = report
        .calibration
        .samples
        .iter()
        .map(|s| {
            serde_json::json!({
                "wall_ms": s.wall_ms,
                "prompt_n": s.prompt_n,
                "prompt_ms": s.prompt_ms,
                "cached_n": s.cached_n,
                "predicted_n": s.predicted_n,
                "predicted_ms": s.predicted_ms,
                "filler_tokens": s.filler_tokens,
            })
        })
        .collect();
    let phrases: Vec<_> = report
        .phrases
        .iter()
        .map(|p| {
            serde_json::json!({
                "predicted_n": p.predicted_n,
                "seconds": p.seconds,
                "expected_offered": p.expected_offered,
            })
        })
        .collect();
    serde_json::json!({
        "trial": report.key.number,
        "generation": report.key.generation,
        "device": match report.path { Path::Cpu => "cpu", Path::Gpu => "gpu" },
        "priming_s": report.priming.as_secs_f64(),
        "prefix_tokens": report.calibration.prefix_tokens,
        "phrases": phrases,
        "largest_predicted_n": report.largest_predicted,
        "max_tokens": report.model.max_tokens,
        "samples": samples,
        "budgets": {
            "bytes": report.budgets.bounds.bytes,
            "tokens": report.budgets.bounds.tokens,
            "run_s": report.budgets.run.as_secs(),
            "depth": report.budgets.depth,
            "queue_s": report.budgets.queue_wait.as_secs(),
            "busy_after_expiry_s": report.budgets.busy_after_expiry.as_secs(),
        },
        "elapsed_s": report.elapsed.as_secs_f64(),
    })
    .to_string()
}

#[cfg(test)]
#[path = "manager_model_tests.rs"]
mod tests;
