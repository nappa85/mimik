//! The interpretation routes (assisted sheet plan §1.4, "Routes", "Apply"; step 2c-1),
//! present only with the assistant on:
//!
//! - `POST /private-sheets/{id}/interpretations` with exactly `csrf` and `note` (the key of
//!   the target's current `unresolved` note) starts a job: 202 with its ID, or the same job
//!   for a repeat;
//! - `GET /private-sheets/{id}/interpretations/{job}` polls it;
//! - `POST /private-sheets/{id}/proposals/{job}/{n}` with exactly `csrf` applies option `n`.
//!
//! Every answer is JSON for the sheet module (D-G). Order as for the other sheet routes:
//! session and CSRF, rights (403), keys and malformed values (400), then the note checks
//! (409) and the assistant's own admission.
use super::{AppError, AssistantAnswer, AuthState, SheetPage, StatusCode, error, sheet_json};
use crate::{
    assistant::jobs::{
        self, Answer, Assistant, JobId, Origin, Poll, Proposal, Refusal, RuntimeState,
    },
    auth,
    campaigns::{
        assist, interpret,
        notes::{Note, NoteValue, Op},
        sheets::SheetView,
        transport::Pairs,
        unit::Unit,
        values,
    },
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Redirect, Response},
};
use serde_json::{Value, json};
use std::sync::Arc;
use tower_sessions::Session;

/// Body limit of a start or an apply: two short pairs.
pub(super) const INTERPRET_POST_BYTES: usize = 8192;

/// Every apply refusal (D-P): nothing written, never a literal fallback.
const CHANGED: &str = "Something this proposal used has changed: ask again.";

fn reply(status: StatusCode, body: &Value) -> Response {
    let mut response = (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response();
    response.extensions_mut().insert(SheetPage);
    // A refusal's JSON is the answer itself: the error page must not replace it.
    response.extensions_mut().insert(AssistantAnswer);
    response
}

fn assistant(state: &AuthState) -> Result<Arc<Assistant>, StatusCode> {
    state.assistant().ok_or(StatusCode::NOT_FOUND)
}

fn runtime_message(state: &RuntimeState) -> String {
    match state {
        RuntimeState::Starting => "The assistant is starting; try again shortly.".into(),
        RuntimeState::Ready => "The assistant is unavailable right now.".into(),
        RuntimeState::Unavailable(reason) => format!("The assistant is unavailable: {reason}"),
    }
}

fn refusal(refused: &Refusal) -> Response {
    let (status, kind, message) = match refused {
        Refusal::Conflict => (
            StatusCode::CONFLICT,
            "conflict",
            "This entry changed since it was saved; nothing to interpret.".to_owned(),
        ),
        Refusal::Busy => (
            StatusCode::TOO_MANY_REQUESTS,
            "busy",
            "The assistant is busy, try again shortly.".to_owned(),
        ),
        Refusal::Runtime(state) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "runtime",
            runtime_message(state),
        ),
        Refusal::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "The assistant is unavailable right now.".to_owned(),
        ),
    };
    reply(status, &json!({"status": kind, "message": message}))
}

/// Starts a job for the note `note` names.
pub(super) async fn start(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    body: axum::body::Bytes,
) -> super::ResultResponse {
    let assistant = assistant(&state)?;
    let pairs = Pairs::parse(&body)?;
    auth::check_csrf(&session, pairs.csrf()?).await?;
    let auth = auth::request_auth(&state, &session).await?;
    let note = pairs.value("note").map(str::to_owned);
    let key = note.as_deref().and_then(assist::parse_key);
    // The origin revision is registered before the database check (plan §1.4, "Routes").
    let registration = key.map(|(revision, _)| assistant.register(id, revision));
    let earlier = key.and_then(|(revision, named)| {
        matches!(named, assist::Named::Indexed { .. }).then_some(revision)
    });
    let permit = crate::definitions::admit().map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let (view, at_key, permit) = state
        .store()
        .private_read_base(auth.clone(), id, earlier, permit)
        .await
        .map_err(error)?;
    drop(permit);
    if !view.editable {
        return Err(StatusCode::FORBIDDEN);
    }
    pairs.admit(
        &["csrf", "note"]
            .into_iter()
            .collect::<std::collections::HashSet<_>>(),
    )?;
    let ((revision, named), registration) = key.zip(registration).ok_or(StatusCode::BAD_REQUEST)?;
    let pin = view.immutable.as_ref().ok_or(StatusCode::BAD_REQUEST)?;
    let definition = pin.definition();
    let address = match named {
        assist::Named::Address(address)
            if (address.starts_with("field:") || address.starts_with("cell:"))
                && assist::in_pin(definition, address) =>
        {
            address.to_owned()
        }
        assist::Named::Indexed { f, r, p } => {
            if revision > view.snapshot.revision {
                return Ok(refusal(&Refusal::Conflict));
            }
            let at = at_key.as_ref().ok_or(StatusCode::BAD_REQUEST)?;
            assist::indexed_address(definition, &at.data, (f, r, p))
                .ok_or(StatusCode::BAD_REQUEST)?
        }
        assist::Named::Address(_) => return Err(StatusCode::BAD_REQUEST),
    };
    // The target's current note must be that revision's `unresolved` note: a parsed or
    // sign-led entry, a newer note or a removed row is a conflict.
    let current = view
        .notes
        .current
        .get(&address)
        .filter(|stored| stored.revision == revision && stored.note.op == Op::Unresolved);
    let Some(stored) = current else {
        return Ok(refusal(&Refusal::Conflict));
    };
    let Some(located) = assist::locate(definition, &view.snapshot.data, &address) else {
        return Ok(refusal(&Refusal::Conflict));
    };
    let Some(budgets) = assistant.budgets() else {
        return Ok(refusal(&Refusal::Runtime(assistant.state())));
    };
    let Some(target) = target(definition, &address) else {
        return Err(StatusCode::BAD_REQUEST);
    };
    let draft = interpret::draft(
        definition,
        pin.formulas(),
        &view.snapshot.data,
        (target, &address, &located.label),
        &stored.note.raw,
        budgets.bounds.bytes,
    );
    let origin = Origin {
        sheet: id,
        target: address,
        revision,
    };
    match assistant.start(&registration, auth.account_id(), origin, draft) {
        Ok((job, repeat)) => Ok(reply(
            if repeat {
                StatusCode::OK
            } else {
                StatusCode::ACCEPTED
            },
            &json!({
                "status": "started",
                "job": job.text(),
                "poll": format!("/private-sheets/{id}/interpretations/{}", job.text()),
            }),
        )),
        Err(refused) => Ok(refusal(&refused)),
    }
}

/// The target an address names (a scalar, or a row cell by host ID).
fn target<'a>(
    definition: &'a crate::definitions::Definition,
    address: &'a str,
) -> Option<interpret::Target<'a>> {
    if let Some(id) = address.strip_prefix("field:") {
        return definition
            .fields
            .iter()
            .find(|field| field.id == id)
            .map(interpret::Target::Field);
    }
    let path = address.strip_prefix("cell:")?;
    let mut parts = path.splitn(3, '/');
    let (list, host, property) = (parts.next()?, parts.next()?, parts.next()?);
    let list = definition.fields.iter().find(|field| field.id == list)?;
    let property = values::properties(list)
        .iter()
        .find(|item| item.id == property)?;
    Some(interpret::Target::Cell {
        list,
        host,
        property,
    })
}

/// Polls a job.
pub(super) async fn poll(
    State(state): State<AuthState>,
    session: Session,
    Path((id, job)): Path<(i64, String)>,
) -> super::ResultResponse {
    let assistant = assistant(&state)?;
    let auth = auth::request_auth(&state, &session).await?;
    if !state
        .store()
        .private_editable(auth.clone(), id)
        .await
        .map_err(error)?
    {
        return Err(StatusCode::FORBIDDEN);
    }
    let Some(job) = JobId::parse(&job) else {
        return Ok(ask_again());
    };
    Ok(match assistant.poll(auth.account_id(), id, job) {
        Poll::Missing => ask_again(),
        Poll::Queued | Poll::Running => {
            let state = assistant.state();
            if state == RuntimeState::Ready {
                reply(
                    StatusCode::OK,
                    &json!({"state": "thinking", "message": "Thinking…"}),
                )
            } else {
                reply(
                    StatusCode::OK,
                    &json!({"state": "unavailable", "message": runtime_message(&state)}),
                )
            }
        }
        Poll::Done(answer) => reply(StatusCode::OK, &done(id, job, &answer)),
    })
}

fn ask_again() -> Response {
    reply(
        StatusCode::NOT_FOUND,
        &json!({"state": "ask_again", "message": "This proposal is no longer available: ask again."}),
    )
}

fn done(sheet: i64, job: JobId, answer: &Answer) -> Value {
    let message = |state: &str, message: &str| json!({"state": state, "message": message});
    match answer {
        Answer::Options(options) => json!({
            "state": "options",
            "message": "The assistant's reading:",
            "options": options.iter().enumerate().map(|(n, option)| option_json(sheet, job, n, option)).collect::<Vec<_>>(),
        }),
        Answer::Readings(readings) => json!({
            "state": "readings",
            "message": "The assistant is not sure what this means. Possible readings:",
            "readings": readings,
        }),
        Answer::Nothing => message("nothing", "The assistant has no suggestion for this entry."),
        Answer::TooLong => message(
            "too_long",
            "This entry is too long for the assistant on this machine.",
        ),
        Answer::Unavailable => message("unavailable", "The assistant is unavailable right now."),
        Answer::Restarted => message("ask_again", "The assistant restarted: ask again."),
        Answer::AskAgain => message("ask_again", "The assistant took too long: ask again."),
        Answer::Superseded => message("superseded", "A newer entry replaced this one."),
        Answer::Stale => message("stale", "This entry changed since: ask again."),
    }
}

fn option_json(sheet: i64, job: JobId, n: usize, option: &Proposal) -> Value {
    let before = option
        .before
        .map_or_else(|| "?".to_owned(), |value| value.to_string());
    json!({
        "n": n.to_string(),
        "current": before,
        "expression": option.shown,
        "value": option.candidate.value.to_string(),
        "effect": format!("{before} \u{2192} {}", option.candidate.value),
        "explanation": option.candidate.explanation,
        "action": format!("/private-sheets/{sheet}/proposals/{}/{n}", job.text()),
    })
}

/// Applies option `n` of `job`.
pub(super) async fn apply(
    State(state): State<AuthState>,
    session: Session,
    Path((id, job, n)): Path<(i64, String, String)>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> super::ResultResponse {
    let assistant = assistant(&state)?;
    let json = sheet_json::wanted(&headers);
    let pairs = Pairs::parse(&body)?;
    auth::check_csrf(&session, pairs.csrf()?).await?;
    let auth = auth::request_auth(&state, &session).await?;
    let csrf = auth::token(&session)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let Some(permit) = crate::definitions::admit_within(super::unit_web::PERMIT_WAIT).await else {
        return super::unit_web::busy_retry(
            id,
            &format!("/private-sheets/{id}/proposals/{job}/{n}"),
            &csrf,
            "Use this proposal again",
        );
    };
    let (view, _, permit) = state
        .store()
        .private_read_base(auth.clone(), id, None, permit)
        .await
        .map_err(error)?;
    if !view.editable {
        return Err(StatusCode::FORBIDDEN);
    }
    pairs.admit(
        &["csrf"]
            .into_iter()
            .collect::<std::collections::HashSet<_>>(),
    )?;
    let (Some(job), Some(n)) = (JobId::parse(&job), values::index(&n)) else {
        return Err(StatusCode::BAD_REQUEST);
    };
    let Some((origin, proposal)) = assistant.proposal(auth.account_id(), id, job, n) else {
        return Ok(changed());
    };
    if !jobs::verify(job, n, &origin, &proposal) {
        return Ok(changed());
    }
    let (planned, view, permit) = tokio::task::spawn_blocking(move || {
        let mut view = view;
        let planned = plan(&mut view, &origin, &proposal);
        (planned, view, permit)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let Some((unit, host)) = planned else {
        return Ok(changed());
    };
    match state
        .store()
        .private_save_keeping(auth.clone(), view, permit)
        .await
    {
        Ok((id, permit)) => {
            assistant.consume(job);
            if json {
                let (view, permit) = state
                    .store()
                    .private_read(auth, id, None, permit)
                    .await
                    .map_err(error)?;
                sheet_json::answer(
                    view,
                    permit,
                    (unit, host),
                    (
                        sheet_json::Outcome::Saved,
                        "The assistant's reading was used.".into(),
                    ),
                    StatusCode::OK,
                )
                .await
            } else {
                Ok(Redirect::to(&format!("/private-sheets/{id}")).into_response())
            }
        }
        Err((AppError::Conflict | AppError::Invalid(_), _)) => Ok(changed()),
        Err((failure, _)) => Err(error(failure)),
    }
}

fn changed() -> Response {
    reply(
        StatusCode::CONFLICT,
        &json!({"status": "conflict", "message": CHANGED}),
    )
}

/// The apply's checks on the live sheet (plan §1.4, "Apply"): the origin note current, the
/// target holding its bound value, every reference holding its bound value in a row that
/// still exists, and the expression giving the shown value again. Writes the value into
/// `view` with a `confirmed` note in a new bundle, bound to the origin note. Returns the
/// unit for the JSON answer.
fn plan(
    view: &mut SheetView,
    origin: &Origin,
    proposal: &Proposal,
) -> Option<(Unit, Option<String>)> {
    let pin = view.immutable.as_ref()?;
    let definition = pin.definition();
    let data = &view.snapshot.data;
    let stored = view
        .notes
        .current
        .get(&origin.target)
        .filter(|stored| stored.revision == origin.revision && stored.note.op == Op::Unresolved)?
        .clone();
    let located = assist::locate(definition, data, &origin.target)?;
    let before = proposal.before.map_or(NoteValue::Unknown, NoteValue::Int);
    if assist::value_at(&located, data)? != before {
        return None;
    }
    let mut now = std::collections::BTreeMap::new();
    for (alias, supplied) in &proposal.candidate.refs {
        let at = assist::locate(definition, data, &supplied.address)?;
        let NoteValue::Int(value) = assist::value_at(&at, data)? else {
            return None;
        };
        if Some(value) != supplied.value {
            return None;
        }
        now.insert(alias.clone(), value);
    }
    let value = crate::assistant::output::recompute(&proposal.candidate.expr, &|alias| {
        now.get(alias).copied()
    })?;
    if value != proposal.candidate.value {
        return None;
    }
    let after = NoteValue::Int(value);
    let unit = unit(definition, data, &origin.target)?;
    assist::set(&located, &mut view.snapshot.data, &after).ok()?;
    view.notes.pending = vec![Note {
        address: origin.target.clone(),
        op: Op::Confirmed,
        raw: stored.note.raw.clone(),
        before: Some(before),
        after,
        target: None,
    }];
    view.notes.bound = vec![stored];
    view.notes.bundle = true;
    Some(unit)
}

/// The unit of the JSON answer for a target address: a field by index, a row cell by its
/// indices in `data` and its host ID.
fn unit(
    definition: &crate::definitions::Definition,
    data: &Value,
    address: &str,
) -> Option<(Unit, Option<String>)> {
    if let Some(id) = address.strip_prefix("field:") {
        let f = definition.fields.iter().position(|field| field.id == id)?;
        return Some((Unit::Field(f), None));
    }
    let path = address.strip_prefix("cell:")?;
    let mut parts = path.splitn(3, '/');
    let (list, host, property) = (parts.next()?, parts.next()?, parts.next()?);
    let f = definition
        .fields
        .iter()
        .position(|field| field.id == list)?;
    let p = values::properties(&definition.fields[f])
        .iter()
        .position(|item| item.id == property)?;
    let r = data[list]
        .as_array()?
        .iter()
        .position(|item| item["id"].as_str() == Some(host))?;
    Some((Unit::Cell { f, r, p }, Some(host.to_owned())))
}
