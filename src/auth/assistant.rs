//! `/admin/assistant` (assisted sheet plan §1.4b, "Page"; step 2c-2a): the owner's view of
//! the model runtime (state, path, confinement level, measurements, budgets, the breaker)
//! and its Restart and "Try the GPU again". Owner only; both are CSRF-checked, and a second
//! press while either runs starts no second transition.
use super::{AuthResult, AuthState, csrf_admission, materials::html, owner_auth};
use crate::assistant::runtime::manager::{Restart, State as Runtime, Status, TryGpu};
use crate::assistant::runtime::{Level, Path};
use askama::Template;
use axum::{
    Router,
    extract::{Form, Query, State},
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
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Notice {
    Restarting,
    AlreadyRestarting,
    TryingGpu,
    GpuNotApplicable,
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
        Level::Bubblewrap => LevelView {
            number: 1,
            name: "bubblewrap",
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
    /// The GPU is wanted but not in use: why, with the "Try the GPU again" button.
    gpu_note: Option<String>,
    /// `--assistant-device cpu`.
    cpu_chosen: bool,
    /// A Restart or "Try the GPU again" is being carried out (the button is disabled).
    restarting: bool,
    device: Option<DeviceLine>,
    device_notes: Vec<String>,
    generation: u64,
    failures: u32,
    last_failure: String,
    breaker: String,
    level: Option<LevelView>,
    notes: Vec<String>,
    /// The Landlock layer of the Hakoniwa sandbox: how far it is in force (none with bubblewrap).
    landlock: Option<LandlockLine>,
    skipped: Vec<(u8, String)>,
    budgets: Option<BudgetsView>,
    calibration: Option<CalibrationView>,
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
            Landlock::Full { .. } => None,
            Landlock::Partial { .. } => Some(
                "This kernel's Landlock lacks part of what Mimik asks for; the private namespaces still confine the model server, as with bubblewrap.",
            ),
            Landlock::Unavailable => Some(
                "This kernel has no Landlock (or it is not enabled); the private namespaces still confine the model server, as with bubblewrap.",
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

fn view(status: Status, breaker: String) -> View {
    let (state, reason) = match status.state {
        Runtime::Starting(phase) => ("Starting", phase.to_owned()),
        Runtime::Ready => ("Ready", String::new()),
        Runtime::Unavailable(why) => ("Unavailable", why),
    };
    let pinned = &crate::assistant::runtime::manifest::MANIFEST.llama_cpp;
    View {
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
        gpu_note: if status.gpu_wanted && status.path == Path::Cpu {
            Some(
                status
                    .gpu_note
                    .unwrap_or_else(|| "the GPU path has not been tried yet".to_owned()),
            )
        } else {
            None
        },
        cpu_chosen: !status.gpu_wanted,
        restarting: status.restarting || state == "Starting",
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
    csrf: String,
    notice: &'static str,
    view: Option<View>,
}

async fn page(
    State(state): State<AuthState>,
    session: Session,
    Query(query): Query<Query_>,
) -> AuthResult {
    owner_auth(&state, &session).await?;
    let csrf = super::token(&session).await?.ok_or(StatusCode::FORBIDDEN)?;
    let view = state.options.assistant.as_ref().and_then(|handle| {
        handle
            .runtime
            .as_ref()
            .map(|runtime| view(runtime.status(), handle.assistant.breaker_state()))
    });
    html(&Page {
        csrf,
        notice: query.notice.map_or("", Notice::text),
        view,
    })
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

#[cfg(test)]
mod tests {
    use super::landlock_line;
    use crate::assistant::runtime::hako::Landlock;

    #[test]
    fn the_page_says_how_far_landlock_is_in_force_and_what_stands_alone_otherwise() {
        let full = landlock_line(Landlock::Full { abi: 11 });
        assert_eq!(full.state, "full (ABI 11)");
        assert!(full.note.is_none());
        let partial = landlock_line(Landlock::Partial { abi: 3 });
        assert_eq!(partial.state, "partial (ABI 3)");
        assert!(partial.note.is_some_and(|note| note.contains("namespaces")));
        let none = landlock_line(Landlock::Unavailable);
        assert_eq!(none.state, "not available");
        assert!(none.note.is_some_and(|note| note.contains("bubblewrap")));
    }
}
