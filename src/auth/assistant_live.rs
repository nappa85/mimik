//! The assistant page's live state (`docs/model-runtime.md`, "Live progress"): one status
//! route and one script for the whole page. The server decides everything the script shows:
//! the poll interval, the step texts, the numbers and whether the page changed (the
//! signature); the script only sets text, `value` and `max`.
//!
//! The signature is one function of one status snapshot. It names what changes the page (a
//! kind of state, the models, a note, a trial's outcome, the downloads' state), never a step,
//! a byte count or a time, so the page reloads on an outcome and not on every step. It is sent
//! as the SHA-256 of that text in lowercase hex: a reason with a line break or a NUL would
//! not survive an HTML attribute unchanged, and the page and the answer must agree for good.
use super::assistant_downloads;
use super::{AuthState, owner_auth};
use crate::assistant::runtime::manager::{State as Runtime, Status};
use crate::assistant::runtime::models::{Model, TrialState};
use axum::{
    Json, Router,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::json;
use sha2::{Digest, Sha256};

/// How often the page asks while work runs, and while nothing does (a crash restart, a
/// change from another tab and the breaker still show up).
pub(super) const BUSY_POLL_MS: u32 = 2_000;
pub(super) const IDLE_POLL_MS: u32 = 15_000;

pub(super) fn routes() -> Router<AuthState> {
    Router::new()
        .route("/admin/assistant/status.json", get(status_json))
        .route(
            "/assets/assistant-live.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../assets/assistant-live.js"),
                )
            }),
        )
}

/// Work runs: the model server starts, restarts or is being changed, a trial runs, or a
/// review or a download does.
pub(super) fn busy(status: &Status, downloads_busy: bool) -> bool {
    matches!(status.state, Runtime::Starting(_))
        || status.restarting
        || status.model.pending
        || status.trial.as_ref().is_some_and(TrialState::running)
        || downloads_busy
}

pub(super) fn poll_ms(busy: bool) -> u32 {
    if busy { BUSY_POLL_MS } else { IDLE_POLL_MS }
}

fn model(model: &Model) -> String {
    format!("{} {} {}", model.spec, model.sha256, model.max_tokens)
}

/// What changes the page, as a plain joined string. `downloads` is the downloads' own
/// signature (empty without downloads). The generation is part of it only while idle: while
/// work runs the page shows it in place.
pub(super) fn signature(status: &Status, downloads: &str) -> String {
    let state = match &status.state {
        Runtime::Starting(_) => "starting".to_owned(),
        Runtime::Ready => "ready".to_owned(),
        Runtime::Unavailable(why) => format!("unavailable:{why}"),
    };
    let trial = match &status.trial {
        None => "none".to_owned(),
        Some(TrialState::Running { number, .. }) => format!("running {number}"),
        Some(TrialState::Passed(report)) => format!("passed {}", report.key),
        Some(TrialState::Failed { number, .. }) => format!("failed {number}"),
        Some(TrialState::Cancelled { number, .. }) => format!("cancelled {number}"),
    };
    let generation = if busy(status, false) {
        String::new()
    } else {
        status.generation.to_string()
    };
    let text = [
        state,
        model(&status.model.active),
        status
            .model
            .previous
            .as_ref()
            .map(model)
            .unwrap_or_default(),
        status.model.note.clone().unwrap_or_default(),
        trial,
        format!("restarting {}", status.restarting),
        format!("pending {}", status.model.pending),
        format!("budgets {}", status.budgets.is_some()),
        generation,
        downloads.to_owned(),
    ]
    .join("|");
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// The running work as the page shows it.
pub(super) struct ProgressView {
    /// The step ("Trial of local:x.gguf: step 8 of 13, trial phrase 1 of 5"): in the live
    /// element, changed only when the step changes.
    pub text: String,
    pub step: usize,
    pub steps: usize,
    /// The steps finished: the bar's value, so it is full only when the work is.
    pub done: usize,
    /// Elapsed time and what is left of it: a separate, non-live line.
    pub time: String,
}

/// The progress of the work the runtime is doing, if it is starting.
pub(super) fn progress(status: &Status) -> Option<ProgressView> {
    let progress = status.state.starting()?;
    let what = match &status.trial {
        Some(TrialState::Running { spec, .. }) => format!("Trial of {spec}"),
        _ => "Starting".to_owned(),
    };
    Some(ProgressView {
        text: format!("{what}: {}", progress.describe()),
        step: progress.step(),
        steps: progress.steps(),
        done: progress.step() - 1,
        time: progress.time_line(),
    })
}

/// What the page and the status answer carry beside the downloads.
pub(super) struct Live {
    pub signature: String,
    pub poll_ms: u32,
}

impl Live {
    /// For `status` and the downloads' signature and busy flag.
    pub(super) fn of(status: &Status, downloads: &str, downloads_busy: bool) -> Self {
        Self {
            signature: signature(status, downloads),
            poll_ms: poll_ms(busy(status, downloads_busy)),
        }
    }
}

async fn status_json(
    State(state): State<AuthState>,
    session: tower_sessions::Session,
) -> Result<Response, StatusCode> {
    owner_auth(&state, &session).await?;
    let Some(runtime) = super::assistant::runtime(&state) else {
        return Ok(Json(json!({"available": false})).into_response());
    };
    let status = runtime.status();
    let downloads = assistant_downloads::live(&state).await;
    let (signature, downloads_busy) = downloads
        .as_ref()
        .map_or((String::new(), false), |d| (d.signature.clone(), d.busy));
    let live = Live::of(&status, &signature, downloads_busy);
    let progress = progress(&status)
        .map(|p| json!({"text": p.text, "step": p.step, "steps": p.steps, "done": p.done, "time": p.time}));
    let mut body = serde_json::Map::new();
    body.insert("available".to_owned(), json!(true));
    body.insert("signature".to_owned(), json!(live.signature));
    body.insert("poll_ms".to_owned(), json!(live.poll_ms));
    body.insert("progress".to_owned(), json!(progress));
    if let Some(downloads) = downloads {
        body.extend(downloads.fields);
    }
    Ok(Json(serde_json::Value::Object(body)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::runtime::manager::tests::{FakeLauncher, manager, ready};

    #[tokio::test(start_paused = true)]
    async fn a_download_alone_polls_fast_and_has_no_progress_block() {
        let launcher = FakeLauncher::scripted([]);
        let (assistant, handle) = manager(&launcher);
        ready(&assistant).await;
        let status = handle.status();
        assert!(!busy(&status, false));
        assert_eq!(Live::of(&status, "", false).poll_ms, IDLE_POLL_MS);
        assert!(busy(&status, true));
        assert_eq!(Live::of(&status, "", true).poll_ms, BUSY_POLL_MS);
        assert!(progress(&status).is_none());
        // The same state with and without the download differs only in the downloads' part.
        assert_ne!(
            Live::of(&status, "idle", false).signature,
            Live::of(&status, "running", true).signature
        );
    }
}
