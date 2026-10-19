//! The switch and Undo routes (assisted sheet slice 1.3): `POST /private-sheets/{id}/switch`
//! and `/undo`, each with exactly `csrf`, `base` and `note`.
//!
//! Precedence as for sheet saves: parse, CSRF, request auth and edit rights (403), then key
//! admission and the note key (400). The note must still be the current note of its
//! address with the cell holding its `after`; otherwise nothing is written and the saved
//! sheet is shown again with 409 ("changed since, or already done"). The write goes through
//! the sheet writer, which rechecks rights, the active state, the live revision and the
//! bound note inside its `IMMEDIATE` transaction.
use super::{
    AppError, AuthState, Notice, Redirect, RuntimeAdmission, StatusCode, error, rejected_editor,
    rendered,
};
use crate::{auth, campaigns::assist, campaigns::sheets::SheetView, campaigns::values};
use axum::{
    Extension,
    extract::{Path, State},
    response::IntoResponse,
};
use std::collections::{BTreeMap, HashSet};
use tower_sessions::Session;

/// Body limit of a switch or Undo POST: three short pairs.
pub(super) const ASSIST_POST_BYTES: usize = 8192;

const KEYS: [&str; 3] = ["csrf", "base", "note"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Switch,
    Undo,
}

pub(super) async fn switch(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    Extension(admission): Extension<RuntimeAdmission>,
    body: axum::body::Bytes,
) -> super::ResultResponse {
    act(state, session, id, admission, &body, Action::Switch).await
}

pub(super) async fn undo(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    Extension(admission): Extension<RuntimeAdmission>,
    body: axum::body::Bytes,
) -> super::ResultResponse {
    act(state, session, id, admission, &body, Action::Undo).await
}

/// Why a switch or Undo writes nothing.
enum Refusal {
    /// A malformed or forged request (400).
    BadRequest,
    /// The note is no longer current, or the cell changed since (409, the page re-read).
    Stale(String),
}

async fn act(
    state: AuthState,
    session: Session,
    id: i64,
    admission: RuntimeAdmission,
    body: &[u8],
    action: Action,
) -> super::ResultResponse {
    let permit = admission.take()?;
    let pairs = super::super::transport::Pairs::parse(body)?;
    auth::check_csrf(&session, pairs.csrf()?).await?;
    let auth = auth::request_auth(&state, &session).await?;
    let (mut view, permit) = state
        .store()
        .private_read(auth.clone(), id, None, permit)
        .await
        .map_err(error)?;
    if !view.editable {
        return Err(StatusCode::FORBIDDEN);
    }
    let form = pairs.admit(&KEYS.into_iter().collect::<HashSet<_>>())?;
    let csrf = auth::token(&session)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let (planned, view, permit) = tokio::task::spawn_blocking(move || {
        let planned = plan(&mut view, &form, action);
        (planned, view, permit)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let refused = match planned {
        Ok(message) => message,
        Err(Refusal::BadRequest) => return Err(StatusCode::BAD_REQUEST),
        Err(Refusal::Stale(message)) => {
            return rendered(
                view,
                permit,
                csrf,
                None,
                StatusCode::CONFLICT,
                Notice::status(message),
            )
            .await;
        }
    };
    match state
        .store()
        .private_save_keeping(auth.clone(), view, permit)
        .await
    {
        Ok((id, _permit)) => Ok(Redirect::to(&format!("/private-sheets/{id}")).into_response()),
        Err((failure @ (AppError::Conflict | AppError::Invalid(_)), permit)) => {
            let message = match failure {
                AppError::Invalid(reason) => format!("Nothing changed: {reason}"),
                _ => refused,
            };
            let (view, permit) = rejected_editor(&state, auth, id, None, permit).await?;
            rendered(
                view,
                permit,
                csrf,
                None,
                error(failure),
                Notice::status(message),
            )
            .await
        }
        Err((failure, _)) => Err(error(failure)),
    }
}

/// Applies the switch or Undo to `view` (the live sheet read in this request): its data,
/// the note to write and the note it is bound to. Returns the refusal message used if the
/// writer finds the binding broken inside its transaction.
fn plan(
    view: &mut SheetView,
    form: &BTreeMap<String, String>,
    action: Action,
) -> Result<String, Refusal> {
    // Native and unsupported pins have no notes.
    let pin = view.immutable.as_ref().ok_or(Refusal::BadRequest)?;
    let live = view.snapshot.revision;
    let base = values::integer(&form["base"]).map_err(|_| Refusal::BadRequest)?;
    let (revision, address) = assist::parse_key(&form["note"]).ok_or(Refusal::BadRequest)?;
    // The note was shown on the page read at `base`, so it cannot be newer than `base`.
    if base < 1 || revision > base {
        return Err(Refusal::BadRequest);
    }
    // A cell the pin does not have is forged (400); a row cell whose row has gone since
    // the page was shown is stale (409, with a generic label).
    if !assist::in_pin(pin.definition(), address) {
        return Err(Refusal::BadRequest);
    }
    let located = assist::locate(pin.definition(), &view.snapshot.data, address);
    let label = located
        .as_ref()
        .map_or("The cell", |located| located.label.as_str());
    let stale = match action {
        Action::Switch => {
            format!("Not switched: {label} changed since, or it was already switched.")
        }
        Action::Undo => {
            format!("Nothing undone: {label} changed since, or this change was already undone.")
        }
    };
    // A base above the live revision is forged or from a lost future: a conflict.
    if base > live {
        return Err(Refusal::Stale(stale));
    }
    let located = located.ok_or_else(|| Refusal::Stale(stale.clone()))?;
    let current = assist::value_at(&located, &view.snapshot.data);
    let stored = view
        .notes
        .current
        .get(address)
        .filter(|stored| stored.revision == revision)
        .ok_or_else(|| Refusal::Stale(stale.clone()))?;
    let (note, value) = match action {
        Action::Switch => {
            let value = assist::switch_value(stored, current.as_ref())
                .ok_or_else(|| Refusal::Stale(stale.clone()))?;
            (
                assist::switch_note(stored, value),
                super::super::notes::NoteValue::Int(value),
            )
        }
        Action::Undo => {
            if !assist::undoable(stored, current.as_ref()) {
                return Err(Refusal::Stale(stale));
            }
            let note = assist::undo_note(stored);
            let value = note.after.clone();
            (note, value)
        }
    };
    assist::set(&located, &mut view.snapshot.data, &value)
        .map_err(|_| Refusal::Stale(stale.clone()))?;
    view.notes.bound = vec![stored.clone()];
    view.notes.pending = vec![note];
    Ok(stale)
}
