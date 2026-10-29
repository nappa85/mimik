//! The Downloads panel and routes of `/admin/assistant` (`docs/model-downloads.md`):
//! Review, Confirm, Cancel, Resume, Discard, delete, and the state for polling. Owner only;
//! every action is CSRF-checked, a strict form of at most 4 KiB, and takes the model-work gate
//! where it changes files.
use super::{AuthState, csrf_admission, owner_auth};
use crate::assistant::download::review::{Snapshot, State as ReviewState};
use crate::assistant::download::rows::{New, Row, State as RowState};
use crate::assistant::download::task::{Discarded, Permit, Phase, Refusal, Status, Stop};
use crate::assistant::downloads::Downloads;
use crate::assistant::model::{HfSpec, pinned};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Form, State},
    http::{StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tower_sessions::Session;

const FORM_BYTES: usize = 4096;

pub(super) fn routes() -> Router<AuthState> {
    let limit = || DefaultBodyLimit::max(FORM_BYTES);
    Router::new()
        .route("/admin/assistant/model/review", post(review).layer(limit()))
        .route(
            "/admin/assistant/download/confirm",
            post(confirm).layer(limit()),
        )
        .route(
            "/admin/assistant/download/cancel",
            post(cancel).layer(limit()),
        )
        .route(
            "/admin/assistant/download/resume",
            post(resume).layer(limit()),
        )
        .route(
            "/admin/assistant/download/repair",
            post(repair).layer(limit()),
        )
        .route(
            "/admin/assistant/download/discard",
            post(discard).layer(limit()),
        )
        .route("/admin/assistant/model/delete", post(delete).layer(limit()))
        .route("/admin/assistant/downloads.json", get(state_json))
        .route(
            "/assets/assistant-downloads.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../assets/assistant-downloads.js"),
                )
            }),
        )
}

/// The notice for a command the download task refused.
fn refusal_notice(why: &Refusal) -> &'static str {
    match why {
        Refusal::Busy | Refusal::Finishing => "model-busy",
        Refusal::OtherPartial => "partial-exists",
        Refusal::NoSuchWaiting | Refusal::NoSuchFinished => "no-such-download",
        Refusal::NothingToRepair => "nothing-to-repair",
        Refusal::Records(_) | Refusal::Disk(_) => "download-failed",
        Refusal::Ended => "downloads-unavailable",
    }
}

fn redirect(notice: &str) -> Response {
    Redirect::to(&format!("/admin/assistant?notice={notice}")).into_response()
}

fn downloads(state: &AuthState) -> Option<Arc<Downloads>> {
    state
        .options
        .assistant
        .as_ref()
        .and_then(|handle| handle.downloads.clone())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewForm {
    csrf: String,
    model: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfirmForm {
    csrf: String,
    token: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdForm {
    csrf: String,
    id: i64,
}

/// Starts the review of `spec` (from the Model field's Try or the pinned panel): at once,
/// the work runs in the background and the page polls.
pub(super) fn start_review(state: &AuthState, spec: HfSpec) -> Response {
    let Some(downloads) = downloads(state) else {
        return redirect("downloads-unavailable");
    };
    match downloads.reviewer.start(spec) {
        Ok(()) => redirect("reviewing"),
        Err(_) => redirect("review-busy"),
    }
}

async fn review(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<ReviewForm>,
) -> Result<Response, StatusCode> {
    owner_auth(&state, &session).await?;
    csrf_admission(&session, &form.csrf).await?;
    match crate::assistant::model::parse_spec(&form.model) {
        Ok(crate::assistant::model::Spec::Hf(spec)) => Ok(start_review(&state, spec)),
        _ => Ok(redirect("not-a-download")),
    }
}

/// The file name a snapshot's download is stored under: the pinned `local:` name for a pinned
/// default, else `hf-<sha256>.gguf`.
fn file_name(snapshot: &Snapshot) -> String {
    snapshot.pinned.as_ref().map_or_else(
        || format!("hf-{}.gguf", snapshot.facts.sha256),
        |pin| pin.file_name().to_owned(),
    )
}

async fn confirm(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<ConfirmForm>,
) -> Result<Response, StatusCode> {
    let auth = owner_auth(&state, &session).await?;
    csrf_admission(&session, &form.csrf).await?;
    let (Some(downloads), Some(runtime)) = (downloads(&state), super::assistant::runtime(&state))
    else {
        return Ok(redirect("downloads-unavailable"));
    };
    let Some(snapshot) = downloads.reviewer.snapshot(&form.token) else {
        return Ok(redirect("review-stale"));
    };
    let Ok(gate) = runtime.model_work() else {
        return Ok(redirect("model-busy"));
    };
    let Ok(rows) = downloads.rows.list().await else {
        return Ok(redirect("download-failed"));
    };
    let spec = snapshot.spec.canonical();
    if rows.iter().any(|row| row.spec == spec) {
        return Ok(redirect("already-confirmed"));
    }
    // One partial at all; a download of the same bytes already waiting is that partial.
    let partials = downloads
        .downloader
        .partials()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let same_bytes_waiting = rows
        .iter()
        .any(|row| row.state == RowState::Confirmed && row.sha256 == snapshot.facts.sha256);
    if !partials.is_empty() || same_bytes_waiting {
        return Ok(redirect("partial-exists"));
    }
    let new = New {
        spec,
        file_name: file_name(&snapshot),
        sha256: snapshot.facts.sha256.clone(),
        size: snapshot.facts.size,
        licence_id: snapshot.licence.id.clone(),
        licence_path: snapshot.licence.path.clone(),
        licence_sha256: snapshot.licence.sha256.clone(),
        confirmed_by: auth.account.id,
    };
    let Ok(row) = downloads.rows.insert(new).await else {
        return Ok(redirect("download-failed"));
    };
    downloads.reviewer.clear();
    let id = row.id;
    if let Err(why) = downloads.downloader.start(row, Box::new(gate)).await {
        // The task refused: nothing was started, so nothing stays recorded.
        let _ = downloads.rows.delete(id).await;
        return Ok(redirect(refusal_notice(&why)));
    }
    Ok(redirect("download-started"))
}

async fn cancel(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<IdForm>,
) -> Result<Response, StatusCode> {
    owner_auth(&state, &session).await?;
    csrf_admission(&session, &form.csrf).await?;
    let Some(downloads) = downloads(&state) else {
        return Ok(redirect("downloads-unavailable"));
    };
    // The id the page showed: a stale page cannot cancel a newer transfer.
    if !downloads.downloader.cancel(Some(form.id)).await {
        return Ok(redirect("stale-download"));
    }
    Ok(redirect("download-cancelled"))
}

async fn resume(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<IdForm>,
) -> Result<Response, StatusCode> {
    owner_auth(&state, &session).await?;
    csrf_admission(&session, &form.csrf).await?;
    let (Some(downloads), Some(runtime)) = (downloads(&state), super::assistant::runtime(&state))
    else {
        return Ok(redirect("downloads-unavailable"));
    };
    let Ok(rows) = downloads.rows.list().await else {
        return Ok(redirect("download-failed"));
    };
    let Some(row) = rows
        .into_iter()
        .find(|row| row.id == form.id && row.state == RowState::Confirmed)
    else {
        return Ok(redirect("no-such-download"));
    };
    let Ok(gate) = runtime.model_work() else {
        return Ok(redirect("model-busy"));
    };
    Ok(redirect(
        match downloads.downloader.resume(row.id, Box::new(gate)).await {
            Ok(()) => "download-resuming",
            Err(why) => refusal_notice(&why),
        },
    ))
}

/// Fetches a finished download again because its file is gone or changed (the row keeps its
/// licence snapshot; a different file under the name is never replaced).
async fn repair(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<IdForm>,
) -> Result<Response, StatusCode> {
    owner_auth(&state, &session).await?;
    csrf_admission(&session, &form.csrf).await?;
    let (Some(downloads), Some(runtime)) = (downloads(&state), super::assistant::runtime(&state))
    else {
        return Ok(redirect("downloads-unavailable"));
    };
    let Ok(gate) = runtime.model_work() else {
        return Ok(redirect("model-busy"));
    };
    Ok(redirect(
        match downloads.downloader.repair(form.id, Box::new(gate)).await {
            Ok(()) => "download-started",
            Err(why) => refusal_notice(&why),
        },
    ))
}

async fn discard(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<IdForm>,
) -> Result<Response, StatusCode> {
    owner_auth(&state, &session).await?;
    csrf_admission(&session, &form.csrf).await?;
    let (Some(downloads), Some(runtime)) = (downloads(&state), super::assistant::runtime(&state))
    else {
        return Ok(redirect("downloads-unavailable"));
    };
    // A discard changes files, so it takes the model-work gate like Delete. The one exception
    // is the running download, whose transfer already holds the gate and which the task
    // discards under that permit.
    let permit: Option<Permit> = match runtime.model_work() {
        Ok(gate) => Some(Box::new(gate)),
        Err(_) if running(&downloads.downloader.status(), form.id) => None,
        Err(_) => return Ok(redirect("model-busy")),
    };
    let mut protect: Vec<String> = pinned()
        .iter()
        .map(|pin| pin.file_name().to_owned())
        .collect();
    protect.extend(runtime.model_files());
    Ok(redirect(
        match downloads.downloader.discard(form.id, protect, permit).await {
            Ok(Discarded::FileKept) => "download-discarded-file-kept",
            Ok(_) => "download-discarded",
            Err(why) => refusal_notice(&why),
        },
    ))
}

async fn delete(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<IdForm>,
) -> Result<Response, StatusCode> {
    owner_auth(&state, &session).await?;
    csrf_admission(&session, &form.csrf).await?;
    let (Some(downloads), Some(runtime)) = (downloads(&state), super::assistant::runtime(&state))
    else {
        return Ok(redirect("downloads-unavailable"));
    };
    let Ok(_gate) = runtime.model_work() else {
        return Ok(redirect("model-busy"));
    };
    let mut protect: Vec<String> = pinned()
        .iter()
        .map(|pin| pin.file_name().to_owned())
        .collect();
    protect.extend(runtime.model_files());
    Ok(redirect(
        match downloads.downloader.delete(form.id, &protect).await {
            Ok(()) => "file-deleted",
            Err(_) => "file-kept",
        },
    ))
}

/// Whether download `id` is the one transferring now (not merely the last one that ran).
fn running(status: &Status, id: i64) -> bool {
    status.id == Some(id) && matches!(status.phase, Phase::Running { .. } | Phase::Verifying)
}

fn phase_word(phase: &Phase) -> &'static str {
    match phase {
        Phase::Idle => "idle",
        Phase::Running { .. } => "running",
        Phase::Verifying => "verifying",
        Phase::Stopped(Stop::Cancelled) => "cancelled",
        Phase::Stopped(Stop::Stalled) => "stalled",
        Phase::Stopped(Stop::Failed(_)) => "failed",
        Phase::Done => "done",
    }
}

fn review_word(state: &ReviewState) -> &'static str {
    match state {
        ReviewState::Idle => "idle",
        ReviewState::Running { .. } => "running",
        ReviewState::Ready(_) => "ready",
        ReviewState::Failed { .. } => "failed",
    }
}

/// What changes the page (a state, not a byte count): the polling script reloads on a change.
pub(super) fn signature(review: &ReviewState, status: &Status) -> String {
    format!("{}|{}", review_word(review), phase_word(&status.phase))
}

async fn state_json(
    State(state): State<AuthState>,
    session: Session,
) -> Result<Response, StatusCode> {
    owner_auth(&state, &session).await?;
    let Some(downloads) = downloads(&state) else {
        return Ok(Json(json!({"available": false})).into_response());
    };
    let review = downloads.reviewer.state();
    let status = downloads.downloader.status();
    let (received, total) = match &status.phase {
        Phase::Running { received, total } => (*received, *total),
        _ => (0, 0),
    };
    let Ok(listed) = downloads.rows.list().await else {
        return Ok(Json(json!({
            "available": true,
            "error": "the download records cannot be read",
        }))
        .into_response());
    };
    let rows: Vec<serde_json::Value> = listed
        .iter()
        .map(|row: &Row| {
            json!({
                "id": row.id,
                "spec": row.spec,
                "file": row.file_name,
                "size": row.size,
                "state": if row.state == RowState::Complete { "complete" } else { "confirmed" },
            })
        })
        .collect();
    Ok(Json(json!({
        "available": true,
        "signature": signature(&review, &status),
        "review": review_word(&review),
        "download": {"id": status.id, "spec": status.spec, "phase": phase_word(&status.phase), "received": received, "total": total},
        "rows": rows,
    }))
    .into_response())
}
