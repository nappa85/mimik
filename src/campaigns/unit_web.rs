//! The per-field save routes (assisted sheet slice 1.5a, plan §3a "Per-cell save", D-C):
//! `POST /private-sheets/{id}/meta/{name|notes}` and `/fields/{field_id}`, each with exactly
//! `csrf`, `base` and `value` (plus `null` for a nullable field).
//!
//! Precedence as for sheet saves: parse, CSRF and request auth, then (D-X3) a bounded wait for
//! the `JOBS` permit, the read and edit rights (403; an unsupported pin is read-only, so it
//! is 403 too), then key admission and the route checks (400: a malformed or zero `base`, a
//! native pin, an ID outside the slug grammar, not in the pin, derived or a list). The value is read against the live revision
//! L: a submission equal to the saved value is "No change" (D-U), even from a stale page;
//! otherwise a `base` above L, or a unit whose value differs between `base` and L, is a
//! conflict (409) that re-renders the page with the draft and every `base` at L. The write
//! goes through the sheet writer, which rechecks rights, the active state and that L is
//! still live inside its `IMMEDIATE` transaction.
use super::{AppError, AuthState, Notice, Redirect, StatusCode, error, rejected_editor, rendered};
use crate::{
    auth,
    campaigns::{sheets::SheetView, snapshots::PrivateSnapshot, transport::Pairs, unit::Unit},
};
use askama::Template;
use axum::{
    extract::{Path, State},
    response::{Html, IntoResponse},
};
use std::collections::BTreeMap;
use std::time::Duration;
use tower_sessions::Session;

/// Body limit of a unit save: a 20,000-character value at 4 bytes per character, URL-encoded
/// (at most three times wider), plus the short pairs.
pub(super) const UNIT_POST_BYTES: usize = 256 * 1024;

/// How long a sheet write waits for the `JOBS` permit before the busy page (D-X3). It fits
/// inside the auth layer's 5 s request timeout.
pub(super) const PERMIT_WAIT: Duration = Duration::from_secs(2);

/// What a unit route names before the pin is read.
enum Route {
    Meta(String),
    Field(String),
}

impl Route {
    /// The route path below `/private-sheets/{id}/`, when it can be echoed: a known
    /// metadata name or an ID in the slug grammar.
    fn path(&self) -> Option<String> {
        match self {
            Self::Meta(which) => Unit::meta(which).map(|_| format!("meta/{which}")),
            Self::Field(id) => {
                crate::definitions::valid_field_id(id).then(|| format!("fields/{id}"))
            }
        }
    }
}

pub(super) async fn meta(
    State(state): State<AuthState>,
    session: Session,
    Path((id, which)): Path<(i64, String)>,
    body: axum::body::Bytes,
) -> super::ResultResponse {
    save(state, session, id, Route::Meta(which), &body).await
}

pub(super) async fn field(
    State(state): State<AuthState>,
    session: Session,
    Path((id, field)): Path<(i64, String)>,
    body: axum::body::Bytes,
) -> super::ResultResponse {
    save(state, session, id, Route::Field(field), &body).await
}

/// A unit draft to show again after a refused save (slice 1.5a): the unit's submitted value
/// and Known/Unknown state, in place of the saved value, with its control focused.
pub(super) struct UnitDraft {
    pub unit: Unit,
    pub value: String,
    pub null: Option<String>,
}

async fn save(
    state: AuthState,
    session: Session,
    id: i64,
    route: Route,
    body: &[u8],
) -> super::ResultResponse {
    let pairs = Pairs::parse(body)?;
    auth::check_csrf(&session, pairs.csrf()?).await?;
    let auth = auth::request_auth(&state, &session).await?;
    let csrf = auth::token(&session)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let Some(permit) = crate::definitions::admit_within(PERMIT_WAIT).await else {
        return busy(id, &route, &pairs, &csrf);
    };
    // `base` is read before admission only to load that revision in the same read as L.
    let base = pairs
        .value("base")
        .and_then(|base| super::values::integer(base).ok());
    let (view, base_snapshot, permit) = state
        .store()
        .private_read_base(auth.clone(), id, base, permit)
        .await
        .map_err(error)?;
    if !view.editable {
        return Err(StatusCode::FORBIDDEN);
    }
    // Native pins keep the full form (Sonnet m1); an unsupported pin never gets here, it is
    // read-only (403 above).
    let pin = view.immutable.as_ref().ok_or(StatusCode::BAD_REQUEST)?;
    let unit = match &route {
        Route::Meta(which) => Unit::meta(which),
        Route::Field(field) => Unit::field(pin.definition(), field),
    }
    .ok_or(StatusCode::BAD_REQUEST)?;
    let form = pairs.admit(&unit.keys(pin.definition()))?;
    let (planned, view, form, permit) = tokio::task::spawn_blocking(move || {
        let mut view = view;
        let planned = plan(&mut view, base_snapshot.as_ref(), unit, &form);
        (planned, view, form, permit)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    finish(
        &state,
        auth,
        (id, csrf),
        (view, permit),
        (unit, &form),
        planned?,
    )
    .await
}

/// Answers a planned unit save: writes it (303 to the unit), or says "No change", or
/// re-renders the live page with the draft (409, 422).
async fn finish(
    state: &AuthState,
    auth: auth::RequestAuth,
    (id, csrf): (i64, String),
    (view, permit): (SheetView, tokio::sync::OwnedSemaphorePermit),
    (unit, form): (Unit, &BTreeMap<String, String>),
    planned: Planned,
) -> super::ResultResponse {
    let draft = UnitDraft {
        unit,
        value: form["value"].clone(),
        null: form.get("null").cloned(),
    };
    let label = view
        .immutable
        .as_ref()
        .map(|pin| unit.label(pin.definition()).to_owned())
        .unwrap_or_default();
    match planned {
        Planned::Write => {}
        Planned::Unchanged => {
            return match state
                .store()
                .private_unchanged(auth.clone(), view.snapshot.sheet_id, view.snapshot.revision)
                .await
            {
                Ok(()) => {
                    rendered(
                        view,
                        permit,
                        csrf,
                        None,
                        StatusCode::OK,
                        Notice::status(format!(
                            "No change: {label} already holds this value, so nothing was saved."
                        )),
                    )
                    .await
                }
                Err(AppError::Conflict) => {
                    let (view, permit) =
                        rejected_editor(state, auth, id, None, Some(permit)).await?;
                    refused(
                        view,
                        permit,
                        csrf,
                        draft,
                        StatusCode::CONFLICT,
                        &label,
                        None,
                    )
                    .await
                }
                Err(failure) => Err(error(failure)),
            };
        }
        Planned::Conflict => {
            return refused(
                view,
                permit,
                csrf,
                draft,
                StatusCode::CONFLICT,
                &label,
                None,
            )
            .await;
        }
        Planned::Invalid(reason) => {
            return refused(
                view,
                permit,
                csrf,
                draft,
                StatusCode::UNPROCESSABLE_ENTITY,
                &label,
                Some(reason),
            )
            .await;
        }
    }
    let anchor = anchor(unit);
    match state
        .store()
        .private_save_keeping(auth.clone(), view, permit)
        .await
    {
        Ok((id, _permit)) => {
            Ok(Redirect::to(&format!("/private-sheets/{id}#{anchor}")).into_response())
        }
        Err((failure @ (AppError::Conflict | AppError::Invalid(_)), permit)) => {
            let (view, permit) = rejected_editor(state, auth, id, None, permit).await?;
            let status = error(&failure);
            let reason = match failure {
                AppError::Invalid(reason) => Some(reason),
                _ => None,
            };
            refused(view, permit, csrf, draft, status, &label, reason).await
        }
        Err((failure, _)) => Err(error(failure)),
    }
}

/// The element a successful save scrolls back to: the saved unit's label.
fn anchor(unit: Unit) -> String {
    match unit {
        Unit::Name => "meta-name-label".into(),
        Unit::Notes => "meta-notes-label".into(),
        Unit::Field(f) => format!("label-{f}"),
    }
}

/// Re-renders the live page with the refused unit's draft (409 or 422). Every unit form
/// carries `base` = L, so pressing Enter again is a conscious overwrite.
async fn refused(
    view: SheetView,
    permit: tokio::sync::OwnedSemaphorePermit,
    csrf: String,
    draft: UnitDraft,
    status: StatusCode,
    label: &str,
    reason: Option<&'static str>,
) -> super::ResultResponse {
    let message = match reason {
        Some(reason) => format!("{label} not saved: {reason} Your entry is kept below."),
        None => format!(
            "{label} not saved: it changed since this page was loaded (now revision {}). \
             Your entry is kept below; press Enter again to save it over the current value.",
            view.snapshot.revision
        ),
    };
    rendered(
        view,
        permit,
        csrf,
        None,
        status,
        Notice {
            message,
            draft: Some(draft),
            ..Notice::default()
        },
    )
    .await
}

/// The outcome of a planned unit save.
enum Planned {
    /// The patched snapshot (and its note) is ready for the writer.
    Write,
    /// The submission equals the saved value (D-U).
    Unchanged,
    /// The unit changed between `base` and L, or `base` is above L.
    Conflict,
    /// The value was refused (422).
    Invalid(&'static str),
}

/// Applies the unit form to `view` (the live sheet L read in this request). Order (plan §3a):
/// a malformed or zero `base` is 400 and a `base` above L a conflict; then the value is read
/// against L, so a no-op answers "No change" even from a stale page; then a unit whose
/// protected value differs between `base` and L is a conflict, before any value refusal
/// (as the full form's revision check precedes value validation).
fn plan(
    view: &mut SheetView,
    base: Option<&PrivateSnapshot>,
    unit: Unit,
    form: &BTreeMap<String, String>,
) -> Result<Planned, StatusCode> {
    let base_revision =
        super::values::integer(&form["base"]).map_err(|_| StatusCode::BAD_REQUEST)?;
    if base_revision < 1 {
        return Err(StatusCode::BAD_REQUEST);
    }
    let pin = view.immutable.as_ref().ok_or(StatusCode::BAD_REQUEST)?;
    let Some(base) = base.filter(|_| base_revision <= view.snapshot.revision) else {
        return Ok(Planned::Conflict);
    };
    let definition = pin.definition();
    let stale = unit.protected(definition, base) != unit.protected(definition, &view.snapshot);
    // The patch goes to a copy: a refusal re-renders the live revision unchanged.
    let mut patched = view.snapshot.clone();
    let mut pending = Vec::new();
    let applied = unit.apply(pin, &mut patched, (form, &view.notes.current), &mut pending);
    match applied {
        Ok(false) => Ok(Planned::Unchanged),
        _ if stale => Ok(Planned::Conflict),
        Ok(true) => {
            view.snapshot = patched;
            view.notes.pending = pending;
            Ok(Planned::Write)
        }
        Err(AppError::Invalid(reason)) => Ok(Planned::Invalid(reason)),
        Err(failure) => Err(error(failure)),
    }
}

#[derive(Template)]
#[template(path = "private_sheet_busy.html")]
struct BusyTemplate<'a> {
    sheet: i64,
    action: &'a str,
    csrf: &'a str,
    /// What the kept entry is for: "the field hp", "Name", "General notes"; `None` for a
    /// full or list form.
    unit: Option<String>,
    /// The submitted pairs other than `csrf` and the visible entry, kept as hidden inputs.
    hidden: Vec<(&'a str, &'a str)>,
    /// A unit's entry, shown in a control (a textarea when it holds a line break).
    value: Option<&'a str>,
    multiline: bool,
    /// The submitted form was larger than a re-render may echo (plan D7), so it was not kept.
    dropped: bool,
}

/// The styled busy answer (D-X3, Sonnet rev-3 M3): 503 with `Retry-After: 1`, the player's
/// submitted value kept in a form that posts the same unit again, so nothing typed is lost.
/// Nothing was read or written. A route that names no echoable unit is 400. Without the
/// permit the pin is not read, so the page names the field by its ID and offers the entry
/// as text, whatever the field's control (review round 1, Sonnet m3).
fn busy(id: i64, route: &Route, pairs: &Pairs, csrf: &str) -> super::ResultResponse {
    let path = route.path().ok_or(StatusCode::BAD_REQUEST)?;
    let unit = match route {
        Route::Meta(which) if which == "name" => "Name".to_owned(),
        Route::Meta(_) => "General notes".to_owned(),
        Route::Field(field) => format!("the field {field}"),
    };
    let value = pairs.value("value").unwrap_or_default();
    let hidden = ["base", "null"]
        .into_iter()
        .filter_map(|key| pairs.value(key).map(|value| (key, value)))
        .collect();
    busy_page(&BusyTemplate {
        sheet: id,
        action: &format!("/private-sheets/{id}/{path}"),
        csrf,
        unit: Some(unit),
        hidden,
        value: Some(value),
        multiline: value.contains('\n'),
        dropped: false,
    })
}

/// Markup bytes charged per kept pair on the busy page (its hidden input's tags and
/// attribute names), so the echo bound covers the page, not only the values.
const BUSY_PAIR_MARKUP: usize = 48;

/// Whether a busy page may keep `pairs`: their keys, values and per-pair markup total at
/// most the re-render echo bound (plan D7, 1 MiB), so the page stays within that bound at the
/// widest escape (5 bytes per byte) plus its fixed chrome (review round 2, Sonnet F3).
fn keepable<'a>(pairs: impl Iterator<Item = (&'a str, &'a str)>) -> bool {
    pairs
        .map(|(key, value)| key.len() + value.len() + BUSY_PAIR_MARKUP)
        .sum::<usize>()
        <= super::ECHO_BYTES
}

/// The busy answer of the full-form and list-form save (review round 1, Sonnet M1): after
/// the same bounded wait, every pair the caller submitted (parsed, CSRF and session checked
/// before the wait, review round 2) is kept as a hidden input of a form posting to the sheet
/// again. A form larger than the echo bound is not kept, and the page says so. It echoes
/// only the caller's own submission: the sheet and the caller's rights on it are not read
/// (that needs the permit).
pub(super) fn busy_form(id: i64, pairs: &Pairs, csrf: &str) -> super::ResultResponse {
    let dropped = !keepable(pairs.iter());
    let hidden = if dropped {
        Vec::new()
    } else {
        pairs.iter().filter(|(key, _)| *key != "csrf").collect()
    };
    busy_page(&BusyTemplate {
        sheet: id,
        action: &format!("/private-sheets/{id}"),
        csrf,
        unit: None,
        hidden,
        value: None,
        multiline: false,
        dropped,
    })
}

fn busy_page(template: &BusyTemplate<'_>) -> super::ResultResponse {
    let html = template
        .render()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut response = (
        StatusCode::SERVICE_UNAVAILABLE,
        [(axum::http::header::RETRY_AFTER, "1")],
        Html(html),
    )
        .into_response();
    response.extensions_mut().insert(super::SheetBusy);
    Ok(response)
}
