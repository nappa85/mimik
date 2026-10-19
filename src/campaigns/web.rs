use super::domain::{Detail, DetailPage, Workspace};
use super::{
    formulas::{DerivedView, FormulaView},
    sheets::{HistorySummary, SheetSummary, SheetView},
    snapshots::Pin,
    values,
};
use crate::{
    AppError,
    auth::{self, AuthState},
};
use askama::Template;
use axum::{
    Extension,
    extract::Request,
    middleware::{self, Next},
};
use axum::{
    Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::sync::OwnedSemaphorePermit;
use tower_sessions::Session;

#[derive(Clone)]
pub(super) struct RuntimeAdmission(Arc<Mutex<Option<OwnedSemaphorePermit>>>);
impl RuntimeAdmission {
    pub(super) fn take(&self) -> Result<OwnedSemaphorePermit, StatusCode> {
        self.0
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .take()
            .ok_or(StatusCode::SERVICE_UNAVAILABLE)
    }
}
async fn runtime(mut request: Request, next: Next) -> Response {
    let Ok(permit) = crate::definitions::admit() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(axum::http::header::RETRY_AFTER, "1")],
            "Runtime worker busy.",
        )
            .into_response();
    };
    request
        .extensions_mut()
        .insert(RuntimeAdmission(Arc::new(Mutex::new(Some(permit)))));
    next.run(request).await
}

#[derive(Clone)]
pub(crate) struct PrivateTrustedEditor;

/// Marks a unit save's styled busy page (503, D-X3): the auth layer keeps its body, which
/// holds the player's entry.
#[derive(Clone)]
pub(crate) struct SheetBusy;

/// Whether the auth layer keeps this response's body as a sheet's busy page.
pub(crate) fn sheet_busy(response: &Response) -> bool {
    response.status() == StatusCode::SERVICE_UNAVAILABLE
        && response.extensions().get::<SheetBusy>().is_some()
}

/// Marks a catalogue route's own explanatory page (plan §19): 400 for a malformed address,
/// 403 for the uniform refusal, 404 for an entry or type missing from a catalogue the reader
/// may browse. The auth layer keeps such a body instead of its generic one.
#[derive(Clone)]
pub(crate) struct CatalogueNotice;

/// Whether the auth layer keeps this response's body as a catalogue notice.
pub(crate) fn catalogue_notice(response: &Response) -> bool {
    matches!(
        response.status(),
        StatusCode::BAD_REQUEST | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
    ) && response.extensions().get::<CatalogueNotice>().is_some()
}

/// Whether each `rejected_editor` call received the rejected save's own permit (tests only).
#[cfg(test)]
pub(super) static REJECTED_EDITOR_KEPT_PERMIT: Mutex<Vec<bool>> = Mutex::new(Vec::new());

/// Runs once between a failed save and the re-render's permit request (tests only).
#[cfg(test)]
pub(super) static AFTER_FAILED_SAVE: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);

/// A planned unit save signals the first sender, then waits for the receiver before it
/// writes (tests only: a concurrent writer between planning and the writer).
#[cfg(test)]
pub(super) type PlanHook = (
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
);
#[cfg(test)]
pub(super) static AFTER_UNIT_PLAN: Mutex<Option<PlanHook>> = Mutex::new(None);

/// Body limit of the private sheet create and edit POSTs (plan D7): stored values are at
/// most 1 MiB, URL encoding expands them at most 3×, and keys add at most 65,536 × 16 bytes.
pub(super) const SHEET_POST_BYTES: usize = 5 * 1024 * 1024;

/// A re-render echoes a draft only when its decoded form values total at most this many
/// bytes: a larger draft could never be stored (plan D7, N8).
pub(super) const ECHO_BYTES: usize = 1024 * 1024;

/// Before a 5 MiB body is buffered, the session must carry a principal (cookie only, no
/// body): an anonymous request answers 403 without its body being read (plan D7, N3).
async fn signed_in(session: Session, request: Request, next: Next) -> Response {
    match auth::has_principal(&session).await {
        Ok(true) => next.run(request).await,
        Ok(false) => StatusCode::FORBIDDEN.into_response(),
        Err(status) => status.into_response(),
    }
}

/// A sheet POST method: its own 5 MiB body limit (inside the router-wide 1 MiB layer, so it
/// wins for this method only) and the pre-body principal check.
fn sheet_post<H, T>(handler: H) -> axum::routing::MethodRouter<AuthState>
where
    H: axum::handler::Handler<T, AuthState>,
    T: 'static,
{
    post(handler)
        .layer(DefaultBodyLimit::max(SHEET_POST_BYTES))
        .route_layer(middleware::from_fn(signed_in))
}

#[path = "assist_web.rs"]
mod assist_web;
#[path = "unit_web.rs"]
mod unit_web;

pub(crate) fn routes() -> Router<AuthState> {
    let sheets = Router::new()
        .route(
            "/campaigns/{id}/sheets/new",
            get(new_sheet).merge(sheet_post(create_sheet)),
        )
        .route("/private-sheets/{id}", get(read_sheet))
        .route("/private-sheets/{id}/history/{revision}", get(read_history))
        .route(
            "/private-sheets/{id}/switch",
            post(assist_web::switch).layer(DefaultBodyLimit::max(assist_web::ASSIST_POST_BYTES)),
        )
        .route(
            "/private-sheets/{id}/undo",
            post(assist_web::undo).layer(DefaultBodyLimit::max(assist_web::ASSIST_POST_BYTES)),
        )
        .route(
            "/private-sheets/{id}/catalogue/{catalogue}",
            get(super::catalogue_web::browse),
        )
        .route(
            "/private-sheets/{id}/catalogue/{catalogue}/{key}",
            get(super::catalogue_web::entry),
        )
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(middleware::from_fn(runtime));
    // Per-field and per-cell saves (slices 1.5a, 1.5b) take the runtime permit themselves,
    // with a bounded wait (D-X3), so they are outside the `runtime` layer.
    let units = Router::new()
        .route("/private-sheets/{id}/meta/{which}", post(unit_web::meta))
        .route("/private-sheets/{id}/fields/{field}", post(unit_web::field))
        .route(
            "/private-sheets/{id}/cells/{f}/{r}/{p}",
            post(unit_web::cell),
        )
        .layer(DefaultBodyLimit::max(unit_web::UNIT_POST_BYTES))
        .route_layer(middleware::from_fn(signed_in))
        // A new row may carry every property of a row, as the full form may.
        .route(
            "/private-sheets/{id}/lists/{f}/rows",
            sheet_post(unit_web::new_row),
        )
        .route(
            "/private-sheets/{id}/lists/{f}/rows/{r}/{action}",
            post(unit_web::row)
                .layer(DefaultBodyLimit::max(unit_web::ROW_POST_BYTES))
                .route_layer(middleware::from_fn(signed_in)),
        )
        // The full-form and list-form save waits the same way (review round 1, Sonnet M1).
        .route("/private-sheets/{id}", sheet_post(edit_sheet));
    Router::new()
        .route("/campaigns", get(index).post(create))
        .route("/campaigns/{id}", get(detail))
        .route("/campaigns/{id}/join", post(join))
        .route("/campaigns/{id}/requests/{target}", post(decide))
        .route("/campaigns/{id}/members/{target}", post(member))
        .route("/private-sheets", get(personal))
        .route("/private-sheets/{id}/history", get(history))
        .layer(DefaultBodyLimit::max(8192))
        .merge(sheets)
        .merge(units)
}

pub(super) fn error(error: impl std::borrow::Borrow<AppError>) -> StatusCode {
    match error.borrow() {
        AppError::NotFound => StatusCode::FORBIDDEN,
        AppError::Conflict => StatusCode::CONFLICT,
        AppError::Invalid(_) => StatusCode::UNPROCESSABLE_ENTITY,
        AppError::AuthStorageBusy => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub(super) type ResultResponse = Result<Response, StatusCode>;
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Page {
    #[serde(default)]
    after: i64,
}

#[derive(Template)]
#[template(path = "campaigns.html")]
struct IndexTemplate<'a> {
    workspace: &'a Workspace,
    csrf: &'a str,
}

#[derive(Template)]
#[template(path = "campaign.html")]
struct DetailTemplate<'a> {
    detail: &'a Detail,
    csrf: &'a str,
    account_id: i64,
    sheets: &'a [SheetSummary],
    sheets_next: Option<i64>,
    page: DetailPage,
}

async fn index(
    State(state): State<AuthState>,
    session: Session,
    Query(page): Query<Page>,
) -> ResultResponse {
    if page.after < 0 {
        return Err(StatusCode::BAD_REQUEST);
    }
    let auth = auth::request_auth(&state, &session).await?;
    let workspace = state
        .store()
        .workspace(auth, page.after)
        .await
        .map_err(error)?;
    let csrf = auth::token(&session)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    Ok(Html(
        IndexTemplate {
            workspace: &workspace,
            csrf: &csrf,
        }
        .render()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    )
    .into_response())
}

async fn detail(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    Query(page): Query<DetailPage>,
) -> ResultResponse {
    if !page.valid() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let auth = auth::request_auth(&state, &session).await?;
    let account_id = auth.account_id();
    let detail = state
        .store()
        .campaign_detail_page(auth.clone(), id, page)
        .await
        .map_err(error)?;
    let sheets = if detail.role.is_empty() {
        super::Listing {
            rows: Vec::new(),
            next: None,
        }
    } else {
        state
            .store()
            .private_list_page(auth, Some(id), page.sheets)
            .await
            .map_err(error)?
    };
    let csrf = auth::token(&session)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    Ok(Html(
        DetailTemplate {
            detail: &detail,
            csrf: &csrf,
            account_id,
            sheets: &sheets.rows,
            sheets_next: sheets.next,
            page,
        }
        .render()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    )
    .into_response())
}

fn pairs(bytes: &[u8], keys: &[&str]) -> Result<BTreeMap<String, String>, StatusCode> {
    super::transport::Pairs::parse(bytes)?.admit(&keys.iter().copied().collect())
}

async fn create(
    State(state): State<AuthState>,
    session: Session,
    body: axum::body::Bytes,
) -> ResultResponse {
    let form = pairs(&body, &["csrf", "title", "game_id", "edition_id"])?;
    auth::check_csrf(&session, &form["csrf"]).await?;
    let auth = auth::request_auth(&state, &session).await?;
    let id = state
        .store()
        .create_campaign(
            auth,
            form["title"].clone(),
            form["game_id"].clone(),
            form["edition_id"].clone(),
        )
        .await
        .map_err(error)?;
    Ok(Redirect::to(&format!("/campaigns/{id}")).into_response())
}

async fn join(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    body: axum::body::Bytes,
) -> ResultResponse {
    let form = pairs(&body, &["csrf"])?;
    auth::check_csrf(&session, &form["csrf"]).await?;
    let auth = auth::request_auth(&state, &session).await?;
    state.store().request_join(auth, id).await.map_err(error)?;
    Ok(Redirect::to(&format!("/campaigns/{id}")).into_response())
}

async fn decide(
    State(state): State<AuthState>,
    session: Session,
    Path((id, target)): Path<(i64, i64)>,
    body: axum::body::Bytes,
) -> ResultResponse {
    let form = pairs(&body, &["csrf", "revision", "action"])?;
    auth::check_csrf(&session, &form["csrf"]).await?;
    let revision = form["revision"]
        .parse()
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    if !matches!(form["action"].as_str(), "accept" | "decline") {
        return Err(StatusCode::BAD_REQUEST);
    }
    let auth = auth::request_auth(&state, &session).await?;
    state
        .store()
        .decide_join(auth, id, target, revision, form["action"] == "accept")
        .await
        .map_err(error)?;
    Ok(Redirect::to(&format!("/campaigns/{id}")).into_response())
}

async fn member(
    State(state): State<AuthState>,
    session: Session,
    Path((id, target)): Path<(i64, i64)>,
    body: axum::body::Bytes,
) -> ResultResponse {
    let form = pairs(&body, &["csrf", "revision", "action"])?;
    auth::check_csrf(&session, &form["csrf"]).await?;
    let revision = form["revision"]
        .parse()
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let auth = auth::request_auth(&state, &session).await?;
    match form["action"].as_str() {
        "promote" | "demote" => state
            .store()
            .change_role(auth, id, target, revision, form["action"] == "promote")
            .await
            .map_err(error)?,
        "leave" if target == auth.account_id() => state
            .store()
            .depart(auth, id, target, revision)
            .await
            .map_err(error)?,
        "remove" if target != auth.account_id() => state
            .store()
            .depart(auth, id, target, revision)
            .await
            .map_err(error)?,
        _ => return Err(StatusCode::BAD_REQUEST),
    }
    Ok(Redirect::to(&format!("/campaigns/{id}")).into_response())
}

#[derive(Template)]
#[template(path = "private_list.html")]
struct PersonalTemplate<'a> {
    sheets: &'a [SheetSummary],
    next: Option<i64>,
}
#[derive(Template)]
#[template(path = "private_history.html")]
struct HistoryTemplate<'a> {
    id: i64,
    rows: &'a [DatedHistory],
    next: Option<i64>,
}
struct DatedHistory {
    summary: HistorySummary,
    date: crate::presentation::DisplayTime,
}

async fn personal(
    State(state): State<AuthState>,
    session: Session,
    Query(page): Query<Page>,
) -> ResultResponse {
    let auth = auth::request_auth(&state, &session).await?;
    let sheets = state
        .store()
        .private_list_page(auth, None, page.after)
        .await
        .map_err(error)?;
    Ok(Html(
        PersonalTemplate {
            sheets: &sheets.rows,
            next: sheets.next,
        }
        .render()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    )
    .into_response())
}
async fn history(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    Query(page): Query<Page>,
) -> ResultResponse {
    let auth = auth::request_auth(&state, &session).await?;
    let rows = state
        .store()
        .private_history_page(auth, id, page.after)
        .await
        .map_err(error)?;
    let dated = rows
        .rows
        .into_iter()
        .map(|summary| DatedHistory {
            date: crate::presentation::datetime(summary.occurred_at),
            summary,
        })
        .collect::<Vec<_>>();
    Ok(Html(
        HistoryTemplate {
            id,
            rows: &dated,
            next: rows.next,
        }
        .render()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    )
    .into_response())
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Version {
    #[serde(default)]
    version: i64,
}

/// A sheet view's optional focus after a row move (slice 1.5b review): `up-<f>-<r>` or
/// `down-<f>-<r>`; anything else is ignored.
#[derive(Deserialize, Default)]
struct SheetQuery {
    #[serde(default)]
    focus: Option<String>,
}

impl SheetQuery {
    fn focus(&self) -> Option<values::Focus> {
        let (verb, rest) = self.focus.as_deref()?.split_once('-')?;
        let (f, r) = rest.split_once('-')?;
        let (f, r) = (values::index(f)?, values::index(r)?);
        match verb {
            "up" => Some(values::Focus::Up(f, r)),
            "down" => Some(values::Focus::Down(f, r)),
            _ => None,
        }
    }
}

struct FieldView {
    id: String,
    /// The DOM id suffix of the field's note, label and actions: its index in the
    /// definition, so a 64-character field ID is not repeated in every reference (D7).
    key: String,
    /// The control's DOM id and the names of its value and Known/Unknown controls: in a
    /// field's own form (slice 1.5a) `v<key>`, `value` and `null`; in the full form
    /// `value-<id>`, `value-<id>` and `null-<id>`.
    control_id: String,
    value_name: String,
    null_name: String,
    /// The control of a refused unit save's draft gets `autofocus` (slice 1.5a).
    autofocus: bool,
    label: String,
    help: String,
    control: String,
    nullable: bool,
    value_state: String,
    value: String,
    options: Vec<(String, String)>,
    /// A derived (formula) field: shown read-only, never rendered as a form control.
    computed: bool,
    /// The computed value or error for a saved snapshot; `None` on create and re-render.
    derived: Option<DerivedView>,
    expr: String,
    rule_ids: String,
    /// The rows of a list field (plan D10); `None` for every other control.
    list: Option<ListView>,
    /// The field's current change note, when it has something to show (slice 1.2).
    note: Option<NoteLine>,
    /// The switch offer and Undo from that note (slice 1.3).
    actions: Option<Actions>,
}

/// At most this many characters of a value shown in a second, visible context (plan D7
/// RE3-1): the full value is echoed once, in its control.
const EXCERPT_CHARS: usize = 64;

/// At most this many characters of a scribble quoted under its cell (slice 1.5a): the
/// control right above echoes it in full, so the note only names it. Shorter than other
/// excerpts so a scalar's own form fits the per-unit page bound (plan D7).
const SCRIBBLE_EXCERPT_CHARS: usize = 32;

fn excerpt(value: &str) -> String {
    cut(value, EXCERPT_CHARS)
}

fn cut(value: &str, chars: usize) -> String {
    match value.char_indices().nth(chars) {
        Some((end, _)) => format!("{}…", &value[..end]),
        None => value.to_owned(),
    }
}

/// One declared property of a list field.
struct PropertyView {
    label: String,
    control: &'static str,
    optional: bool,
    options: Vec<(String, String)>,
}

struct CellView {
    p: usize,
    /// The DOM id suffix `<f>-<r>-<p>`.
    key: String,
    value: String,
    state: String,
    /// An integer cell: empty, it is unknown (slice 1.4).
    integer: bool,
    /// The cell's current change note, when it has something to show (slice 1.2).
    note: Option<NoteLine>,
    /// The switch offer and Undo from that note (slice 1.3).
    actions: Option<Actions>,
    /// The name of the value control in its own form (slice 1.5b): `value` in a cell's
    /// form, `row-<p>` in the new-row form.
    name: String,
    /// The value control of a refused cell save's or new row's draft gets `autofocus`, or
    /// for a new row whose first property is optional, its Set/Not set select (slice 1.5b).
    autofocus: bool,
    focus_set: bool,
}

/// What a saved, editable page offers under a cell from its current note (slice 1.3): the
/// D-Q switch ("Did you mean 84 − 15 = 69? Yes") and Undo of the cell's last change (D-N).
/// Each button belongs to one of the page's two small forms (`form` attribute) and submits
/// the note key as its own `note` pair, never the sheet form, so Enter in the sheet still
/// saves it. The buttons name their cell through `aria-labelledby`, so a cell's label is
/// never repeated and the page markup per cell stays bounded (plan D7).
struct Actions {
    /// The DOM id suffix of the cell (`hp`, `4-0-1`).
    key: String,
    /// The ids naming the cell: its label, and for a row cell the row legend.
    labelled: String,
    /// The note key, `<revision>:<address>`.
    note: String,
    offer: Option<Offer>,
    /// The value Undo restores, when Undo is offered.
    undo: Option<String>,
}

struct Offer {
    /// The relative reading ("84 − 15"), shown inside `<bdi>`.
    text: String,
    value: String,
}

/// Which of the page's shared action forms any cell or row uses: switch, Undo, and the row
/// actions (slice 1.5b).
#[derive(Default)]
struct AssistForms {
    switch: bool,
    undo: bool,
    rows: bool,
}

impl Actions {
    /// The offer and Undo of `address` from its current note, on a saved page the reader may
    /// edit; `None` when the note offers neither.
    ///
    /// A row cell passes its known location and indices (`located`), so rendering never
    /// searches rows, and its buttons submit the short indexed note key (slice 1.5b, D7).
    fn of(
        view: &SheetView,
        address: &str,
        located: Option<(super::assist::Located<'_>, (usize, usize, usize))>,
        (key, labelled): (String, String),
        forms: &mut AssistForms,
    ) -> Option<Self> {
        if !view.editable || view.snapshot.sheet_id == 0 {
            return None;
        }
        let pin = view.immutable.as_ref()?;
        let stored = view.notes.current.get(address)?;
        let (located, indices) = match located {
            Some((located, indices)) => (located, Some(indices)),
            None => (
                super::assist::locate(pin.definition(), &view.snapshot.data, address)?,
                None,
            ),
        };
        let current = super::assist::value_at(&located, &view.snapshot.data);
        let offer = super::assist::switch_value(stored, current.as_ref()).and_then(|value| {
            let Some(super::notes::NoteValue::Int(before)) = stored.note.before else {
                return None;
            };
            Some(Offer {
                text: super::entry::offer_text(before, &stored.note.raw)?,
                value: value.to_string(),
            })
        });
        let undo = super::assist::undoable(stored, current.as_ref()).then(|| {
            stored
                .note
                .before
                .as_ref()
                .map_or_else(String::new, super::notes::NoteValue::text)
        });
        forms.switch |= offer.is_some();
        forms.undo |= undo.is_some();
        (offer.is_some() || undo.is_some()).then(|| Self {
            key,
            labelled,
            note: indices.map_or_else(
                || super::assist::key(stored),
                |indices| super::assist::indexed_key(stored.revision, indices),
            ),
            offer,
            undo,
        })
    }
}

/// The ids an input is described by: its note line and its switch offer, or, for an empty
/// integer control (an unknown value, slice 1.4), the page's one "Unknown value" hint, so the
/// `placeholder="?"` is not its only cue.
fn described_by(
    key: &str,
    note: Option<&NoteLine>,
    actions: Option<&Actions>,
    unknown: bool,
) -> String {
    let mut ids = Vec::new();
    if unknown {
        ids.push("unknown-hint".to_owned());
    }
    if note.is_some() {
        ids.push(format!("note-{key}"));
    }
    if actions.is_some_and(|actions| actions.offer.is_some()) {
        ids.push(format!("offer-{key}"));
    }
    ids.join(" ")
}

/// What a sheet page shows next to a cell from its current change note (assisted sheet
/// slice 1.2): a sum ("84 − 15 = 69") or a scribble ("kept 84 · ‘lots’ not understood").
/// Other notes show nothing on the sheet; history lists them all.
struct NoteLine {
    /// `sum` or `unresolved`.
    kind: &'static str,
    /// The normalised sum, or the raw scribble (rendered escaped inside `<bdi>`).
    text: String,
    /// The value the note resulted in.
    value: String,
    /// The note left the cell unknown (`null`): the sheet shows "?" (slice 1.4).
    unknown: bool,
}

impl NoteLine {
    /// The line for `stored` when it describes the saved value `saved` of a cell.
    fn of(
        stored: &super::notes::Stored,
        control: crate::definitions::Control,
        saved: &serde_json::Value,
    ) -> Option<Self> {
        let note = &stored.note;
        if super::notes::NoteValue::of(control, saved).as_ref() != Some(&note.after) {
            return None;
        }
        let value = note.after.text();
        let unknown = note.after == super::notes::NoteValue::Unknown;
        match note.op {
            // A sign-led sum ("-5+3") is shown like a sum; a one-term literal shows nothing.
            super::notes::Op::Sum | super::notes::Op::SignLiteral => Some(Self {
                kind: "sum",
                text: super::entry::sum_text(&note.raw)?,
                value,
                unknown,
            }),
            // After a switch the cell shows the relative reading it applied: "84 − 15 = 69".
            super::notes::Op::Switched => {
                let super::notes::NoteValue::Int(after) = note.after else {
                    return None;
                };
                let literal = super::entry::read(&note.raw).ok()?.value;
                Some(Self {
                    kind: "sum",
                    text: super::entry::offer_text(after.checked_sub(literal)?, &note.raw)?,
                    value,
                    unknown,
                })
            }
            super::notes::Op::Unresolved => Some(Self {
                kind: "unresolved",
                text: note.raw.clone(),
                value,
                unknown,
            }),
            _ => None,
        }
    }
    /// A scribble keeps the saved value; its control shows what the player wrote.
    fn scribble(&self) -> Option<&str> {
        (self.kind == "unresolved").then_some(self.text.as_str())
    }
    /// The text under the cell: long expressions and scribbles are elided to an excerpt
    /// (the control echoes a scribble in full; history keeps every note's full text), so
    /// a cell's markup stays bounded (plan D7, slice 1.3 review).
    fn shown(&self) -> String {
        if self.kind == "unresolved" {
            cut(&self.text, SCRIBBLE_EXCERPT_CHARS)
        } else {
            self.text.clone()
        }
    }
}

/// One note of a history revision (D-E: the custodian and Masters see raw text).
struct NoteRow {
    address: String,
    op: &'static str,
    raw: String,
    before: String,
    after: String,
    /// The revision an Undo reverted.
    target: Option<i64>,
}

impl NoteRow {
    fn of(stored: &super::notes::Stored) -> Self {
        let note = &stored.note;
        Self {
            address: note.address.clone(),
            op: note.op.as_str(),
            raw: note.raw.clone(),
            before: note
                .before
                .as_ref()
                .map_or_else(|| "new".into(), super::notes::NoteValue::text),
            after: note.after.text(),
            target: note.target,
        }
    }
}

impl CellView {
    fn described_by(&self) -> String {
        described_by(
            &self.key,
            self.note.as_ref(),
            self.actions.as_ref(),
            self.integer && self.value.is_empty(),
        )
    }
    fn valid_boolean(&self) -> bool {
        matches!(self.value.as_str(), "" | "true" | "false")
    }
    fn valid_state(&self) -> bool {
        matches!(self.state.as_str(), "present" | "absent")
    }
    fn value_excerpt(&self) -> String {
        excerpt(&self.value)
    }
    fn state_excerpt(&self) -> String {
        excerpt(&self.state)
    }
}

struct RowView {
    r: usize,
    id: String,
    cells: Vec<CellView>,
    /// Which of this row's controls gets `autofocus` after a draft operation, if any.
    focus: RowFocus,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowFocus {
    None,
    Start,
    Up,
    Down,
}

impl RowView {
    fn number(&self) -> usize {
        self.r + 1
    }
    fn focus_start(&self, cell: &CellView) -> bool {
        self.focus == RowFocus::Start && cell.p == 0
    }
    fn focus_up(&self) -> bool {
        self.focus == RowFocus::Up
    }
    fn focus_down(&self) -> bool {
        self.focus == RowFocus::Down
    }
    fn id_excerpt(&self) -> String {
        excerpt(&self.id)
    }
}

/// A list field: its index `f` in the definition (the form key prefix), properties and rows.
struct ListView {
    f: usize,
    /// The sheet revision its controls were read at: a list draft's submitted revision, so a
    /// stale draft still conflicts, else the saved revision. Echoed once per form (plan D7).
    revision: String,
    properties: Vec<PropertyView>,
    rows: Vec<RowView>,
    /// The Add button gets `autofocus` (after Remove).
    focus_add: bool,
    /// A saved list on a page with a form per unit (slice 1.5b): each cell is its own form
    /// (`cells/{f}/{r}/{p}`), each row has Remove, Move up and Move down (page-level shared
    /// form), and a new-row form ends the list. Otherwise the rows belong to the full form.
    units: bool,
    /// The list's latest row removal, while its row is still away: "Row removed · Undo"
    /// (D-V), with the note key the Undo button submits.
    removed: Option<String>,
    /// The new-row form's controls (slice 1.5b); `None` when the list is full or not a
    /// unit list.
    new_row: Option<Vec<CellView>>,
    /// A refused draft that no control can take any more (review round 1): a cell whose row
    /// was removed, or a new row refused because the list is full. Shown read-only, to copy.
    kept: Option<Kept>,
}

/// A draft kept read-only on the page: why, then each property with its value.
struct Kept {
    message: &'static str,
    entries: Vec<KeptEntry>,
}

struct KeptEntry {
    label: String,
    /// An optional property's Set or Not set.
    state: Option<&'static str>,
    value: String,
}

impl ListView {
    fn new(
        f: usize,
        field: &crate::definitions::Field,
        draft: values::ListDraft,
        focus: Option<values::Focus>,
        saved: Option<(&serde_json::Value, &BTreeMap<String, super::notes::Stored>)>,
    ) -> Self {
        let row_focus = |r: usize| match focus {
            Some(values::Focus::RowStart(list, row)) if (list, row) == (f, r) => RowFocus::Start,
            Some(values::Focus::Up(list, row)) if (list, row) == (f, r) => RowFocus::Up,
            Some(values::Focus::Down(list, row)) if (list, row) == (f, r) => RowFocus::Down,
            _ => RowFocus::None,
        };
        let properties = values::properties(field)
            .iter()
            .map(|property| PropertyView {
                label: property.label.clone(),
                control: control_name(property.control),
                optional: property.optional,
                options: choices(property.options.as_ref()),
            })
            .collect();
        let declared = values::properties(field);
        // Notes are shown on a saved page only, never over a re-rendered draft.
        // A saved page lists the saved rows in order: row `r` is saved item `r`, checked by
        // its ID, so no row is searched for.
        let note = |r: usize, id: &str, p: usize| {
            let (saved, current) = saved?;
            let property = declared.get(p)?;
            let item = saved
                .as_array()?
                .get(r)
                .filter(|item| item["id"].as_str() == Some(id))?;
            let stored = current.get(&super::notes::cell_address(&field.id, id, &property.id))?;
            NoteLine::of(stored, property.control, item.get(&property.id)?)
        };
        let rows = draft
            .rows
            .into_iter()
            .enumerate()
            .map(|(r, row)| RowView {
                r,
                cells: row
                    .values
                    .into_iter()
                    .zip(row.states)
                    .enumerate()
                    .map(|(p, (value, state))| {
                        let note = note(r, &row.id, p);
                        let value = note
                            .as_ref()
                            .and_then(NoteLine::scribble)
                            .map_or(value, str::to_owned);
                        CellView {
                            p,
                            key: format!("{f}-{r}-{p}"),
                            value,
                            state,
                            integer: declared.get(p).is_some_and(|property| {
                                property.control == crate::definitions::Control::Integer
                            }),
                            note,
                            actions: None,
                            name: "value".into(),
                            autofocus: false,
                            focus_set: false,
                        }
                    })
                    .collect(),
                id: row.id,
                focus: row_focus(r),
            })
            .collect();
        Self {
            f,
            revision: String::new(),
            properties,
            rows,
            focus_add: focus == Some(values::Focus::Add(f)),
            units: false,
            removed: None,
            new_row: None,
            kept: None,
        }
    }
    /// Adds the switch and Undo of each saved row cell (slice 1.3).
    fn offer(
        &mut self,
        view: &SheetView,
        field: &crate::definitions::Field,
        forms: &mut AssistForms,
    ) {
        let properties = values::properties(field);
        let saved = &view.snapshot.data[&field.id];
        for row in &mut self.rows {
            // A saved page lists the saved rows in order; skip anything else.
            if saved[row.r]["id"].as_str() != Some(row.id.as_str()) {
                continue;
            }
            for cell in &mut row.cells {
                let address =
                    super::notes::cell_address(&field.id, &row.id, &properties[cell.p].id);
                // The cell label, the list legend and the row legend name the buttons.
                let labelled = format!(
                    "label-{} list-legend-{} legend-{}-{}",
                    cell.key, self.f, self.f, row.r
                );
                let located = super::assist::row(field, row.r, &properties[cell.p]);
                cell.actions = Actions::of(
                    view,
                    &address,
                    Some((located, (self.f, row.r, cell.p))),
                    (cell.key.clone(), labelled),
                    forms,
                );
            }
        }
    }
    /// A saved list on a page with a form per unit (slice 1.5b): a refused cell save's draft
    /// in its row (found by host ID), "Row removed · Undo" for the list's latest removal while
    /// its row is away (D-V), and the new-row form (with a refused new row's draft).
    fn units(
        &mut self,
        view: &SheetView,
        field: &crate::definitions::Field,
        draft: Option<&unit_web::UnitDraft>,
        forms: &mut AssistForms,
    ) {
        let properties = values::properties(field);
        self.units = true;
        forms.rows |= !self.rows.is_empty();
        if let Some((draft, p)) = draft.and_then(|draft| match draft.unit {
            super::unit::Unit::Cell { f, p, .. } if f == self.f => Some((draft, p)),
            _ => None,
        }) && let Some(cell) = self
            .rows
            .iter_mut()
            .find(|row| draft.host.as_deref() == Some(row.id.as_str()))
            .and_then(|row| row.cells.get_mut(p))
        {
            cell.value.clone_from(&draft.value);
            if properties[p].optional {
                cell.state = draft.null.clone().unwrap_or_default();
            }
            cell.note = None;
            cell.actions = None;
            cell.autofocus = true;
        } else if let Some((draft, p)) = draft.and_then(|draft| match draft.unit {
            super::unit::Unit::Cell { f, p, .. } if f == self.f && draft.host.is_some() => {
                Some((draft, p))
            }
            _ => None,
        }) {
            // Its row was removed: the draft is kept, never put into another row.
            self.kept = Some(Kept {
                message: "The row this entry was for has been removed, so it was not saved. \
                          Your entry is kept below to copy into another row.",
                entries: vec![KeptEntry {
                    label: properties[p].label.clone(),
                    state: properties[p]
                        .optional
                        .then(|| state_word(draft.null.as_deref())),
                    value: draft.value.clone(),
                }],
            });
        }
        if let Some(pin) = &view.immutable
            && let Some(stored) = view.notes.rows.get(&field.id)
        {
            let current =
                super::assist::locate(pin.definition(), &view.snapshot.data, &stored.note.address)
                    .and_then(|located| super::assist::value_at(&located, &view.snapshot.data));
            if super::assist::removal_undoable(stored, current.as_ref()) {
                self.removed = Some(super::assist::key(stored));
                forms.undo = true;
            }
        }
        let submitted = draft
            .filter(|draft| draft.unit == super::unit::Unit::NewRow(self.f))
            .and_then(|draft| draft.row.as_ref());
        if self.full() {
            // A new row refused because the list filled up: kept to copy, not addable.
            if let Some(row) = submitted {
                self.kept = Some(Kept {
                    message: "This list is full (256 rows), so your new row was not added. \
                              What you entered is kept below to copy; remove a row first.",
                    entries: properties
                        .iter()
                        .enumerate()
                        .map(|(p, property)| KeptEntry {
                            label: property.label.clone(),
                            state: property.optional.then(|| {
                                state_word(row.get(&format!("set-{p}")).map(String::as_str))
                            }),
                            value: row.get(&format!("row-{p}")).cloned().unwrap_or_default(),
                        })
                        .collect(),
                });
            }
            return;
        }
        let pair = |key: String| submitted.and_then(|row| row.get(&key)).cloned();
        self.new_row = Some(
            properties
                .iter()
                .enumerate()
                .map(|(p, property)| CellView {
                    p,
                    key: format!("new-{}-{p}", self.f),
                    value: pair(format!("row-{p}")).unwrap_or_default(),
                    state: if property.optional {
                        pair(format!("set-{p}")).unwrap_or_else(|| "absent".into())
                    } else {
                        String::new()
                    },
                    integer: property.control == crate::definitions::Control::Integer,
                    note: None,
                    actions: None,
                    name: format!("row-{p}"),
                    autofocus: submitted.is_some() && p == 0 && !property.optional,
                    focus_set: submitted.is_some() && p == 0 && property.optional,
                })
                .collect(),
        );
    }
    /// The last row: its Move down button is disabled.
    fn last(&self, row: &RowView) -> bool {
        row.r + 1 == self.rows.len()
    }
    /// At the row limit the Add button is disabled.
    fn full(&self) -> bool {
        self.rows.len() >= crate::definitions::formula::MAX_LIST_ITEMS
    }
    fn property(&self, cell: &CellView) -> &PropertyView {
        &self.properties[cell.p]
    }
    fn valid_option(&self, cell: &CellView) -> bool {
        self.properties[cell.p]
            .options
            .iter()
            .any(|option| option.0 == cell.value)
    }
}

/// How a kept optional property's Set/Not set state reads.
fn state_word(state: Option<&str>) -> &'static str {
    if state == Some("present") {
        "Set"
    } else {
        "Not set"
    }
}

fn control_name(control: crate::definitions::Control) -> &'static str {
    match control {
        crate::definitions::Control::Text => "text",
        crate::definitions::Control::Multiline => "multiline",
        crate::definitions::Control::Integer => "integer",
        crate::definitions::Control::Boolean => "boolean",
        crate::definitions::Control::Enumeration => "enum",
        crate::definitions::Control::List => "list",
    }
}

fn choices(options: Option<&Vec<crate::definitions::Choice>>) -> Vec<(String, String)> {
    options
        .map(|choices| {
            choices
                .iter()
                .map(|c| (c.id.clone(), c.label.clone()))
                .collect()
        })
        .unwrap_or_default()
}

impl FieldView {
    fn described_by(&self) -> String {
        described_by(
            &self.key,
            self.note.as_ref(),
            self.actions.as_ref(),
            self.control == "integer" && self.value.is_empty(),
        )
    }
    fn value_excerpt(&self) -> String {
        excerpt(&self.value)
    }
    fn state_excerpt(&self) -> String {
        excerpt(&self.value_state)
    }
    fn valid_boolean(&self) -> bool {
        matches!(self.value.as_str(), "" | "true" | "false")
    }
    fn valid_option(&self) -> bool {
        self.options.iter().any(|option| option.0 == self.value)
    }
}
struct GroupView {
    label: String,
    fields: Vec<FieldView>,
}

#[derive(Template)]
#[template(path = "private_sheet.html")]
struct PrivateTemplate<'a> {
    view: &'a SheetView,
    csrf: &'a str,
    action: String,
    draft: &'a crate::mork_form::SheetDraft,
    sheet: &'a crate::mork_borg::Sheet,
    groups: &'a [GroupView],
    message: &'a str,
    /// A `role="alert"` notice (an oversized draft that was not kept); empty otherwise.
    alert: &'a str,
    name: &'a str,
    notes: &'a str,
    revision: String,
    /// The submitted revision as shown in visible text: an excerpt, so the hidden control
    /// is its only full echo (RE3-1).
    revision_excerpt: String,
    native: bool,
    create: bool,
    pin: String,
    /// The definition declares a list field: the page gets a top Save button (plan D6).
    lists: bool,
    mode: FormulaMode,
    /// Evaluation of the saved snapshot; never of an unsaved draft.
    formulas: Option<FormulaView>,
    /// A history page's change notes, with raw text (D-E).
    revision_notes: Vec<NoteRow>,
    /// The switch and Undo forms this page needs (slice 1.3), outside the sheet form.
    assist_forms: AssistForms,
    /// Whether the page has a form per unit (slice 1.5a), and which metadata control a
    /// refused unit save focuses.
    unit_forms: UnitForms,
}

/// A saved runtime page the reader may edit has a form per unit (slice 1.5a): Name, General
/// notes and each scalar field save on their own, each list from its list form. Create,
/// native, read-only and history pages, and a refused full-form draft, keep the one full
/// form.
struct UnitForms {
    shown: bool,
    /// The metadata unit whose refused draft is shown, focused.
    focus: Option<super::unit::Unit>,
}

impl UnitForms {
    fn focus_name(&self) -> bool {
        self.focus == Some(super::unit::Unit::Name)
    }
    fn focus_notes(&self) -> bool {
        self.focus == Some(super::unit::Unit::Notes)
    }
}

/// How a runtime sheet shows its derived values.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FormulaMode {
    /// Native, unsupported or formula-free pin: no formula section.
    Absent,
    /// Create page: nothing saved yet, so nothing is evaluated.
    Create,
    /// The saved snapshot itself (view and history).
    Saved,
    /// A failed save re-renders the rejected draft beside the saved revision's results.
    Draft,
}

impl FormulaMode {
    fn of(view: &SheetView, create: bool, draft: bool) -> Self {
        match (
            view.immutable
                .as_ref()
                .is_some_and(|pin| pin.formulas().is_some()),
            create,
            draft,
        ) {
            (false, _, _) => Self::Absent,
            (true, true, _) => Self::Create,
            (true, false, false) => Self::Saved,
            (true, false, true) => Self::Draft,
        }
    }
    fn shown(self) -> bool {
        self != Self::Absent
    }
    fn draft(self) -> bool {
        self == Self::Draft
    }
}

fn native_draft(form: &BTreeMap<String, String>) -> crate::mork_form::SheetDraft {
    let get = |key: &str| form.get(key).cloned().unwrap_or_default();
    crate::mork_form::SheetDraft {
        name: get("name"),
        revision: get("revision"),
        agility: get("agility"),
        presence: get("presence"),
        strength: get("strength"),
        toughness: get("toughness"),
        hp: get("hp"),
        max_hp: get("max_hp"),
        worn_armor: get("worn_armor"),
        protection: get("protection"),
        standard_items: get("standard_items"),
        silver: get("silver"),
        omens: get("omens"),
        equipment: get("equipment"),
        description: get("description"),
        class_notes: get("class_notes"),
        powers: get("powers"),
        weapon_one: get("weapon_one"),
        weapon_two: get("weapon_two"),
        campaign_miseries: get("campaign_miseries"),
    }
}

fn render_private(
    view: &SheetView,
    csrf: &str,
    raw: Option<&BTreeMap<String, String>>,
    notice: &Notice,
) -> Result<String, AppError> {
    let native = matches!(view.snapshot.pin, Pin::Native { .. })
        && super::snapshots::supported(&view.snapshot);
    let sheet = if native {
        serde_json::from_value(view.snapshot.data.clone())?
    } else {
        crate::mork_borg::Sheet::default()
    };
    let draft = if let Some(raw) = raw {
        native_draft(raw)
    } else {
        crate::mork_form::SheetDraft::from_sheet(&view.snapshot.name, &sheet)
    };
    let create = view.snapshot.sheet_id == 0;
    // Evaluated once per response, of the saved snapshot only: never on create (`{}` is not
    // valid input) and never of a rejected draft.
    let formulas = if create {
        None
    } else {
        view.immutable.as_ref().and_then(|pin| {
            pin.evaluate(&view.snapshot.data).map(|result| {
                if let Ok(evaluation) = &result {
                    super::formulas::log_failures(
                        view.snapshot.sheet_id,
                        pin.raw.version,
                        evaluation,
                    );
                }
                super::formulas::present(pin, result.as_ref())
            })
        })
    };
    // A full-form draft (a refused full-form POST, or a slice-1.5a list form's from a page
    // loaded before slice 1.5b) is re-rendered in the full form, which can save it.
    let units = !create && !native && view.editable && view.immutable.is_some() && raw.is_none();
    let mut assist_forms = AssistForms::default();
    let unit_draft = notice.draft.as_ref();
    let groups = groups(
        view,
        (raw, unit_draft),
        formulas.as_ref(),
        (notice.focus, units),
        &mut assist_forms,
    )?;
    let name = shown_meta(view, (raw, unit_draft), super::unit::Unit::Name);
    let notes = shown_meta(view, (raw, unit_draft), super::unit::Unit::Notes);
    let revision = raw
        .and_then(|r| r.get("revision"))
        .cloned()
        .unwrap_or_else(|| view.snapshot.revision.to_string());
    let action = if create {
        let version = match view.snapshot.pin {
            Pin::Runtime { version_id, .. } => version_id,
            Pin::Native { .. } => 0,
        };
        format!(
            "/campaigns/{}/sheets/new?version={version}",
            view.snapshot.campaign_id
        )
    } else {
        format!("/private-sheets/{}", view.snapshot.sheet_id)
    };
    PrivateTemplate {
        view,
        csrf,
        action,
        draft: &draft,
        sheet: &sheet,
        groups: &groups,
        message: &notice.message,
        alert: &notice.alert,
        name,
        notes,
        revision_excerpt: excerpt(&revision),
        revision,
        native,
        create,
        pin: serde_json::to_string(&view.snapshot.pin)?,
        lists: view.immutable.as_ref().is_some_and(|pin| {
            pin.definition()
                .fields
                .iter()
                .any(|field| field.control == crate::definitions::Control::List)
        }),
        mode: FormulaMode::of(view, create, raw.is_some()),
        formulas,
        revision_notes: view.notes.revision.iter().map(NoteRow::of).collect(),
        assist_forms,
        unit_forms: UnitForms {
            shown: units,
            focus: unit_draft
                .map(|draft| draft.unit)
                .filter(|unit| !matches!(unit, super::unit::Unit::Field(_))),
        },
    }
    .render()
    .map_err(|_| AppError::Template)
}

/// The DOM id of a scalar field's control and the names of its value and Known/Unknown
/// controls: in its own form (slice 1.5a) `v<f>`, `value` and `null`; in the full form
/// `value-<id>`, `value-<id>` and `null-<id>`.
fn control_names((f, id): (usize, &str), units: bool) -> (String, String, String) {
    if units {
        (format!("v{f}"), "value".into(), "null".into())
    } else {
        (
            format!("value-{id}"),
            format!("value-{id}"),
            format!("null-{id}"),
        )
    }
}

/// The Name or General notes a page shows: a refused unit save's draft of it, a full-form
/// draft's, or the saved text.
fn shown_meta<'a>(
    view: &'a SheetView,
    (raw, draft): (
        Option<&'a BTreeMap<String, String>>,
        Option<&'a unit_web::UnitDraft>,
    ),
    unit: super::unit::Unit,
) -> &'a str {
    let (key, saved) = match unit {
        super::unit::Unit::Name => ("name", &view.snapshot.name),
        _ => ("notes", &view.snapshot.notes),
    };
    draft
        .filter(|draft| draft.unit == unit)
        .map(|draft| draft.value.as_str())
        .or_else(|| raw.and_then(|r| r.get(key)).map(String::as_str))
        .unwrap_or(saved.as_str())
}

/// The list view of list field `f`: a list draft (a full-form draft, or a list form's for this
/// list) or the saved rows with their notes, switch and Undo, and on a page with a form per
/// unit their cell forms, row actions and new-row form (slice 1.5b).
fn list_view(
    view: &SheetView,
    (f, field): (usize, &crate::definitions::Field),
    (raw, unit_draft): (
        Option<&BTreeMap<String, String>>,
        Option<&unit_web::UnitDraft>,
    ),
    (focus, units): (Option<values::Focus>, bool),
    forms: &mut AssistForms,
) -> ListView {
    let saved = &view.snapshot.data[&field.id];
    let list_raw = raw.filter(|raw| list_scope(raw).is_none_or(|scope| scope == f));
    let draft = list_raw.map_or_else(
        || values::ListDraft::saved(field, saved),
        |raw| values::ListDraft::submitted(f, field, raw),
    );
    let notes = list_raw.is_none().then_some((saved, &view.notes.current));
    let mut list = ListView::new(f, field, draft, focus, notes);
    list.revision = list_raw
        .and_then(|raw| raw.get("revision"))
        .cloned()
        .unwrap_or_else(|| view.snapshot.revision.to_string());
    // Switch and Undo on a saved list only, never over a re-rendered draft.
    if list_raw.is_none() {
        list.offer(view, field, forms);
        if units {
            list.units(view, field, unit_draft, forms);
        }
    }
    list
}

/// The groups of a runtime page. A full-form draft (`raw`) replaces every field it holds; a
/// list form's draft only its own list; a refused unit save's draft (slice 1.5a) only its
/// unit. Every other field shows its saved value with its note, switch and Undo.
fn groups(
    view: &SheetView,
    (raw, draft): (
        Option<&BTreeMap<String, String>>,
        Option<&unit_web::UnitDraft>,
    ),
    formulas: Option<&FormulaView>,
    (focus, units): (Option<values::Focus>, bool),
    forms: &mut AssistForms,
) -> Result<Vec<GroupView>, AppError> {
    let mut groups = Vec::new();
    if let Some(pin) = &view.immutable {
        for group in &pin.definition().groups {
            let mut fields = Vec::new();
            for id in &group.fields {
                let (f, field) = pin
                    .definition()
                    .fields
                    .iter()
                    .enumerate()
                    .find(|(_, field)| &field.id == id)
                    .ok_or(AppError::Invalid("Invalid pinned group."))?;
                let saved = &view.snapshot.data[id];
                let list = (field.control == crate::definitions::Control::List)
                    .then(|| list_view(view, (f, field), (raw, draft), (focus, units), forms));
                let unit_draft = draft.filter(|draft| draft.unit == super::unit::Unit::Field(f));
                let raw_value = raw.and_then(|r| r.get(&format!("value-{id}")));
                let drafted = unit_draft.is_some() || raw_value.is_some();
                let note = view
                    .notes
                    .current
                    .get(&super::notes::field_address(id))
                    .filter(|_| !drafted)
                    .and_then(|stored| NoteLine::of(stored, field.control, saved));
                let value = unit_draft
                    .map(|draft| draft.value.clone())
                    .or_else(|| raw_value.cloned())
                    .or_else(|| {
                        note.as_ref()
                            .and_then(NoteLine::scribble)
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| match saved {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Null => String::new(),
                        v => v.to_string(),
                    });
                let value_state = unit_draft
                    .and_then(|draft| draft.null.clone())
                    .or_else(|| raw.and_then(|r| r.get(&format!("null-{id}"))).cloned())
                    .unwrap_or_else(|| if saved.is_null() { "unknown" } else { "known" }.into());
                let (control_id, value_name, null_name) = control_names((f, id), units);
                fields.push(FieldView {
                    id: id.clone(),
                    key: f.to_string(),
                    control_id,
                    value_name,
                    null_name,
                    autofocus: unit_draft.is_some(),
                    label: field.label.clone(),
                    help: field.help.clone().unwrap_or_default(),
                    control: control_name(field.control).into(),
                    nullable: field.nullable,
                    value_state,
                    value,
                    options: choices(field.options.as_ref()),
                    computed: field.formula.is_some(),
                    derived: field
                        .formula
                        .as_ref()
                        .filter(|_| raw.is_none())
                        .and(formulas)
                        .and_then(|formulas| formulas.value(id))
                        .cloned(),
                    expr: field
                        .formula
                        .as_ref()
                        .map(|formula| formula.expr.clone())
                        .unwrap_or_default(),
                    rule_ids: field
                        .formula
                        .as_ref()
                        .map(|formula| super::formulas::rule_list(&formula.rule_ids))
                        .unwrap_or_default(),
                    list,
                    actions: if !drafted && field.formula.is_none() {
                        let labelled = format!("label-{f}");
                        Actions::of(
                            view,
                            &super::notes::field_address(id),
                            None,
                            (f.to_string(), labelled),
                            forms,
                        )
                    } else {
                        None
                    },
                    note,
                });
            }
            groups.push(GroupView {
                label: group.label.clone(),
                fields,
            });
        }
    }
    Ok(groups)
}

/// What a rendered sheet page announces: the `role="status"` message, an optional
/// `role="alert"`, the single `autofocus` target after a draft operation, and a refused unit
/// save's draft (slice 1.5a).
#[derive(Default)]
struct Notice {
    message: String,
    alert: String,
    focus: Option<values::Focus>,
    draft: Option<unit_web::UnitDraft>,
}

impl Notice {
    fn status(message: String) -> Self {
        Self {
            message,
            ..Self::default()
        }
    }
    fn focused(self, focus: values::Focus) -> Self {
        Self {
            focus: Some(focus),
            ..self
        }
    }
}

async fn rendered(
    view: SheetView,
    permit: OwnedSemaphorePermit,
    csrf: String,
    raw: Option<BTreeMap<String, String>>,
    status: StatusCode,
    notice: Notice,
) -> ResultResponse {
    let html = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        render_private(&view, &csrf, raw.as_ref(), &notice)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(error)?;
    let mut response = (status, Html(html)).into_response();
    if matches!(
        status,
        StatusCode::CONFLICT | StatusCode::UNPROCESSABLE_ENTITY
    ) {
        response.extensions_mut().insert(PrivateTrustedEditor);
    }
    Ok(response)
}

async fn new_sheet(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    Query(version): Query<Version>,
    Extension(admission): Extension<RuntimeAdmission>,
) -> ResultResponse {
    let permit = admission.take()?;
    let auth = auth::request_auth(&state, &session).await?;
    let (view, permit) = state
        .store()
        .private_create_context(auth, id, version.version, permit)
        .await
        .map_err(error)?;
    let csrf = auth::token(&session)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    rendered(view, permit, csrf, None, StatusCode::OK, Notice::default()).await
}

async fn read_sheet(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    Query(query): Query<SheetQuery>,
    Extension(admission): Extension<RuntimeAdmission>,
) -> ResultResponse {
    read_private(state, session, (id, None), query.focus(), admission).await
}
async fn read_history(
    State(state): State<AuthState>,
    session: Session,
    Path((id, revision)): Path<(i64, i64)>,
    Extension(admission): Extension<RuntimeAdmission>,
) -> ResultResponse {
    read_private(state, session, (id, Some(revision)), None, admission).await
}
async fn read_private(
    state: AuthState,
    session: Session,
    (id, history): (i64, Option<i64>),
    focus: Option<values::Focus>,
    admission: RuntimeAdmission,
) -> ResultResponse {
    let permit = admission.take()?;
    let auth = auth::request_auth(&state, &session).await?;
    let (view, permit) = state
        .store()
        .private_read(auth, id, history, permit)
        .await
        .map_err(error)?;
    let csrf = auth::token(&session)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let notice = Notice {
        focus,
        ..Notice::default()
    };
    rendered(view, permit, csrf, None, StatusCode::OK, notice).await
}

const NATIVE_KEYS: &[&str] = &[
    "csrf",
    "revision",
    "name",
    "notes",
    "agility",
    "presence",
    "strength",
    "toughness",
    "hp",
    "max_hp",
    "worn_armor",
    "protection",
    "standard_items",
    "silver",
    "omens",
    "equipment",
    "description",
    "class_notes",
    "powers",
    "weapon_one",
    "weapon_two",
    "campaign_miseries",
];

async fn create_sheet(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    Query(version): Query<Version>,
    Extension(admission): Extension<RuntimeAdmission>,
    body: axum::body::Bytes,
) -> ResultResponse {
    let permit = admission.take()?;
    save_private(state, session, id, Some(version.version), permit, body).await
}
/// The full-form and list-form save (slice 1.5a review, Sonnet M1): like a unit save it
/// parses its body and checks CSRF and the session (review round 2, Sol Minor 1), then
/// waits a short, bounded time for the runtime permit (D-X3), then answers the styled busy
/// page that keeps every submitted pair. The body is buffered and parsed outside the permit;
/// its size and pair count are capped (5 MiB, 65,536 pairs) and the auth layer admits at
/// most 8 requests at once.
async fn edit_sheet(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    body: axum::body::Bytes,
) -> ResultResponse {
    let pairs = parse_sheet_form(body).await?;
    auth::check_csrf(&session, pairs.csrf()?).await?;
    let auth = auth::request_auth(&state, &session).await?;
    let Some(permit) = crate::definitions::admit_within(unit_web::PERMIT_WAIT).await else {
        let csrf = auth::token(&session)
            .await?
            .ok_or(StatusCode::UNAUTHORIZED)?;
        return unit_web::busy_form(id, &pairs, &csrf);
    };
    save_parsed(&state, &session, (id, None), (auth, permit), pairs).await
}

/// Parses a sheet POST body (up to 65,536 pairs) off the async workers.
async fn parse_sheet_form(body: axum::body::Bytes) -> Result<super::transport::Pairs, StatusCode> {
    tokio::task::spawn_blocking(move || {
        super::transport::Pairs::parse_limited(&body, super::transport::SHEET_FORM_PAIRS)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
}

/// Closed-key admission of a sheet POST (plan D2). It is O(pairs) but may generate up to
/// 65,536 keys, so it is its own blocking step, whose only failure is 400 (never a
/// re-render).
async fn admit_form(
    view: SheetView,
    pairs: super::transport::Pairs,
    permit: OwnedSemaphorePermit,
) -> Result<(BTreeMap<String, String>, SheetView, OwnedSemaphorePermit), StatusCode> {
    tokio::task::spawn_blocking(move || {
        let keys = if let Some(pin) = &view.immutable {
            match pairs.value("list") {
                // A list form (slice 1.5a): one list's rows; every other value stays saved.
                // The create page has only the full form.
                Some(list) => values::index(list)
                    .filter(|_| view.snapshot.sheet_id != 0)
                    .and_then(|f| values::list_keys(pin.definition(), f, pairs.keys())),
                None => values::sheet_keys(pin.definition(), pairs.keys()),
            }
            .ok_or(StatusCode::BAD_REQUEST)?
        } else {
            let mut keys: std::collections::HashSet<String> =
                NATIVE_KEYS.iter().map(|s| (*s).into()).collect();
            if pairs.keys().any(|key| key == "op") {
                keys.insert("op".into());
            }
            keys
        };
        Ok::<_, StatusCode>((pairs.admit(&keys)?, view, permit))
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
}

/// A draft operation (plan D5): applies itself to the submitted draft and re-renders it with
/// 200, never validating or saving it. A non-integer revision is forged (400); Add at the row
/// limit answers 422 with the draft. A draft from an older revision gets a warning, and keeps
/// its submitted revision: nothing rebases it.
async fn draft_op(
    mut view: SheetView,
    permit: OwnedSemaphorePermit,
    csrf: String,
    mut form: BTreeMap<String, String>,
    op: values::Op,
) -> ResultResponse {
    let submitted = values::integer(&form["revision"]).map_err(|_| StatusCode::BAD_REQUEST)?;
    let (applied, view, form, permit) = tokio::task::spawn_blocking(move || {
        let applied = view
            .immutable
            .as_ref()
            .ok_or(AppError::Invalid("Draft operations need list fields."))
            .and_then(|pin| op.apply(pin.definition(), &mut form));
        if submitted != view.snapshot.revision {
            let stale = format!(
                "The saved sheet changed (revision {submitted} → {}); saving this draft will be refused with a conflict (409). Copy your changes, reload and re-apply them.",
                view.snapshot.revision
            );
            view.warning = if view.warning.is_empty() {
                stale
            } else {
                format!("{stale} {}", view.warning)
            };
        }
        (applied, view, form, permit)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match applied {
        Ok(applied) => {
            rendered(
                view,
                permit,
                csrf,
                Some(form),
                StatusCode::OK,
                Notice::status(applied.message).focused(applied.focus),
            )
            .await
        }
        Err(AppError::Invalid(reason)) => {
            rendered(
                view,
                permit,
                csrf,
                Some(form),
                StatusCode::UNPROCESSABLE_ENTITY,
                Notice::status(format!("{reason} Not saved yet.")),
            )
            .await
        }
        Err(failure) => Err(error(failure)),
    }
}

/// A list form's draft operation acts on its own list only (slice 1.5a).
fn op_in_scope(form: &BTreeMap<String, String>, op: values::Op) -> bool {
    match (list_scope(form), op) {
        (
            Some(scope),
            values::Op::Add(f)
            | values::Op::Remove(f, _)
            | values::Op::Up(f, _)
            | values::Op::Down(f, _),
        ) => scope == f,
        _ => true,
    }
}

/// The list field index of an admitted list form (slice 1.5a); `None` for a full form.
fn list_scope(form: &BTreeMap<String, String>) -> Option<usize> {
    form.get("list").and_then(|list| values::index(list))
}

/// Validates an admitted save form into `view.snapshot` and its change notes into
/// `view.notes.pending` (plan D4 steps 5 and 6; assisted sheet slice 1.2). Returns whether
/// anything changes: a save that changes no value and records no note is a no-op (D-U).
fn apply_save(view: &mut SheetView, form: &BTreeMap<String, String>) -> Result<bool, AppError> {
    // The revision check precedes value validation: a stale draft answers 409 even when its
    // values are invalid.
    let revision = values::integer(&form["revision"])?;
    if revision != view.snapshot.revision {
        return Err(AppError::Conflict);
    }
    let create = view.snapshot.sheet_id == 0;
    let scope = list_scope(form);
    // A list form keeps the saved Name and General notes (slice 1.5a).
    let (name, notes_text) = match scope {
        Some(_) => (view.snapshot.name.clone(), view.snapshot.notes.clone()),
        None => (
            super::name_notes(&form["name"], &form["notes"])?,
            form["notes"].clone(),
        ),
    };
    let (data, notes) = if let Some(pin) = &view.immutable {
        let (data, notes) = values::form(
            pin.definition(),
            form,
            (
                (!create).then_some(&view.snapshot.data),
                view.snapshot.revision,
            ),
            &view.notes.current,
            scope,
        )?;
        pin.validate_values(&data)?;
        if notes.len() > super::notes::MAX_NOTES {
            return Err(super::notes::too_many());
        }
        (data, notes)
    } else {
        (
            serde_json::to_value(native_draft(form).validate()?)?,
            Vec::new(),
        )
    };
    let changed = create
        || name != view.snapshot.name
        || notes_text != view.snapshot.notes
        || data != view.snapshot.data
        || !notes.is_empty();
    view.snapshot.name = name;
    view.snapshot.notes = notes_text;
    view.snapshot.data = data;
    view.notes.pending = notes;
    Ok(changed)
}

async fn save_private(
    state: AuthState,
    session: Session,
    id: i64,
    version: Option<i64>,
    permit: OwnedSemaphorePermit,
    body: axum::body::Bytes,
) -> ResultResponse {
    // The create save holds the permit while it parses (it is taken before the body).
    let pairs = parse_sheet_form(body).await?;
    // Precedence (plan D4): parse, then CSRF and authorization, then key admission, so a
    // bad key set never answers anything but 403 to a caller without a token or rights.
    auth::check_csrf(&session, pairs.csrf()?).await?;
    let auth = auth::request_auth(&state, &session).await?;
    save_parsed(&state, &session, (id, version), (auth, permit), pairs).await
}

/// A sheet save after parsing, CSRF and session auth: rights, key admission, then the
/// draft operation or the save.
async fn save_parsed(
    state: &AuthState,
    session: &Session,
    (id, version): (i64, Option<i64>),
    (auth, permit): (auth::RequestAuth, OwnedSemaphorePermit),
    pairs: super::transport::Pairs,
) -> ResultResponse {
    let (state, session) = (state.clone(), session.clone());
    let (view, permit) = load_editor(&state, auth.clone(), id, version, permit).await?;
    if !view.editable {
        return Err(StatusCode::FORBIDDEN);
    }
    let (form, mut view, permit) = admit_form(view, pairs, permit).await?;
    let op = values::Op::parse(
        form.get("op").map(String::as_str),
        view.immutable
            .as_ref()
            .map(crate::definitions::private_pin::ImmutablePin::definition),
        &form,
    )
    .ok_or(StatusCode::BAD_REQUEST)?;
    if !op_in_scope(&form, op) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let csrf = auth::token(&session)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if form.values().map(String::len).sum::<usize>() > ECHO_BYTES {
        // Never storable and never echoed: the saved revision is shown instead.
        return rendered(
            view,
            permit,
            csrf,
            None,
            StatusCode::UNPROCESSABLE_ENTITY,
            Notice {
                alert: "This draft is larger than the 1 MiB sheet limit and was not kept. \
                        Nothing was saved; copy large text before submitting."
                    .into(),
                ..Notice::default()
            },
        )
        .await;
    }
    if op != values::Op::Save {
        return draft_op(view, permit, csrf, form, op).await;
    }
    let (result, view, form, permit) = tokio::task::spawn_blocking(move || {
        let result = apply_save(&mut view, &form);
        (result, view, form, permit)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let changed = match result {
        Ok(changed) => changed,
        Err(failure) => {
            return invalid_draft(&state, auth, (id, version), permit, csrf, form, failure).await;
        }
    };
    if !changed {
        return unchanged(&state, auth, (id, version), view, permit, csrf, form).await;
    }
    match state
        .store()
        .private_save_keeping(auth.clone(), view, permit)
        .await
    {
        Ok((id, _permit)) => Ok(Redirect::to(&format!("/private-sheets/{id}")).into_response()),
        Err((failure @ (AppError::Conflict | AppError::Invalid(_)), permit)) => {
            #[cfg(test)]
            if let Some(hook) = AFTER_FAILED_SAVE.lock().unwrap().take() {
                hook();
            }
            let (view, permit) = rejected_editor(&state, auth, id, version, permit).await?;
            rendered(
                view,
                permit,
                csrf,
                Some(form),
                error(failure),
                Notice::status(
                    "No changes saved. Copy your draft, reload and reassess the submitted revision."
                        .into(),
                ),
            )
            .await
        }
        Err((failure, _)) => Err(error(failure)),
    }
}

/// Re-renders a draft whose values were refused before any database step (plan D4 step 6).
async fn invalid_draft(
    state: &AuthState,
    auth: auth::RequestAuth,
    (id, version): (i64, Option<i64>),
    permit: OwnedSemaphorePermit,
    csrf: String,
    form: BTreeMap<String, String>,
    failure: AppError,
) -> ResultResponse {
    let message = match &failure {
        AppError::Invalid(reason) => {
            format!("No changes saved: {reason} Check the full draft and its submitted revision.")
        }
        _ => "No changes saved. Check the full draft and its submitted revision.".into(),
    };
    let (view, permit) = rejected_editor(state, auth, id, version, Some(permit)).await?;
    rendered(
        view,
        permit,
        csrf,
        Some(form),
        error(failure),
        Notice::status(message),
    )
    .await
}

/// A save that changes nothing (D-U): nothing is written, but rights, the active state and
/// the revision are still rechecked in a transaction before "No change". A stale page gets
/// the usual 409 with its draft; lost rights get 403.
async fn unchanged(
    state: &AuthState,
    auth: auth::RequestAuth,
    (id, version): (i64, Option<i64>),
    view: SheetView,
    permit: OwnedSemaphorePermit,
    csrf: String,
    form: BTreeMap<String, String>,
) -> ResultResponse {
    match state
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
                Notice::status(
                    "No change: every value equals the saved sheet, so nothing was saved.".into(),
                ),
            )
            .await
        }
        Err(AppError::Conflict) => {
            let (view, permit) = rejected_editor(state, auth, id, version, Some(permit)).await?;
            rendered(
                view,
                permit,
                csrf,
                Some(form),
                StatusCode::CONFLICT,
                Notice::status(
                    "No changes saved. Copy your draft, reload and reassess the submitted revision."
                        .into(),
                ),
            )
            .await
        }
        Err(failure) => Err(error(failure)),
    }
}

/// Reloads the editor to re-render a rejected draft. The rejected save keeps its permit when
/// it has one, so a concurrent request cannot turn the re-render into a 503 that loses the
/// draft.
async fn rejected_editor(
    state: &AuthState,
    auth: auth::RequestAuth,
    id: i64,
    version: Option<i64>,
    permit: Option<OwnedSemaphorePermit>,
) -> Result<(SheetView, OwnedSemaphorePermit), StatusCode> {
    #[cfg(test)]
    REJECTED_EDITOR_KEPT_PERMIT
        .lock()
        .unwrap()
        .push(permit.is_some());
    // `None` only when the database step itself failed before returning the permit (for
    // example an authorization recheck reporting Conflict/Invalid). That rare path asks
    // for a new permit and may answer 503; the auth layer adds `Retry-After: 1` to every
    // 503 document, and the draft is not re-rendered.
    let permit = match permit {
        Some(permit) => permit,
        None => crate::definitions::admit().map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?,
    };
    // A second, accepted loss path: if edit rights were lost between the rejected save and
    // this reload (a departure, removal or custody change racing the save), the sheet is
    // read-only for this caller and the draft is not re-rendered; the answer is the plain
    // 403 access-denied document. Nothing was saved either way.
    let (view, permit) = load_editor(state, auth, id, version, permit).await?;
    if !view.editable {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok((view, permit))
}

async fn load_editor(
    state: &AuthState,
    auth: auth::RequestAuth,
    id: i64,
    version: Option<i64>,
    permit: OwnedSemaphorePermit,
) -> Result<(SheetView, OwnedSemaphorePermit), StatusCode> {
    if let Some(version) = version {
        state
            .store()
            .private_create_context(auth, id, version, permit)
            .await
            .map_err(error)
    } else {
        state
            .store()
            .private_read(auth, id, None, permit)
            .await
            .map_err(error)
    }
}
