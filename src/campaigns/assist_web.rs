//! The switch and Undo routes (assisted sheet slice 1.3): `POST /private-sheets/{id}/switch`
//! and `/undo`, each with exactly `csrf`, `base` and `note`.
//!
//! Precedence as for sheet saves: parse, CSRF, request auth and edit rights (403), then key
//! admission and the note key (400). The note must still be the current note of its
//! address with the cell holding its `after`; otherwise nothing is written and the saved
//! sheet is shown again with 409 ("changed since, or already done"). The write goes through
//! the sheet writer, which rechecks rights, the active state, the live revision and the
//! bound note inside its `IMMEDIATE` transaction.
//!
//! Slice 1.5b: a sheet page names a row cell by indices (`<rev>:<f>.<r>.<p>`, read against
//! the page's `base`), and Undo of a row removal (`<rev>:row:<list>/<host>`) re-inserts the
//! row from the revision before the removal; both earlier revisions are read in the same
//! transaction as the live one.
use super::{
    AppError, AuthState, Notice, Redirect, RuntimeAdmission, StatusCode, error, rejected_editor,
    rendered,
};
use crate::{
    auth,
    campaigns::{assist, sheets::SheetView, snapshots::PrivateSnapshot, values},
};
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
    /// The action cannot be done as asked (422, the page re-read): Undo of a removal into a
    /// full list.
    Invalid(&'static str),
}

/// The earlier revision a switch or Undo reads with the live one (slice 1.5b): the page's
/// `base` for an indexed row cell, the revision before a removal for its Undo; `None`
/// otherwise or when the pairs are malformed (admission answers those).
fn earlier(pairs: &super::super::transport::Pairs) -> Option<i64> {
    let (revision, named) = assist::parse_key(pairs.value("note")?)?;
    match named {
        assist::Named::Indexed { .. } => values::integer(pairs.value("base")?).ok(),
        assist::Named::Address(address) if address.starts_with("row:") => revision.checked_sub(1),
        assist::Named::Address(_) => None,
    }
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
    let (mut view, earlier, permit) = state
        .store()
        .private_read_base(auth.clone(), id, earlier(&pairs), permit)
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
        let planned = plan(&mut view, &form, action, earlier.as_ref());
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
        Err(Refusal::Invalid(reason)) => {
            return rendered(
                view,
                permit,
                csrf,
                None,
                StatusCode::UNPROCESSABLE_ENTITY,
                Notice::status(format!("Nothing changed: {reason}")),
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
/// writer finds the binding broken inside its transaction. `earlier` is the revision
/// [`earlier`] asked for, when it exists.
fn plan(
    view: &mut SheetView,
    form: &BTreeMap<String, String>,
    action: Action,
    earlier: Option<&PrivateSnapshot>,
) -> Result<String, Refusal> {
    // Native and unsupported pins have no notes.
    let pin = view.immutable.as_ref().ok_or(Refusal::BadRequest)?;
    let definition = pin.definition();
    let live = view.snapshot.revision;
    let base = values::integer(&form["base"]).map_err(|_| Refusal::BadRequest)?;
    let (revision, named) = assist::parse_key(&form["note"]).ok_or(Refusal::BadRequest)?;
    // The note was shown on the page read at `base`, so it cannot be newer than `base`.
    if base < 1 || revision > base {
        return Err(Refusal::BadRequest);
    }
    // A cell the pin does not have is forged (400); a row cell whose row has gone since
    // the page was shown is stale (409, with a generic label).
    let address = match named {
        assist::Named::Address(address) if assist::in_pin(definition, address) => {
            address.to_owned()
        }
        assist::Named::Indexed { f, r, p } => {
            assist::row_property(definition, (f, p)).ok_or(Refusal::BadRequest)?;
            if base > live {
                return Err(Refusal::Stale(stale("The cell", action)));
            }
            // `base` is within 1..=L here, so the page's revision was read with L.
            let page = earlier.ok_or(Refusal::BadRequest)?;
            assist::indexed_address(definition, &page.data, (f, r, p)).ok_or(Refusal::BadRequest)?
        }
        assist::Named::Address(_) => return Err(Refusal::BadRequest),
    };
    let located = assist::locate(definition, &view.snapshot.data, &address);
    let label = located
        .as_ref()
        .map_or("The cell", |located| located.label.as_str());
    let stale = stale(label, action);
    // A base above the live revision is forged or from a lost future: a conflict.
    if base > live {
        return Err(Refusal::Stale(stale));
    }
    let located = located.ok_or_else(|| Refusal::Stale(stale.clone()))?;
    let current = assist::value_at(&located, &view.snapshot.data);
    if let Some((list, host)) = located.line() {
        // A row note (slice 1.5b): only Undo of the list's latest removal exists.
        if action == Action::Switch {
            return Err(Refusal::BadRequest);
        }
        let stored = view
            .notes
            .rows
            .get(&list.id)
            .filter(|stored| stored.revision == revision && stored.note.address == address)
            .filter(|stored| assist::removal_undoable(stored, current.as_ref()))
            .ok_or_else(|| Refusal::Stale(stale.clone()))?
            .clone();
        let before = earlier
            .filter(|before| before.revision.checked_add(1) == Some(revision))
            .ok_or_else(|| Refusal::Stale(stale.clone()))?;
        let next = live
            .checked_add(1)
            .ok_or_else(|| Refusal::Stale(stale.clone()))?;
        match assist::restore(list, host, &before.data, &mut view.snapshot.data, next) {
            Some(Ok(())) => {}
            Some(Err(AppError::Invalid(reason))) => return Err(Refusal::Invalid(reason)),
            _ => return Err(Refusal::Stale(stale)),
        }
        view.notes.pending = vec![assist::undo_note(&stored)];
        view.notes.bound = vec![stored];
        return Ok(stale);
    }
    let stored = view
        .notes
        .current
        .get(&address)
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

/// The message of a switch or Undo that finds its cell changed since.
fn stale(label: &str, action: Action) -> String {
    match action {
        Action::Switch => {
            format!("Not switched: {label} changed since, or it was already switched.")
        }
        Action::Undo => {
            format!("Nothing undone: {label} changed since, or this change was already undone.")
        }
    }
}
