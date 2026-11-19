//! The reminder route (assisted sheet plan 3c-1, "Route"), present only with the assistant on:
//! `POST /private-sheets/{id}/reminders` with exactly `csrf` and `note` (the key of the
//! decrease note a save's JSON answer carried) starts a reminder job: 202 with its ID and the
//! poll path of the interpretation routes. Off, unavailable or not ready the route answers 404
//! after the parse and the CSRF check, before rights; a trigger that no longer holds is 409.
use super::interpret_web::{refusal_json, reply};
use super::{StatusCode, error};
use crate::{
    assistant::jobs::{Kind, Origin, ReminderDraft, ReminderRefusal},
    auth::{self, AuthState},
    campaigns::{assist, catalogue::EntryWant, notes::NoteValue, remind, transport::Pairs},
};
use axum::extract::{Path, State};
use serde_json::json;
use tower_sessions::Session;

const CHANGED: &str = "This entry changed since it was saved; nothing to remind.";

fn conflict() -> axum::response::Response {
    refusal_json(StatusCode::CONFLICT, "conflict", CHANGED)
}

/// Starts a reminder for the decrease note `note` names.
pub(super) async fn start(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    body: axum::body::Bytes,
) -> super::ResultResponse {
    let assistant = state.assistant().ok_or(StatusCode::NOT_FOUND)?;
    let pairs = Pairs::parse(&body)?;
    auth::check_csrf(&session, pairs.csrf()?).await?;
    // Off, unavailable for the generation or not ready: the module stays quiet.
    if !assistant.reminders_effective() {
        return Err(StatusCode::NOT_FOUND);
    }
    let auth = auth::request_auth(&state, &session).await?;
    let note = pairs.value("note").map(str::to_owned);
    let key = note.as_deref().and_then(assist::parse_key);
    let registration = key.map(|(revision, _)| assistant.register(id, revision));
    let permit = crate::definitions::admit().map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let (view, _, permit) = state
        .store()
        .private_read_base(auth.clone(), id, None, permit)
        .await
        .map_err(error)?;
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
    let assist::Named::Address(address) = named else {
        return Err(StatusCode::BAD_REQUEST);
    };
    if !address.starts_with("field:") || !assist::in_pin(definition, address) {
        return Err(StatusCode::BAD_REQUEST);
    }
    // Re-derived from the stored note: current, a top-level integer field, an allowed op, a
    // known `before` over `after`, the field still holding `after`, no applied reminder yet.
    let Some(stored) = view
        .notes
        .current
        .get(address)
        .filter(|stored| stored.revision == revision)
    else {
        return Ok(conflict());
    };
    let Some(decrease) = remind::trigger(stored) else {
        return Ok(conflict());
    };
    let Some(located) = assist::locate(definition, &view.snapshot.data, address) else {
        return Ok(conflict());
    };
    if assist::value_at(&located, &view.snapshot.data) != Some(NoteValue::Int(decrease.1))
        || remind::applied(view.notes.current.values(), stored)
    {
        return Ok(conflict());
    }
    let Some(mut request) = remind::build(
        definition,
        &view.snapshot.data,
        stored,
        decrease,
        &view.entry_links,
    ) else {
        return Ok(conflict());
    };
    attach_entries(&state, &auth, id, definition, &mut request, permit).await?;
    let origin = Origin {
        sheet: id,
        target: address.to_owned(),
        revision,
        kind: Kind::Reminder,
    };
    match assistant.start_reminder(
        &registration,
        auth.account_id(),
        origin,
        ReminderDraft { request },
    ) {
        Ok(job) => Ok(reply(
            StatusCode::ACCEPTED,
            &json!({
                "status": "started",
                "job": job.text(),
                "poll": format!("/private-sheets/{id}/interpretations/{}", job.text()),
            }),
        )),
        Err(ReminderRefusal::NotAvailable) => Err(StatusCode::NOT_FOUND),
        // Transient: the module tries again on a later decrease.
        Err(ReminderRefusal::BreakerNotClosed) => Err(StatusCode::SERVICE_UNAVAILABLE),
        Err(ReminderRefusal::Conflict) => Ok(conflict()),
        Err(ReminderRefusal::Busy) => Ok(refusal_json(
            StatusCode::TOO_MANY_REQUESTS,
            "busy",
            "The assistant is busy, try again shortly.",
        )),
    }
}

/// The linked entries' text (plan 3c-2, step 3), read under the permit the request holds; a
/// row whose entry cannot be read keeps its name only.
async fn attach_entries(
    state: &AuthState,
    auth: &auth::RequestAuth,
    id: i64,
    definition: &crate::definitions::Definition,
    request: &mut crate::assistant::reminder::ReminderRequest,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<(), StatusCode> {
    let assistant = state.assistant().ok_or(StatusCode::NOT_FOUND)?;
    let wanted: Vec<EntryWant> = request
        .rows
        .iter()
        .filter_map(|row| row.entry.as_ref())
        .map(|binding| EntryWant {
            address: String::new(),
            catalogue: binding.catalogue.clone(),
            version: binding.version,
            key: binding.key.clone(),
            characters: 0,
        })
        .collect();
    let bytes = assistant
        .budgets()
        .map_or(0, |budgets| budgets.bounds.bytes);
    if wanted.is_empty() || bytes == 0 {
        drop(permit);
        for row in &mut request.rows {
            row.attach(None);
        }
        return Ok(());
    }
    let (contexts, permit) = state
        .store()
        .reminder_entries(
            auth.clone(),
            id,
            (definition.game_id.clone(), definition.edition_id.clone()),
            wanted,
            bytes,
            permit,
        )
        .await
        .map_err(error)?;
    drop(permit);
    let mut contexts = contexts.into_iter();
    for row in &mut request.rows {
        let context = row
            .entry
            .is_some()
            .then(|| contexts.next().flatten())
            .flatten()
            .map(|context| {
                (
                    (context.name, context.badge.label().to_owned(), context.text),
                    context.cut,
                )
            });
        match context {
            Some((context, cut)) => row.attach_cut(Some(context), cut),
            None => row.attach(None),
        }
    }
    Ok(())
}
