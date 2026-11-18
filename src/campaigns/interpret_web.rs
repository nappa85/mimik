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
        self, Answer, Assistant, Edit, JobId, Kind, Origin, Poll, Proposal, Refusal, RuntimeState,
    },
    assistant::proposal::MAX_CHANGES,
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

pub(super) fn reply(status: StatusCode, body: &Value) -> Response {
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

pub(super) fn refusal_json(status: StatusCode, kind: &str, message: &str) -> Response {
    reply(status, &json!({"status": kind, "message": message}))
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
        kind: Kind::Interpretation,
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
        Poll::Done(answer) => {
            let shows_options = matches!(answer, Answer::Options(_) | Answer::Reminder { .. });
            let pin = if shows_options {
                // The options name their targets and enum choices by label: the pin's
                // definition, read like the apply does.
                let Some(permit) =
                    crate::definitions::admit_within(super::unit_web::PERMIT_WAIT).await
                else {
                    return Err(StatusCode::SERVICE_UNAVAILABLE);
                };
                let (view, _, permit) = state
                    .store()
                    .private_read_base(auth, id, None, permit)
                    .await
                    .map_err(error)?;
                drop(permit);
                view.immutable
            } else {
                None
            };
            reply(
                StatusCode::OK,
                &done(
                    id,
                    job,
                    &answer,
                    pin.as_ref()
                        .map(crate::definitions::private_pin::ImmutablePin::definition),
                ),
            )
        }
    })
}

fn ask_again() -> Response {
    reply(
        StatusCode::NOT_FOUND,
        &json!({"state": "ask_again", "message": "This proposal is no longer available: ask again."}),
    )
}

fn done(
    sheet: i64,
    job: JobId,
    answer: &Answer,
    definition: Option<&crate::definitions::Definition>,
) -> Value {
    let message = |state: &str, message: &str| json!({"state": state, "message": message});
    match answer {
        Answer::Options(options) => json!({
            "state": "options",
            "message": "The assistant's reading:",
            "options": options.iter().enumerate().map(|(n, option)| option_json(definition, sheet, job, n, option)).collect::<Vec<_>>(),
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
        // A reminder: the hint, the cited row's name and the option(s), as the module shows them
        // below the decreased field; the module treats every other reminder state as nothing.
        Answer::Reminder { hint, row, options } => json!({
            "state": "reminder",
            "message": "Reminder",
            "hint": hint,
            "row": row,
            "options": options.iter().enumerate().map(|(n, option)| option_json(definition, sheet, job, n, option)).collect::<Vec<_>>(),
        }),
    }
}

/// The label of the target at `address` and the labels of its enum choices (none for other
/// controls); an address the pin does not know shows as itself.
fn label_of<'a>(
    definition: Option<&'a crate::definitions::Definition>,
    address: &'a str,
) -> (&'a str, &'a [crate::definitions::Choice]) {
    let found = definition.and_then(|definition| match target(definition, address)? {
        interpret::Target::Field(field) => Some((field.label.as_str(), field.options.as_deref())),
        interpret::Target::Cell { property, .. } => {
            Some((property.label.as_str(), property.options.as_deref()))
        }
    });
    found.map_or((address, &[][..]), |(label, options)| {
        (label, options.unwrap_or(&[]))
    })
}

fn option_json(
    definition: Option<&crate::definitions::Definition>,
    sheet: i64,
    job: JobId,
    n: usize,
    option: &Proposal,
) -> Value {
    // An enum value shows as its choice's label, an unknown number as `?`.
    let value = |choices: &[crate::definitions::Choice], value: &NoteValue| match value {
        NoteValue::Unknown => "?".to_owned(),
        NoteValue::Enum(id) => choices
            .iter()
            .find(|choice| &choice.id == id)
            .map_or_else(|| id.clone(), |choice| choice.label.clone()),
        other => other.text(),
    };
    let changes: Vec<Value> = option
        .changes
        .iter()
        .map(|change| {
            let (label, choices) = label_of(definition, &change.address);
            let before = value(choices, &change.before);
            let after = value(choices, &change.after());
            let shown = match &change.edit {
                Edit::Enum(_) => after.clone(),
                _ => change.shown(),
            };
            json!({
                "target": label,
                "before": before,
                "expression": shown,
                "after": after,
            })
        })
        .collect();
    let first = &changes[0];
    json!({
        "n": n.to_string(),
        "changes": changes,
        "current": first["before"],
        "expression": first["expression"],
        "value": first["after"],
        "effect": format!("{} \u{2192} {}", first["before"].as_str().unwrap_or(""), first["after"].as_str().unwrap_or("")),
        "explanation": option.explanation,
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
    let Some(units) = planned else {
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
                sheet_json::answer_units(
                    view,
                    permit,
                    units,
                    (
                        sheet_json::Outcome::Saved,
                        "The assistant's reading was used.".into(),
                    ),
                    StatusCode::OK,
                    assistant.reminders_effective(),
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

/// The apply's checks on the live sheet (plan §1.4, "Apply", and §1.5, "Bundles"): the origin
/// note as its kind requires (an interpretation's current `unresolved` note; a reminder's
/// decrease note, current on its field, the field still holding its `after`), then for every
/// change its target still on the sheet and holding its bound value, every reference holding
/// its bound value in a row that still exists, the expression giving the shown value again,
/// an enum option the pin declares. Writes every value into `view` with one note per change
/// (`confirmed` with the phrase, or `reminded` with the decrease note's phrase), all in one
/// new bundle, bound to the origin note. Returns the unit of each change, in change order,
/// for the JSON answer.
pub(in crate::campaigns) fn plan(
    view: &mut SheetView,
    origin: &Origin,
    proposal: &Proposal,
) -> Option<Vec<(Unit, Option<String>)>> {
    let pin = view.immutable.as_ref()?;
    let definition = pin.definition();
    let data = &view.snapshot.data;
    let stored = view
        .notes
        .current
        .get(&origin.target)
        .filter(|stored| stored.revision == origin.revision)?
        .clone();
    let (op, held) = match origin.kind {
        Kind::Interpretation => (Op::Confirmed, stored.note.op == Op::Unresolved),
        Kind::Reminder => {
            let here = assist::locate(definition, data, &origin.target)?;
            let holds = assist::value_at(&here, data).as_ref() == Some(&stored.note.after);
            (Op::Reminded, holds)
        }
    };
    let changes = &proposal.changes;
    let targets: std::collections::BTreeSet<&str> = changes
        .iter()
        .map(|change| change.address.as_str())
        .collect();
    // An interpretation is exactly one integer expression on its own target (plan §1.5).
    let one_expression = origin.kind != Kind::Interpretation
        || matches!(
            changes.as_slice(),
            [change] if change.address == origin.target && matches!(change.edit, Edit::Expr { .. })
        );
    if !held
        || !one_expression
        || changes.is_empty()
        || changes.len() > MAX_CHANGES
        || targets.len() != changes.len()
    {
        return None;
    }
    if !reads_hold(definition, data, &proposal.reads) {
        return None;
    }
    let mut writes = Vec::new();
    let mut notes = Vec::new();
    let mut units = Vec::new();
    for change in changes {
        let located = assist::locate(definition, data, &change.address)?;
        if assist::value_at(&located, data)? != change.before {
            return None;
        }
        let after = change.after();
        if after == change.before || !fits(definition, &change.address, &change.edit) {
            return None;
        }
        if let Edit::Expr { candidate, .. } = &change.edit {
            let mut now = std::collections::BTreeMap::new();
            for (id, supplied) in &candidate.refs {
                let at = assist::locate(definition, data, &supplied.address)?;
                let NoteValue::Int(value) = assist::value_at(&at, data)? else {
                    return None;
                };
                if Some(value) != supplied.value {
                    return None;
                }
                now.insert(id.clone(), value);
            }
            let value =
                crate::assistant::output::recompute(&candidate.expr, &|id| now.get(id).copied())?;
            if value != candidate.value {
                return None;
            }
        }
        units.push(unit(definition, data, &change.address)?);
        notes.push(Note {
            address: change.address.clone(),
            op,
            raw: stored.note.raw.clone(),
            before: Some(change.before.clone()),
            after: after.clone(),
            // A reminder's notes name the decrease note they are bound to, which is how a
            // later reminder request sees this one was applied.
            target: (op == Op::Reminded).then_some(origin.revision),
        });
        writes.push((located, after));
    }
    // Every check passed: only now is the data written (the `located` borrows end here).
    let mut written = view.snapshot.data.clone();
    for (located, after) in &writes {
        assist::set(located, &mut written, after).ok()?;
    }
    view.snapshot.data = written;
    view.notes.pending = notes;
    view.notes.bound = vec![stored];
    view.notes.bundle = true;
    Some(units)
}

/// Whether every value a reminder sent still reads the same (plan 3c-1, "Read set and hash"):
/// a field or cell by its typed address and encoded value, a catalogue row by its `row:` address
/// (list and host ID) and its name. A row renamed or removed, or a sent field changed, fails.
pub(in crate::campaigns) fn reads_hold(
    definition: &crate::definitions::Definition,
    data: &Value,
    reads: &[crate::assistant::proposal::Read],
) -> bool {
    reads.iter().all(|read| {
        if let Some(path) = read.address.strip_prefix("row:") {
            let Some((list, host)) = path.split_once('/') else {
                return false;
            };
            let Some(list) = definition.fields.iter().find(|field| field.id == list) else {
                return false;
            };
            let Some(name) = values::properties(list)
                .iter()
                .find(|property| property.catalogue.is_some())
            else {
                return false;
            };
            return data[&list.id].as_array().is_some_and(|rows| {
                rows.iter().any(|row| {
                    row["id"].as_str() == Some(host)
                        && row[&name.id].as_str() == Some(read.value.as_str())
                })
            });
        }
        // A top-level key the data lacks was sent as unknown, so it reads as unknown now.
        assist::locate(definition, data, &read.address)
            .and_then(|located| assist::value_or_unknown_at(&located, data))
            .and_then(|value| value.encode().ok())
            .is_some_and(|encoded| encoded == read.value)
    })
}

/// Whether the edit suits the control of the target at `address`: an expression an integer, an
/// option ID an enum (one the pin declares for that field or property), a boolean a boolean.
/// `bind` cannot check it for an unknown or unset value before, which fits any kind.
fn fits(definition: &crate::definitions::Definition, address: &str, edit: &Edit) -> bool {
    use crate::definitions::Control;
    let (control, options) = if let Some(field) = address.strip_prefix("field:") {
        let Some(field) = definition
            .fields
            .iter()
            .find(|candidate| candidate.id == field)
        else {
            return false;
        };
        (field.control, field.options.as_deref())
    } else {
        let property = address.strip_prefix("cell:").and_then(|path| {
            let mut parts = path.splitn(3, '/');
            let (list, _, property) = (parts.next()?, parts.next()?, parts.next()?);
            let list = definition.fields.iter().find(|field| field.id == list)?;
            values::properties(list)
                .iter()
                .find(|item| item.id == property)
        });
        let Some(property) = property else {
            return false;
        };
        (property.control, property.options.as_deref())
    };
    match edit {
        Edit::Expr { .. } => control == Control::Integer,
        Edit::Bool(_) => control == Control::Boolean,
        Edit::Enum(id) => {
            control == Control::Enumeration
                && options.is_some_and(|options| options.iter().any(|option| &option.id == id))
        }
    }
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
