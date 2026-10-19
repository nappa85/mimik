//! The per-field and per-cell save routes (assisted sheet slices 1.5a and 1.5b, plan §3a
//! "Per-cell save", D-C, D-S, D-V): `POST /private-sheets/{id}/meta/{name|notes}` and
//! `/fields/{field_id}`, each with exactly `csrf`, `base` and `value` (plus `null` for a
//! nullable field); `/cells/{f}/{r}/{p}` with `csrf`, `base` and `value` (plus `set` for an
//! optional property); `/lists/{f}/rows` with `csrf`, `base` and that row's `row-<p>` (and
//! `set-<p>`); and `/lists/{f}/rows/{r}/{remove|up|down}` with `csrf` and `base`. List
//! routes name a list, a row (its index at `base`) and a property by index (`super::rows`).
//!
//! Precedence as for sheet saves: parse, CSRF and request auth, then (D-X3) a bounded wait for
//! the `JOBS` permit, the read and edit rights (403; an unsupported pin is read-only, so it
//! is 403 too), then key admission and the route checks (400: a malformed or zero `base`, a
//! native pin, an ID outside the slug grammar, not in the pin, derived or a list; for list
//! routes an index outside the canonical grammar, not a list of the pin or not its
//! property, a row missing at `base` or gone from L). The value is read against the live revision
//! L: a submission equal to the saved value is "No change" (D-U), even from a stale page;
//! otherwise a `base` above L, or a unit whose value differs between `base` and L, is a
//! conflict (409) that re-renders the page with the draft and every `base` at L. The write
//! goes through the sheet writer, which rechecks rights, the active state and that L is
//! still live inside its `IMMEDIATE` transaction.
use super::{AppError, AuthState, Notice, Redirect, StatusCode, error, rejected_editor, rendered};
use crate::{
    auth,
    campaigns::{
        rows::{self, Plan},
        sheets::SheetView,
        snapshots::PrivateSnapshot,
        transport::Pairs,
        unit::{Planned, RowAction, Unit},
    },
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
    /// `cells/{f}/{r}/{p}` (slice 1.5b).
    Cell(String, String, String),
    /// `lists/{f}/rows`.
    NewRow(String),
    /// `lists/{f}/rows/{r}/{action}`.
    Row(String, String, String),
}

impl Route {
    /// The route path below `/private-sheets/{id}/`, when it can be echoed: a known
    /// metadata name, an ID in the slug grammar, canonical indices and a row action.
    fn path(&self) -> Option<String> {
        let index = |raw: &str| super::values::index(raw).is_some();
        match self {
            Self::Meta(which) => Unit::meta(which).map(|_| format!("meta/{which}")),
            Self::Field(id) => {
                crate::definitions::valid_field_id(id).then(|| format!("fields/{id}"))
            }
            Self::Cell(f, r, p) => {
                (index(f) && index(r) && index(p)).then(|| format!("cells/{f}/{r}/{p}"))
            }
            Self::NewRow(f) => index(f).then(|| format!("lists/{f}/rows")),
            Self::Row(f, r, action) => (index(f) && index(r) && RowAction::parse(action).is_some())
                .then(|| format!("lists/{f}/rows/{r}/{action}")),
        }
    }

    /// The unit the route names in the pinned definition (400 otherwise).
    fn unit(&self, definition: &crate::definitions::Definition) -> Option<Unit> {
        match self {
            Self::Meta(which) => Unit::meta(which),
            Self::Field(field) => Unit::field(definition, field),
            Self::Cell(f, r, p) => Unit::cell(definition, (f, r, p)),
            Self::NewRow(f) => Unit::new_row(definition, f),
            Self::Row(f, r, action) => Unit::row(definition, (f, r, action)),
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

pub(super) async fn cell(
    State(state): State<AuthState>,
    session: Session,
    Path((id, f, r, p)): Path<(i64, String, String, String)>,
    body: axum::body::Bytes,
) -> super::ResultResponse {
    save(state, session, id, Route::Cell(f, r, p), &body).await
}

pub(super) async fn new_row(
    State(state): State<AuthState>,
    session: Session,
    Path((id, f)): Path<(i64, String)>,
    body: axum::body::Bytes,
) -> super::ResultResponse {
    save(state, session, id, Route::NewRow(f), &body).await
}

pub(super) async fn row(
    State(state): State<AuthState>,
    session: Session,
    Path((id, f, r, action)): Path<(i64, String, String, String)>,
    body: axum::body::Bytes,
) -> super::ResultResponse {
    save(state, session, id, Route::Row(f, r, action), &body).await
}

/// Body limit of a row action: two short pairs.
pub(super) const ROW_POST_BYTES: usize = 8192;

/// A unit draft to show again after a refused save (slice 1.5a): the unit's submitted value
/// and Known/Unknown (or, for a row cell, Set/Not set) state, in place of the saved value,
/// with its control focused. A cell's row is named by its host ID, so the draft is shown in
/// that row whatever moved since; a new-row draft keeps every pair of its form.
pub(super) struct UnitDraft {
    pub unit: Unit,
    pub value: String,
    pub null: Option<String>,
    pub host: Option<String>,
    pub row: Option<BTreeMap<String, String>>,
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
    let unit = route
        .unit(pin.definition())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let form = pairs.admit(&unit.keys(pin.definition()))?;
    if matches!(unit, Unit::NewRow(_))
        && form.values().map(String::len).sum::<usize>() > super::ECHO_BYTES
    {
        // A row above the echo bound could never be stored (plan D7): not echoed.
        return rendered(
            view,
            permit,
            csrf,
            None,
            StatusCode::UNPROCESSABLE_ENTITY,
            Notice {
                alert: "This row is larger than the 1 MiB sheet limit and was not kept. \
                        Nothing was saved; copy large text before submitting."
                    .into(),
                ..Notice::default()
            },
        )
        .await;
    }
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
    plan: Plan,
) -> super::ResultResponse {
    let draft = UnitDraft {
        unit,
        value: form.get("value").cloned().unwrap_or_default(),
        null: form.get("null").or_else(|| form.get("set")).cloned(),
        host: plan.host,
        row: matches!(unit, Unit::NewRow(_)).then(|| form.clone()),
    };
    let label = view
        .immutable
        .as_ref()
        .map(|pin| unit.label(pin.definition()))
        .unwrap_or_default();
    match plan.planned {
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
                        Notice::status(unchanged(unit, &label)),
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
    let anchor = plan.anchor;
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

/// The element a successful metadata or field save scrolls back to: its label.
fn anchor(unit: Unit) -> String {
    match unit {
        Unit::Name => "meta-name-label".into(),
        Unit::Notes => "meta-notes-label".into(),
        Unit::Field(f) => format!("label-{f}"),
        Unit::Cell { .. } | Unit::NewRow(_) | Unit::Row { .. } => String::new(),
    }
}

/// The "No change" status of a unit.
fn unchanged(unit: Unit, label: &str) -> String {
    match unit {
        Unit::Row {
            action: RowAction::Remove,
            ..
        } => format!("No change: {label} was already removed, so nothing was saved."),
        _ => format!("No change: {label} already holds this value, so nothing was saved."),
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
    let revision = view.snapshot.revision;
    let message = match (draft.unit, reason) {
        (Unit::Row { action, .. }, reason) => {
            let verb = if action == RowAction::Remove {
                "removed"
            } else {
                "moved"
            };
            match reason {
                Some(reason) => format!("{label} not {verb}: {reason}"),
                None => format!(
                    "{label} not {verb}: the list changed since this page was loaded (now \
                     revision {revision}). Check the row and press the button again."
                ),
            }
        }
        (Unit::NewRow(_), Some(reason)) => {
            format!("{label} was not added: {reason} Your row is kept below.")
        }
        (Unit::NewRow(_), None) => format!(
            "{label} was not added: the sheet changed while it was saved (now revision \
             {revision}). Your row is kept below; press Enter again to add it."
        ),
        (_, Some(reason)) => format!("{label} not saved: {reason} Your entry is kept below."),
        (_, None) => format!(
            "{label} not saved: it changed since this page was loaded (now revision \
             {revision}). Your entry is kept below; press Enter again to save it over the \
             current value."
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
) -> Result<Plan, StatusCode> {
    let base_revision =
        super::values::integer(&form["base"]).map_err(|_| StatusCode::BAD_REQUEST)?;
    if base_revision < 1 {
        return Err(StatusCode::BAD_REQUEST);
    }
    let pin = view.immutable.as_ref().ok_or(StatusCode::BAD_REQUEST)?;
    let Some(base) = base.filter(|_| base_revision <= view.snapshot.revision) else {
        // A `base` above L names no page that was read; a cell's draft is shown in its
        // row's position in L.
        let host = match unit {
            Unit::Cell { f, r, .. } => rows::host(pin, &view.snapshot, (f, r)),
            _ => None,
        };
        return Ok(Plan {
            planned: Planned::Conflict,
            anchor: anchor(unit),
            host,
        });
    };
    if matches!(unit, Unit::Name | Unit::Notes | Unit::Field(_)) {
        let planned = scalar(view, base, unit, form)?;
        return Ok(Plan {
            planned,
            anchor: anchor(unit),
            host: None,
        });
    }
    // List units (slice 1.5b) are planned on a copy: a refusal re-renders L unchanged.
    let mut patched = view.snapshot.clone();
    let mut pending = Vec::new();
    let current = &view.notes.current;
    let plan = match unit {
        Unit::Cell { f, r, p } => rows::cell(
            pin,
            (&mut patched, base),
            (f, r, p),
            (form, current),
            &mut pending,
        )?,
        Unit::NewRow(f) => rows::new_row(pin, &mut patched, f, (form, current), &mut pending)?,
        Unit::Row { f, r, action } => {
            rows::action(pin, (&mut patched, base), (f, r, action), &mut pending)?
        }
        // Planned above.
        Unit::Name | Unit::Notes | Unit::Field(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };
    if matches!(plan.planned, Planned::Write) {
        view.snapshot = patched;
        view.notes.pending = pending;
    }
    Ok(plan)
}

/// The plan of a metadata or scalar unit (slice 1.5a).
fn scalar(
    view: &mut SheetView,
    base: &PrivateSnapshot,
    unit: Unit,
    form: &BTreeMap<String, String>,
) -> Result<Planned, StatusCode> {
    let pin = view.immutable.as_ref().ok_or(StatusCode::BAD_REQUEST)?;
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
    /// full, list or new-row form and a row action.
    unit: Option<String>,
    /// A row action's retry button ("Remove row 2 again", slice 1.5b).
    retry: Option<String>,
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
    let action = format!("/private-sheets/{id}/{path}");
    let unit = match route {
        Route::Meta(which) if which == "name" => "Name".to_owned(),
        Route::Meta(_) => "General notes".to_owned(),
        Route::Field(field) => format!("the field {field}"),
        // Slice 1.5b: a cell is named by its indices (the labels need the pin).
        Route::Cell(f, r, p) => format!(
            "column {} of row {} of list field {f}",
            index(p) + 1,
            index(r) + 1
        ),
        // A new row keeps every pair, like a full form; a row action only its `base`.
        Route::NewRow(_) => return busy_pairs(id, &action, pairs, csrf),
        Route::Row(_, r, verb) => {
            let retry = match RowAction::parse(verb) {
                Some(RowAction::Remove) => format!("Remove row {} again", index(r) + 1),
                Some(RowAction::Up) => format!("Move row {} up again", index(r) + 1),
                _ => format!("Move row {} down again", index(r) + 1),
            };
            return busy_page(&BusyTemplate {
                sheet: id,
                action: &action,
                csrf,
                unit: None,
                retry: Some(retry),
                hidden: pairs
                    .value("base")
                    .map(|base| ("base", base))
                    .into_iter()
                    .collect(),
                value: None,
                multiline: false,
                dropped: false,
            });
        }
    };
    let value = pairs.value("value").unwrap_or_default();
    let hidden = ["base", "null", "set"]
        .into_iter()
        .filter_map(|key| pairs.value(key).map(|value| (key, value)))
        .collect();
    busy_page(&BusyTemplate {
        sheet: id,
        action: &action,
        csrf,
        unit: Some(unit),
        retry: None,
        hidden,
        value: Some(value),
        multiline: value.contains('\n'),
        dropped: false,
    })
}

/// A canonical route index ([`Route::path`] admitted it).
fn index(raw: &str) -> usize {
    super::values::index(raw).unwrap_or_default()
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
    busy_pairs(id, &format!("/private-sheets/{id}"), pairs, csrf)
}

/// The busy answer keeping every submitted pair for `action` (a full or list form, or a
/// new-row form, slice 1.5b) within the echo bound.
fn busy_pairs(id: i64, action: &str, pairs: &Pairs, csrf: &str) -> super::ResultResponse {
    let dropped = !keepable(pairs.iter());
    let hidden = if dropped {
        Vec::new()
    } else {
        pairs.iter().filter(|(key, _)| *key != "csrf").collect()
    };
    busy_page(&BusyTemplate {
        sheet: id,
        action,
        csrf,
        unit: None,
        retry: None,
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
