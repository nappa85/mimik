//! `/admin/assistant` (`docs/model-runtime.md`; steps 2c-2a and 2d-1): the owner's
//! view of the model runtime (state, path, confinement level, measurements, budgets, the
//! breaker), its Restart and "Try the GPU again", and the "Model" setting: Try (the trial),
//! Cancel, Switch, Keep and Switch back. Owner only; every action is CSRF-checked and
//! body-bounded, and a second press while a transition or trial runs starts no second one.
use super::materials::{TrustedDraft, html};
use super::{AuthResult, AuthState, csrf_admission, owner_auth};
use crate::assistant::jobs::Reminders;
use crate::assistant::model::{SpecError, parse_spec};
use crate::assistant::runtime::manager::{
    CancelTrial, Chosen, Restart, SetLength, State as Runtime, Status, TryGpu, TryModel,
};
use crate::assistant::runtime::models::{TrialKey, TrialReport, TrialState};
use crate::assistant::runtime::{Level, Path};
use askama::Template;
use axum::{
    Router,
    extract::{DefaultBodyLimit, Form, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect},
    routing::{get, post},
};
use serde::Deserialize;
use tower_sessions::Session;

pub(super) fn routes() -> Router<AuthState> {
    Router::new()
        .route("/admin/assistant", get(page))
        .route("/admin/assistant/restart", post(restart))
        .route("/admin/assistant/try-gpu", post(try_gpu))
        .route(
            "/admin/assistant/model/try",
            post(try_model).layer(DefaultBodyLimit::max(MODEL_FORM_BYTES)),
        )
        .route(
            "/admin/assistant/model/cancel",
            post(cancel_trial).layer(DefaultBodyLimit::max(MODEL_FORM_BYTES)),
        )
        .route(
            "/admin/assistant/model/switch",
            post(switch).layer(DefaultBodyLimit::max(MODEL_FORM_BYTES)),
        )
        .route(
            "/admin/assistant/model/keep",
            post(keep).layer(DefaultBodyLimit::max(MODEL_FORM_BYTES)),
        )
        .route(
            "/admin/assistant/model/switch-back",
            post(switch_back).layer(DefaultBodyLimit::max(MODEL_FORM_BYTES)),
        )
        .route(
            "/admin/assistant/model/length",
            post(set_length).layer(DefaultBodyLimit::max(MODEL_FORM_BYTES)),
        )
        .route(
            "/admin/assistant/reminders",
            post(set_reminders).layer(DefaultBodyLimit::max(MODEL_FORM_BYTES)),
        )
        .merge(super::assistant_downloads::routes())
        .merge(super::assistant_live::routes())
}

/// The model forms are a CSRF token and at most a 512-byte spec or a trial key of 100
/// characters, escaped: 4 KiB is far more than they need.
const MODEL_FORM_BYTES: usize = 4096;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Notice {
    Restarting,
    AlreadyRestarting,
    TryingGpu,
    GpuNotApplicable,
    TryingModel,
    TrialRunning,
    ModelUnsupported,
    TrialCancelled,
    NoTrialRunning,
    Switching,
    SwitchingBack,
    TrialStale,
    NoPreviousModel,
    LengthSet,
    LengthInvalid,
    LengthUnchanged,
    LengthNotStored,
    RemindersOn,
    RemindersOff,
    RemindersInvalid,
    RemindersNotStored,
    Kept,
    NothingToKeep,
    ModelBusy,
    CancelStale,
    Reviewing,
    ReviewBusy,
    ReviewStale,
    DownloadsUnavailable,
    NotADownload,
    DownloadStarted,
    DownloadFailed,
    PartialExists,
    AlreadyConfirmed,
    DownloadCancelled,
    DownloadResuming,
    DownloadDiscarded,
    NoSuchDownload,
    FileDeleted,
    FileKept,
    StaleDownload,
    NothingToRepair,
    DownloadDiscardedFileKept,
}

impl Notice {
    fn text(self) -> &'static str {
        match self {
            Self::Restarting => "Restarting: the model server is being started and measured again.",
            Self::AlreadyRestarting => "A restart is already running; nothing else was started.",
            Self::TryingGpu => {
                "Trying the GPU again: the model server is being started on the GPU and measured again."
            }
            Self::GpuNotApplicable => {
                "Nothing to try: the GPU is in use, or the CPU was chosen with --assistant-device cpu."
            }
            Self::TryingModel => {
                "Trying the model: the current model server stopped, so the assistant is starting until the trial ends. This page updates itself; without JavaScript, reload it to follow the trial."
            }
            Self::TrialRunning => "A trial is already running; nothing else was started.",
            Self::ModelUnsupported => {
                "This assistant keeps no model setting (a development endpoint is in use)."
            }
            Self::TrialCancelled => {
                "Cancelling the trial: the candidate stops and the previous model starts again."
            }
            Self::NoTrialRunning => "No trial is running.",
            Self::Switching => {
                "Switching: the model server restarts with the new model and is measured again."
            }
            Self::SwitchingBack => {
                "Switching back: the model server restarts with the previous model and is measured again."
            }
            Self::TrialStale => {
                "That trial is not the latest finished one, so nothing changed. Try the model again."
            }
            Self::NoPreviousModel => "There is no previous model to switch back to.",
            Self::LengthSet => {
                "Setting the longest reply: the model server restarts once and is measured again. If the new length leaves no usable budgets, the file's earlier setting comes back."
            }
            Self::LengthInvalid => {
                "That is not a reply length this page accepts (a whole number of tokens in the allowed range, in the allowed steps); nothing changed."
            }
            Self::LengthUnchanged => {
                "The active model already has that reply length; nothing changed."
            }
            Self::LengthNotStored => {
                "The reply length could not be stored; nothing changed and the model server was not restarted."
            }
            Self::RemindersOn => "Reminders are on.",
            Self::RemindersOff => "Reminders are off.",
            Self::RemindersInvalid => {
                "That is not a reminder setting this page accepts; nothing changed."
            }
            Self::RemindersNotStored => {
                "The reminder setting could not be stored; nothing changed."
            }
            Self::Kept => "Kept the current model; the trial result is dismissed.",
            Self::NothingToKeep => "There is no trial result to dismiss.",
            Self::ModelBusy => {
                "A model change or a restart is already being carried out; nothing else was started."
            }
            Self::CancelStale => {
                "That page showed an older trial than the one running, so nothing was cancelled. Reload it."
            }
            Self::Reviewing => {
                "Reviewing the repository: the metadata and the licence are being fetched. Nothing downloads until you confirm."
            }
            Self::ReviewBusy => "A review is already running; nothing else was started.",
            Self::ReviewStale => {
                "That review is gone or was replaced, so nothing was confirmed. Review the model again."
            }
            Self::DownloadsUnavailable => "Downloads are not available with this assistant.",
            Self::NotADownload => "That is not an hf: model.",
            Self::DownloadStarted => "Confirmed: the download has started.",
            Self::DownloadFailed => "The download could not be recorded or changed.",
            Self::PartialExists => {
                "A download is already waiting or partly fetched: resume it or discard it first."
            }
            Self::AlreadyConfirmed => "That model was already confirmed for download.",
            Self::DownloadCancelled => "Cancelled: the partial file is kept; Resume continues it.",
            Self::DownloadResuming => "Resuming the download.",
            Self::DownloadDiscarded => "Discarded: the partial file and the record are removed.",
            Self::NoSuchDownload => "There is no such waiting download.",
            Self::FileDeleted => "Deleted the downloaded file.",
            Self::DownloadDiscardedFileKept => {
                "Discarded: the partial file and the record are removed. A file under the download's name was left in place because it is shared, in use, a pinned default, or not exactly this download's verified file."
            }
            Self::StaleDownload => {
                "That page showed another download than the one running, so nothing was cancelled. Reload it."
            }
            Self::NothingToRepair => {
                "Nothing to download again: the file is there (a different file under the name is never replaced)."
            }
            Self::FileKept => {
                "The file was kept: it is in use, shared with another record, a pinned default, or not finished."
            }
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Query_ {
    notice: Option<Notice>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RestartForm {
    csrf: String,
}

struct LevelView {
    number: u8,
    name: &'static str,
    text: &'static str,
}

fn level_view(level: Level) -> LevelView {
    match level {
        Level::Sandbox => LevelView {
            number: 1,
            name: "Hakoniwa",
            text: "private namespaces, so the model server has no network beyond its own loopback and sees only an allowlist of read-only paths; seccomp; it is killed with Mimik. Mimik reaches it only through its socket, checked to be this sandbox's process.",
        },
        Level::Unconfined => LevelView {
            number: 3,
            name: "unconfined",
            text: "not confined: TCP egress allowed, and the whole filesystem readable by Mimik's user is readable by the model server (seccomp only refuses new processes and non-TCP sockets).",
        },
    }
}

struct BudgetsView {
    bytes: usize,
    tokens: usize,
    run: u64,
    depth: usize,
    queue_wait: u64,
    busy: u64,
}

struct SampleView {
    wall: String,
    prompt_n: String,
    prompt: String,
    cached: String,
    predicted_n: String,
    predicted: String,
    filler: String,
}

struct CalibrationView {
    prefix: String,
    density: String,
    samples: Vec<SampleView>,
}

struct DeviceLine {
    description: String,
    name: String,
    pci: String,
    node: String,
    total_mib: u64,
}

struct View {
    llama: String,
    state: &'static str,
    reason: String,
    path: &'static str,
    /// Why the CPU path runs although the GPU is wanted (shown with the retry button).
    gpu_note: Option<String>,
    /// Why Granite runs on the GPU in place of the 9B default (shown with the retry button).
    gpu_default_note: Option<String>,
    /// `--assistant-device cpu`.
    cpu_chosen: bool,
    device: Option<DeviceLine>,
    device_notes: Vec<String>,
    generation: u64,
    failures: u32,
    last_failure: String,
    breaker: String,
    level: Option<LevelView>,
    notes: Vec<String>,
    /// The Landlock layer of the Hakoniwa sandbox: how far it is in force (none until it reports).
    landlock: Option<LandlockLine>,
    skipped: Vec<(u8, String)>,
    budgets: Option<BudgetsView>,
    calibration: Option<CalibrationView>,
    model: ModelView,
    /// The "Reminders" setting and what the generation's measurements say of it.
    reminders: ReminderView,
    /// The running work's step, when the runtime is starting.
    progress: Option<super::assistant_live::ProgressView>,
}

/// The Model section: the active and previous model, the latest trial and the field.
struct ModelView {
    /// The owner can try and switch models.
    switchable: bool,
    active: ModelLine,
    /// Where the active model comes from.
    source: &'static str,
    previous: Option<ModelLine>,
    note: Option<String>,
    /// Why the device default is not the 9B although the GPU is wanted.
    default_note: Option<String>,
    /// The active model's measurement predates the current prompt (ASSIST-56).
    older_prompt: bool,
    trial: Option<TrialView>,
    /// A trial is running: the progress block sits in its section.
    trial_running: bool,
    /// The "Longest reply" setting of the active model.
    length: LengthView,
    /// The field's content: the refused input is kept.
    input: String,
    /// Why the input was refused.
    error: Option<String>,
}

/// The "Longest reply" setting: its bounds, the active value and what it means on the running
/// path (shown once the runtime is measured).
struct LengthView {
    min: u32,
    max: u32,
    step: u32,
    value: u32,
    path: &'static str,
    /// R: a reply can take up to this many seconds.
    run: Option<u64>,
    /// The measured generation time per token.
    per_token: Option<String>,
    /// The longest a player can wait: the queue wait Q plus R, with jobs queued ahead.
    worst: Option<u64>,
}

/// The slower sample's generation time per token, as the page shows it.
fn per_token(status: &Status) -> Option<String> {
    let calibration = status.calibration.as_ref()?;
    let slowest = calibration
        .samples
        .iter()
        .filter(|s| s.predicted_n > 0.0 && s.predicted_ms.is_finite())
        .map(|s| s.predicted_ms / s.predicted_n)
        .fold(f64::NAN, f64::max);
    slowest.is_finite().then(|| format!("{slowest:.1} ms"))
}

fn length_view(status: &Status) -> LengthView {
    use crate::assistant::model::reply_length;
    let run = status.budgets.as_ref().map(|b| b.run.as_secs());
    let worst = status
        .budgets
        .as_ref()
        .map(|b| (b.queue_wait + b.run).as_secs());
    LengthView {
        min: reply_length::MIN,
        max: reply_length::MAX,
        step: reply_length::STEP,
        value: status.model.active.max_tokens,
        path: match status.path {
            Path::Cpu => "CPU",
            Path::Gpu => "GPU",
        },
        run,
        per_token: run.and_then(|_| per_token(status)),
        worst,
    }
}

/// The "Reminders" setting (ASSIST-60) and, for the current generation, what its measurements
/// say: path, threads, model, the estimated time of a reminder, the worst extra wait of a
/// scribble reading right after one, and why reminders are unavailable when they are.
pub(super) struct ReminderView {
    /// The checkbox: the stored choice, else the path's default.
    pub on: bool,
    /// The owner has chosen (a stored choice), as opposed to the default.
    pub chosen: bool,
    /// Path, threads and model of the generation these numbers are measured on.
    pub generation: Option<String>,
    /// About how long a reminder takes (the estimate before the 1.25 margin).
    pub estimate: Option<String>,
    /// The worst extra wait of a scribble reading right after a reminder.
    pub extra_wait: Option<String>,
    /// Why they are unavailable for this generation.
    pub unavailable: Option<String>,
    /// On the CPU path: the catalogue text does not fit.
    pub names_only: bool,
}

pub(super) fn reminder_view(
    choice: Option<bool>,
    generation: Option<(Path, Reminders)>,
    model: &str,
    threads: Option<u32>,
) -> ReminderView {
    let path_name = |path: Path| match path {
        Path::Cpu => "CPU",
        Path::Gpu => "GPU",
    };
    let default_on = matches!(&generation, Some((Path::Gpu, _)));
    let mut view = ReminderView {
        on: choice.unwrap_or(default_on),
        chosen: choice.is_some(),
        generation: None,
        estimate: None,
        extra_wait: None,
        unavailable: None,
        names_only: false,
    };
    let Some((path, reminders)) = generation else {
        return view;
    };
    view.generation = Some(format!(
        "{} path{}, {model}",
        path_name(path),
        threads.map_or_else(String::new, |n| format!(", {n} threads"))
    ));
    view.names_only = path == Path::Cpu;
    match reminders {
        Reminders::Unavailable(why) => view.unavailable = Some(why),
        Reminders::Available(budgets) => {
            view.estimate = Some(format!("{:.0} s", budgets.estimate_s.ceil()));
            view.extra_wait = Some(format!(
                "{} s (the whole prompt of a cancelled reminder, without the chat template's tokens, plus restoring the interpretation prefix{})",
                budgets.extra_wait.as_secs(),
                if budgets.restore.dev_host_constant {
                    "; the restore is a figure measured on the development host, not on this machine"
                } else {
                    ", measured by priming"
                }
            ));
        }
    }
    view
}

struct ModelLine {
    spec: String,
    sha256: String,
    max_tokens: u32,
}

fn model_line(model: &crate::assistant::runtime::models::Model) -> ModelLine {
    ModelLine {
        spec: if model.spec.is_empty() {
            "(not named)".to_owned()
        } else {
            model.spec.clone()
        },
        sha256: if model.sha256.is_empty() {
            "(not recorded)".to_owned()
        } else {
            model.sha256.clone()
        },
        max_tokens: model.max_tokens,
    }
}

struct TrialView {
    number: u64,
    spec: String,
    running: bool,
    /// The phase of a running trial, or why a trial failed or was cancelled.
    detail: String,
    outcome: &'static str,
    report: Option<ReportView>,
}

struct ReportView {
    /// The trial key the "Switch" form carries.
    key: String,
    device: &'static str,
    total: String,
    priming: String,
    prefix_tokens: String,
    max_tokens: u32,
    largest: u32,
    budgets: BudgetsView,
    speeds: Vec<SpeedView>,
    phrases: Vec<PhraseView>,
}

struct SpeedView {
    prompt: String,
    generation: String,
    wall: String,
}

struct PhraseView {
    note: &'static str,
    tokens: String,
    seconds: String,
    offered: &'static str,
}

fn rate(count: f64, ms: f64) -> String {
    if count.is_finite() && ms.is_finite() && ms > 0.0 {
        format!("{:.1} tokens/s", count / (ms / 1000.0))
    } else {
        "unknown".to_owned()
    }
}

fn report_view(report: &TrialReport) -> ReportView {
    let b = &report.budgets;
    ReportView {
        key: report.key.to_string(),
        device: match report.path {
            Path::Cpu => "CPU",
            Path::Gpu => "GPU",
        },
        total: format!("{:.0} s", report.elapsed.as_secs_f64()),
        priming: format!("{:.1} s", report.priming.as_secs_f64()),
        prefix_tokens: count(report.calibration.prefix_tokens),
        max_tokens: report.model.max_tokens,
        largest: report.largest_predicted,
        budgets: BudgetsView {
            bytes: b.bounds.bytes,
            tokens: b.bounds.tokens,
            run: b.run.as_secs(),
            depth: b.depth,
            queue_wait: b.queue_wait.as_secs(),
            busy: b.busy_after_expiry.as_secs(),
        },
        speeds: report
            .calibration
            .samples
            .iter()
            .map(|s| SpeedView {
                prompt: rate(s.prompt_n, s.prompt_ms),
                generation: rate(s.predicted_n, s.predicted_ms),
                wall: seconds(s.wall_ms),
            })
            .collect(),
        phrases: report
            .phrases
            .iter()
            .map(|p| PhraseView {
                note: p.note,
                tokens: count(p.predicted_n),
                seconds: format!("{:.1} s", p.seconds),
                offered: if p.expected_offered { "yes" } else { "no" },
            })
            .collect(),
    }
}

fn trial_view(trial: TrialState) -> TrialView {
    match trial {
        TrialState::Running {
            number,
            spec,
            phase,
        } => TrialView {
            number,
            spec,
            running: true,
            detail: phase.label(),
            outcome: "running",
            report: None,
        },
        TrialState::Passed(report) => TrialView {
            number: report.key.number,
            spec: report.model.spec.clone(),
            running: false,
            detail: String::new(),
            outcome: "passed",
            report: Some(report_view(&report)),
        },
        TrialState::Failed { number, spec, why } => TrialView {
            number,
            spec,
            running: false,
            detail: why,
            outcome: "failed",
            report: None,
        },
        TrialState::Cancelled { number, spec, why } => TrialView {
            number,
            spec,
            running: false,
            detail: why.to_owned(),
            outcome: "cancelled",
            report: None,
        },
    }
}

/// The Landlock line of the Confinement section.
struct LandlockLine {
    state: String,
    /// Said when Landlock is not in full force.
    note: Option<&'static str>,
}

fn landlock_line(landlock: crate::assistant::runtime::hako::Landlock) -> LandlockLine {
    use crate::assistant::runtime::hako::Landlock;
    LandlockLine {
        state: landlock.describe(),
        note: match landlock {
            Landlock::Full { .. } => Some(
                "Mimik asks for the ABI 6 rules. Newer rights, such as connecting to a Unix socket by path (ABI 9), are not asked for.",
            ),
            Landlock::Partial { .. } => Some(
                "This kernel's Landlock lacks part of what Mimik asks for; the private namespaces still confine the model server.",
            ),
            Landlock::Unavailable => Some(
                "This kernel has no Landlock (or it is not enabled); the private namespaces still confine the model server.",
            ),
        },
    }
}

fn seconds(ms: f64) -> String {
    format!("{:.1} s", ms / 1000.0)
}

fn count(n: f64) -> String {
    format!("{n:.0}")
}

fn view(
    status: Status,
    breaker: String,
    input: String,
    error: Option<String>,
    reminders: ReminderView,
) -> View {
    let length = length_view(&status);
    let retry_note = status.gpu_retry_note().map(str::to_owned);
    let (gpu_note, gpu_default_note) = if status.path == Path::Cpu {
        (retry_note, None)
    } else {
        (None, retry_note)
    };
    let progress = super::assistant_live::progress(&status);
    let trial_running = status.trial.as_ref().is_some_and(TrialState::running);
    let (state, reason) = match status.state {
        Runtime::Starting(progress) => ("Starting", progress.phase.label()),
        Runtime::Ready => ("Ready", String::new()),
        Runtime::Unavailable(why) => ("Unavailable", why),
    };
    let pinned = &crate::assistant::runtime::manifest::MANIFEST.llama_cpp;
    let model = ModelView {
        length,
        switchable: status.model.switchable,
        active: model_line(&status.model.active),
        source: if status.model.from_owner_setting {
            "the owner's stored setting (the start flags are ignored)"
        } else if status.model.device_default.is_on() {
            "the default for this device"
        } else {
            "the start flags"
        },
        previous: status.model.previous.as_ref().map(model_line),
        note: status.model.note,
        default_note: status.model.device_default.note().map(str::to_owned),
        older_prompt: status.model.older_prompt.is_older(),
        trial: status.trial.map(trial_view),
        trial_running,
        input,
        error,
    };
    View {
        model,
        reminders,
        progress,
        llama: format!(
            "llama.cpp {} (commit {})",
            pinned.tag,
            pinned.commit.get(..9).unwrap_or(&pinned.commit)
        ),
        state,
        reason,
        path: match status.path {
            Path::Cpu => "CPU",
            Path::Gpu => "GPU",
        },
        gpu_note,
        gpu_default_note,
        cpu_chosen: !status.gpu_wanted,
        device: status.device.map(|device| DeviceLine {
            description: device.description,
            name: device.name,
            pci: device.pci,
            node: device.node,
            total_mib: device.total_mib,
        }),
        device_notes: status.device_notes,
        generation: status.generation,
        failures: status.failures,
        last_failure: status.last_failure.unwrap_or_default(),
        breaker,
        level: status.level.map(level_view),
        notes: status.notes,
        landlock: status.landlock.map(landlock_line),
        skipped: status
            .skipped
            .into_iter()
            .map(|(level, why)| (level.number(), why))
            .collect(),
        budgets: status.budgets.map(|b| BudgetsView {
            bytes: b.bounds.bytes,
            tokens: b.bounds.tokens,
            run: b.run.as_secs(),
            depth: b.depth,
            queue_wait: b.queue_wait.as_secs(),
            busy: b.busy_after_expiry.as_secs(),
        }),
        calibration: status.calibration.map(|c| CalibrationView {
            prefix: count(c.prefix_tokens),
            density: format!("{:.2} and {:.2}", c.density[0], c.density[1]),
            samples: c
                .samples
                .iter()
                .map(|s| SampleView {
                    wall: seconds(s.wall_ms),
                    prompt_n: count(s.prompt_n),
                    prompt: seconds(s.prompt_ms),
                    cached: count(s.cached_n),
                    predicted_n: count(s.predicted_n),
                    predicted: seconds(s.predicted_ms),
                    filler: count(s.filler_tokens),
                })
                .collect(),
        }),
    }
}

#[derive(Template)]
#[template(path = "admin_assistant.html")]
struct Page {
    nav: super::nav::Nav,
    csrf: String,
    notice: &'static str,
    view: Option<View>,
    downloads: Option<super::assistant_downloads_view::DownloadsView>,
    /// The signature and poll interval the script starts from.
    live: Option<super::assistant_live::Live>,
}

/// The page, with the Model field holding `input` and `error` beside it when it was refused.
async fn render(
    state: &AuthState,
    session: &Session,
    notice: &'static str,
    input: &str,
    error: Option<String>,
) -> AuthResult {
    let csrf = super::token(session).await?.ok_or(StatusCode::FORBIDDEN)?;
    let status = state.options.assistant.as_ref().and_then(|handle| {
        handle
            .runtime
            .as_ref()
            .map(crate::assistant::runtime::manager::Handle::status)
    });
    let live_status = status.clone();
    let view = state.options.assistant.as_ref().and_then(|handle| {
        status.map(|status| {
            let reminders = reminder_view(
                handle.assistant.reminder_choice(),
                handle.assistant.reminder_generation(),
                &status.model.active.spec,
                handle.threads,
            );
            view(
                status,
                handle.assistant.breaker_state(),
                input.to_owned(),
                error,
                reminders,
            )
        })
    });
    let downloads = match state.options.assistant.as_ref() {
        Some(handle) if handle.runtime.is_some() => {
            let active = handle
                .runtime
                .as_ref()
                .map(crate::assistant::runtime::manager::Handle::model_files)
                .unwrap_or_default();
            Some(super::assistant_downloads_view::view(handle, &active).await)
        }
        _ => None,
    };
    let live = live_status.map(|status| {
        let (signature, busy) = downloads
            .as_ref()
            .map_or((String::new(), false), |d| (d.signature.clone(), d.polling));
        let reminders = state
            .options
            .assistant
            .as_ref()
            .map_or_else(String::new, |handle| {
                super::assistant_live::reminders_signature(&handle.assistant)
            });
        super::assistant_live::Live::of(&status, &signature, &reminders, busy)
    });
    html(&Page {
        nav: super::nav::Nav::here(super::nav::Section::Assistant),
        csrf,
        notice,
        view,
        downloads,
        live,
    })
}

async fn page(
    State(state): State<AuthState>,
    session: Session,
    Query(query): Query<Query_>,
) -> AuthResult {
    owner_auth(&state, &session).await?;
    let notice = query.notice.map_or("", Notice::text);
    // The field starts with the active model's spec.
    let input = state
        .options
        .assistant
        .as_ref()
        .and_then(|handle| handle.runtime.as_ref())
        .map(|runtime| runtime.status().model.active.spec)
        .unwrap_or_default();
    render(&state, &session, notice, &input, None).await
}

async fn restart(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<RestartForm>,
) -> AuthResult {
    owner_auth(&state, &session).await?;
    csrf_admission(&session, &form.csrf).await?;
    let runtime = state
        .options
        .assistant
        .as_ref()
        .and_then(|handle| handle.runtime.as_ref())
        .ok_or(StatusCode::NOT_FOUND)?;
    let notice = match runtime.restart() {
        Restart::Accepted => "restarting",
        Restart::AlreadyRunning => "already-restarting",
    };
    Ok(Redirect::to(&format!("/admin/assistant?notice={notice}")).into_response())
}

async fn try_gpu(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<RestartForm>,
) -> AuthResult {
    owner_auth(&state, &session).await?;
    csrf_admission(&session, &form.csrf).await?;
    let runtime = state
        .options
        .assistant
        .as_ref()
        .and_then(|handle| handle.runtime.as_ref())
        .ok_or(StatusCode::NOT_FOUND)?;
    let notice = match runtime.try_gpu() {
        TryGpu::Accepted => "trying-gpu",
        TryGpu::AlreadyRunning => "already-restarting",
        TryGpu::NotApplicable => "gpu-not-applicable",
    };
    Ok(Redirect::to(&format!("/admin/assistant?notice={notice}")).into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TryForm {
    csrf: String,
    model: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CancelForm {
    csrf: String,
    /// The number of the trial the page showed.
    trial: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LengthForm {
    csrf: String,
    /// The reply length in tokens, as typed (parsed and checked by the route).
    max_tokens: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemindersForm {
    csrf: String,
    /// `on` or `off`.
    reminders: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SwitchForm {
    csrf: String,
    trial: String,
}

fn redirect(notice: &str) -> axum::response::Response {
    Redirect::to(&format!("/admin/assistant?notice={notice}")).into_response()
}

/// The runtime manager, if the assistant has one.
pub(super) fn runtime(state: &AuthState) -> Option<&crate::assistant::runtime::manager::Handle> {
    state
        .options
        .assistant
        .as_ref()
        .and_then(|handle| handle.runtime.as_ref())
}

/// The assistant's job state, if there is an assistant.
pub(super) fn runtime_assistant(state: &AuthState) -> Option<&crate::assistant::jobs::Assistant> {
    state
        .options
        .assistant
        .as_ref()
        .map(|handle| handle.assistant.as_ref())
}

fn runtime_of(
    state: &AuthState,
) -> Result<&crate::assistant::runtime::manager::Handle, StatusCode> {
    state
        .options
        .assistant
        .as_ref()
        .and_then(|handle| handle.runtime.as_ref())
        .ok_or(StatusCode::NOT_FOUND)
}

/// "Try": the field's spec by the grammar, then the trial. A refused spec shows the page
/// again with the rule named and the input kept.
async fn try_model(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<TryForm>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    csrf_admission(&session, &form.csrf).await?;
    let runtime = runtime_of(&state)?;
    let spec = match parse_spec(&form.model) {
        Ok(spec) => spec,
        Err(refused) => return refused_spec(&state, &session, &form.model, refused).await,
    };
    let name = match spec {
        crate::assistant::model::Spec::Local(name) => name,
        // Reviewed and downloaded by the Downloads panel (step 2d-2, routes below).
        crate::assistant::model::Spec::Hf(hf) => {
            return Ok(super::assistant_downloads::start_review(&state, hf));
        }
    };
    Ok(redirect(
        match runtime.try_model(auth.account.id, form.model, name) {
            TryModel::Accepted(_) => "trying-model",
            TryModel::Busy => "trial-running",
            TryModel::Pending => "model-busy",
            TryModel::Unsupported => "model-unsupported",
        },
    ))
}

async fn refused_spec(
    state: &AuthState,
    session: &Session,
    input: &str,
    refused: SpecError,
) -> AuthResult {
    let mut response = render(state, session, "", input, Some(refused.to_string())).await?;
    *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
    response.extensions_mut().insert(TrustedDraft);
    Ok(response)
}

async fn cancel_trial(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<CancelForm>,
) -> AuthResult {
    owner_auth(&state, &session).await?;
    csrf_admission(&session, &form.csrf).await?;
    let runtime = runtime_of(&state)?;
    Ok(redirect(match runtime.cancel_trial(form.trial) {
        CancelTrial::Done => "trial-cancelled",
        CancelTrial::NoTrial => "no-trial-running",
        CancelTrial::Stale => "cancel-stale",
    }))
}

async fn switch(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<SwitchForm>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    csrf_admission(&session, &form.csrf).await?;
    let runtime = runtime_of(&state)?;
    // A key that does not parse names no trial: stale, nothing changes.
    let Ok(key) = form.trial.parse::<TrialKey>() else {
        return Ok(redirect("trial-stale"));
    };
    Ok(redirect(match runtime.switch(auth.account.id, &key) {
        Chosen::Accepted => "switching",
        Chosen::Busy => "trial-running",
        Chosen::Pending => "model-busy",
        Chosen::Stale | Chosen::NoPrevious => "trial-stale",
    }))
}

async fn keep(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<RestartForm>,
) -> AuthResult {
    owner_auth(&state, &session).await?;
    csrf_admission(&session, &form.csrf).await?;
    let runtime = runtime_of(&state)?;
    Ok(redirect(match runtime.keep() {
        Chosen::Accepted => "kept",
        Chosen::Busy => "trial-running",
        Chosen::Pending => "model-busy",
        Chosen::Stale | Chosen::NoPrevious => "nothing-to-keep",
    }))
}

async fn switch_back(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<RestartForm>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    csrf_admission(&session, &form.csrf).await?;
    let runtime = runtime_of(&state)?;
    Ok(redirect(match runtime.switch_back(auth.account.id) {
        Chosen::Accepted => "switching-back",
        Chosen::Busy => "trial-running",
        Chosen::Pending => "model-busy",
        Chosen::Stale | Chosen::NoPrevious => "no-previous-model",
    }))
}

/// "Longest reply": the form's text is parsed as a number here; the runtime manager alone
/// validates it against `model::reply_length`'s bounds and steps. Anything it refuses changes
/// nothing.
async fn set_length(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<LengthForm>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    csrf_admission(&session, &form.csrf).await?;
    let runtime = runtime_of(&state)?;
    let Ok(max_tokens) = form.max_tokens.trim().parse::<u32>() else {
        return Ok(redirect("length-invalid"));
    };
    Ok(redirect(
        match runtime.set_length(auth.account.id, max_tokens).await {
            SetLength::Accepted => "length-set",
            SetLength::Busy => "trial-running",
            SetLength::Pending => "model-busy",
            SetLength::Invalid => "length-invalid",
            SetLength::Unchanged => "length-unchanged",
            SetLength::NotStored => "length-not-stored",
            SetLength::Unsupported => "model-unsupported",
        },
    ))
}

/// "Reminders": stored first (migration 035), then held with the runtime state, so the save
/// answer and the route see it without a database read. No restart follows.
async fn set_reminders(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<RemindersForm>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    csrf_admission(&session, &form.csrf).await?;
    let handle = state
        .options
        .assistant
        .as_ref()
        .ok_or(StatusCode::NOT_FOUND)?;
    handle.runtime.as_ref().ok_or(StatusCode::NOT_FOUND)?;
    let enabled = match form.reminders.as_str() {
        "on" => true,
        "off" => false,
        _ => return Ok(redirect("reminders-invalid")),
    };
    if handle
        .store_reminders(auth.account.id, enabled)
        .await
        .is_err()
    {
        return Ok(redirect("reminders-not-stored"));
    }
    Ok(redirect(if enabled {
        "reminders-on"
    } else {
        "reminders-off"
    }))
}

#[cfg(test)]
mod tests {
    use super::{ReminderView, landlock_line, reminder_view};
    use crate::assistant::jobs::Reminders;
    use crate::assistant::runtime::Path;
    use crate::assistant::runtime::hako::Landlock;
    use crate::assistant::runtime::reminder_budgets::{ReminderBudgets, Restore};
    use std::time::Duration;

    fn budgets(dev_host: bool) -> Reminders {
        Reminders::Available(ReminderBudgets {
            run: Duration::from_secs(150),
            estimate_s: 118.2,
            busy: Duration::from_secs(80),
            restore: Restore {
                seconds: 0.3,
                dev_host_constant: dev_host,
            },
            reply_tokens: 600,
            prefix_tokens: 1500,
            extra_wait: Duration::from_secs(81),
        })
    }

    fn named(view: &ReminderView) -> (bool, bool) {
        (view.on, view.chosen)
    }

    #[test]
    fn the_reminder_setting_defaults_per_path_and_a_stored_choice_wins() {
        // No generation yet: off until the path is known; a stored choice shows as stored.
        assert_eq!(named(&reminder_view(None, None, "m", None)), (false, false));
        assert_eq!(
            named(&reminder_view(Some(true), None, "m", None)),
            (true, true)
        );
        let cpu = Some((Path::Cpu, budgets(true)));
        let gpu = Some((Path::Gpu, budgets(true)));
        assert_eq!(
            named(&reminder_view(None, cpu.clone(), "m", None)),
            (false, false)
        );
        assert_eq!(
            named(&reminder_view(None, gpu.clone(), "m", None)),
            (true, false)
        );
        assert_eq!(
            named(&reminder_view(Some(false), gpu, "m", None)),
            (false, true)
        );
        assert_eq!(
            named(&reminder_view(Some(true), cpu, "m", None)),
            (true, true)
        );
    }

    #[test]
    fn the_estimates_come_from_the_measurements_and_the_cpu_says_names_only() {
        let view = reminder_view(None, Some((Path::Cpu, budgets(true))), "granite", Some(2));
        assert_eq!(
            view.generation.as_deref(),
            Some("CPU path, 2 threads, granite")
        );
        assert_eq!(view.estimate.as_deref(), Some("119 s"));
        let wait = view.extra_wait.unwrap();
        assert!(wait.starts_with("81 s"), "{wait}");
        assert!(wait.contains("without the chat template's tokens"));
        assert!(wait.contains("development host"));
        assert!(view.names_only && view.unavailable.is_none());
        // The GPU path measures the restore by priming, and sends catalogue text later.
        let gpu = reminder_view(None, Some((Path::Gpu, budgets(false))), "qwen", None);
        assert!(!gpu.names_only);
        assert!(gpu.extra_wait.unwrap().contains("measured by priming"));
    }

    #[test]
    fn a_generation_that_cannot_run_reminders_says_why_and_shows_no_estimate() {
        let view = reminder_view(
            Some(true),
            Some((
                Path::Cpu,
                Reminders::Unavailable(
                    "a reminder would need a run deadline of 700 s, over the 600 s ceiling".into(),
                ),
            )),
            "m",
            None,
        );
        assert!(view.unavailable.unwrap().contains("600 s ceiling"));
        assert!(view.estimate.is_none() && view.extra_wait.is_none());
    }

    #[test]
    fn the_page_says_how_far_landlock_is_in_force_and_what_stands_alone_otherwise() {
        let full = landlock_line(Landlock::Full { abi: 11 });
        assert_eq!(full.state, "full for the ABI 6 rules (kernel ABI 11)");
        // Newer rights are not asked for, and the page says so.
        assert!(full.note.is_some_and(|note| note.contains("ABI 9")));
        let partial = landlock_line(Landlock::Partial { abi: 3 });
        assert_eq!(partial.state, "partial (kernel ABI 3)");
        assert!(partial.note.is_some_and(|note| note.contains("namespaces")));
        let none = landlock_line(Landlock::Unavailable);
        assert_eq!(none.state, "not available");
        assert!(none.note.is_some_and(|note| note.contains("namespaces")));
    }
}
