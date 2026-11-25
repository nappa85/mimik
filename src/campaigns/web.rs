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

/// Marks a sheet page or a sheet JSON answer (step 2a): `normalize` gives it [`SHEET_CSP`].
#[derive(Clone)]
pub(crate) struct SheetPage;

/// The policy of sheet pages: scripts and styles from this origin only, no inline style or
/// script, no plugins, no base or frame tricks, and Trusted Types with no policy, so no
/// string ever becomes markup or code.
pub(crate) const SHEET_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; \
    object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'; \
    require-trusted-types-for 'script'; trusted-types 'none'";

/// The sheet module (`assets/sheet-assist.js`, step 2a).
pub(crate) async fn script_asset() -> impl IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/javascript; charset=utf-8",
        )],
        include_str!("../../assets/sheet-assist.js"),
    )
}

/// The Master live-list module (`assets/master-live.js`, CAMP-14).
pub(crate) async fn master_script_asset() -> Response {
    let mut response = (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/javascript; charset=utf-8",
        )],
        include_str!("../../assets/master-live.js"),
    )
        .into_response();
    response.extensions_mut().insert(SheetPage);
    response
}

/// The one-click acknowledgement module of a Master's sheet page (`assets/sheet-seen.js`).
pub(crate) async fn seen_script_asset() -> Response {
    let mut response = (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/javascript; charset=utf-8",
        )],
        include_str!("../../assets/sheet-seen.js"),
    )
        .into_response();
    response.extensions_mut().insert(SheetPage);
    response
}

/// Marks a unit save's styled busy page (503, D-X3): the auth layer keeps its body, which
/// holds the player's entry.
#[derive(Clone)]
pub(crate) struct SheetBusy;

/// Whether the auth layer keeps this response's body as a sheet's busy page.
pub(crate) fn sheet_busy(response: &Response) -> bool {
    response.status() == StatusCode::SERVICE_UNAVAILABLE
        && response.extensions().get::<SheetBusy>().is_some()
}

/// Marks an interpretation route's JSON answer (step 2c-1): fixed messages for "ask again"
/// (404), a conflict (409), busy (429) and unavailable (503), which the sheet module shows.
/// The auth layer keeps such a body instead of its generic one.
#[derive(Clone)]
pub(crate) struct AssistantAnswer;

pub(crate) fn assistant_answer(response: &Response) -> bool {
    matches!(
        response.status(),
        StatusCode::NOT_FOUND
            | StatusCode::CONFLICT
            | StatusCode::TOO_MANY_REQUESTS
            | StatusCode::SERVICE_UNAVAILABLE
    ) && response.extensions().get::<AssistantAnswer>().is_some()
        && response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .is_some_and(|value| value == "application/json")
}

/// Marks the suggest route's JSON answer (step 4): `unavailable` (403), `busy` (429, 503)
/// and the suggestions. The auth layer keeps such a body instead of its generic one.
#[derive(Clone)]
pub(crate) struct SuggestAnswer;

/// Whether the auth layer keeps this response's body as a suggest answer.
pub(crate) fn suggest_answer(response: &Response) -> bool {
    matches!(
        response.status(),
        StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE
    ) && response.extensions().get::<SuggestAnswer>().is_some()
        && response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .is_some_and(|value| value == "application/json")
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
#[path = "interpret_web.rs"]
pub(in crate::campaigns) mod interpret_web;
#[path = "remind_web.rs"]
mod remind_web;
#[path = "sheet_json.rs"]
mod sheet_json;
#[path = "suggest_web.rs"]
pub(in crate::campaigns) mod suggest_web;
#[path = "unit_web.rs"]
mod unit_web;
#[path = "which_web.rs"]
pub(in crate::campaigns) mod which_web;

/// The campaign and sheet routes; `assistant` adds the interpretation routes (step 2c-1),
/// which do not exist otherwise.
pub(crate) fn routes(assistant: bool) -> Router<AuthState> {
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
        .route(
            "/private-sheets/{id}/cells/{f}/{r}/{p}/which",
            get(which_web::which),
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
        // The quick-add line (slice 1.6): three short pairs.
        .route(
            "/private-sheets/{id}/lists/{f}/quick",
            post(unit_web::quick)
                .layer(DefaultBodyLimit::max(unit_web::ROW_POST_BYTES))
                .route_layer(middleware::from_fn(signed_in)),
        )
        // The full-form and list-form save waits the same way (review round 1, Sonnet M1).
        .route("/private-sheets/{id}", sheet_post(edit_sheet));
    let units = if assistant {
        units
            .route(
                "/private-sheets/{id}/interpretations",
                post(interpret_web::start)
                    .layer(DefaultBodyLimit::max(interpret_web::INTERPRET_POST_BYTES))
                    .route_layer(middleware::from_fn(signed_in)),
            )
            .route(
                "/private-sheets/{id}/interpretations/{job}",
                get(interpret_web::poll),
            )
            .route(
                "/private-sheets/{id}/reminders",
                post(remind_web::start)
                    .layer(DefaultBodyLimit::max(interpret_web::INTERPRET_POST_BYTES))
                    .route_layer(middleware::from_fn(signed_in)),
            )
            .route(
                "/private-sheets/{id}/proposals/{job}/{n}",
                post(interpret_web::apply)
                    .layer(DefaultBodyLimit::max(interpret_web::INTERPRET_POST_BYTES))
                    .route_layer(middleware::from_fn(signed_in)),
            )
    } else {
        // Off: the paths exist only to answer 404 (the router's fallback is 403); no
        // assistant code is reachable.
        let off = || async { StatusCode::NOT_FOUND };
        units
            .route("/private-sheets/{id}/interpretations", post(off))
            .route("/private-sheets/{id}/interpretations/{job}", get(off))
            .route("/private-sheets/{id}/reminders", post(off))
            .route("/private-sheets/{id}/proposals/{job}/{n}", post(off))
    };
    Router::new()
        .route("/campaigns", get(index).post(create))
        .route("/campaigns/{id}", get(detail))
        .route("/campaigns/{id}/join", post(join))
        .route("/campaigns/{id}/requests/{target}", post(decide))
        .route("/campaigns/{id}/members/{target}", post(member))
        .route("/private-sheets", get(personal))
        .route("/private-sheets/{id}/history", get(history))
        .layer(DefaultBodyLimit::max(8192))
        .merge(master_routes())
        .merge(sheets)
        .merge(units)
        .merge(suggest_route())
}

/// The Master workspace page and the click-only acknowledgement of an opened revision: outside
/// the sheet router's `runtime` layer (neither reads a sheet document), with the small body limit.
fn master_routes() -> Router<AuthState> {
    Router::new()
        .route("/campaigns/{id}/master", get(master))
        .route("/campaigns/{id}/master/snapshot", get(master_snapshot))
        .route("/campaigns/{id}/master/events", get(master_events))
        .route("/private-sheets/{id}/seen", post(seen))
        .layer(DefaultBodyLimit::max(8192))
}

/// The suggest route (step 4): JSON, inside `signed_in`, outside the `runtime` layer (its own
/// pool of 4; a pin-facts miss takes the runtime permit itself).
fn suggest_route() -> Router<AuthState> {
    Router::new().route(
        "/private-sheets/{id}/suggest/{f}/{p}",
        get(suggest_web::suggest).route_layer(middleware::from_fn(signed_in)),
    )
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
    nav: crate::auth::nav::Nav,
    workspace: &'a Workspace,
    csrf: &'a str,
}

#[derive(Template)]
#[template(path = "campaign.html")]
struct DetailTemplate<'a> {
    nav: crate::auth::nav::Nav,
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
            nav: crate::auth::nav::Nav::here(crate::auth::nav::Section::Campaigns),
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
            nav: crate::auth::nav::Nav::here(crate::auth::nav::Section::Campaigns),
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

#[derive(Template)]
#[template(path = "master.html")]
struct MasterTemplate<'a> {
    nav: crate::auth::nav::Nav,
    workspace: &'a super::master::MasterWorkspace,
    page: super::master::MasterPage,
}

/// The Master workspace (plain HTML): player and NPC sheets with this Master's seen state.
/// Everyone but a current Master of the campaign gets the uniform 403.
async fn master(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    Query(page): Query<super::master::MasterPage>,
) -> ResultResponse {
    if !page.valid() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let auth = auth::request_auth(&state, &session).await?;
    let workspace = state
        .store()
        .master_workspace(auth, id, page)
        .await
        .map_err(error)?;
    let mut response = Html(
        MasterTemplate {
            nav: crate::auth::nav::Nav::here_strict(crate::auth::nav::Section::Campaigns),
            workspace: &workspace,
            page,
        }
        .render()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    )
    .into_response();
    // The page runs the live-list module under the strict sheet policy.
    response.extensions_mut().insert(SheetPage);
    Ok(response)
}

/// The query of the snapshot probe: the page cursors of the workspace and, optionally, the
/// public token a stream issued to this tab (its first `status` event).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotQuery {
    #[serde(default)]
    players_after: i64,
    #[serde(default)]
    npcs_after: i64,
    #[serde(default)]
    stream: Option<String>,
}

/// The header of the probe's answer that tells whether the tab's stream (named by `stream`) is
/// `live`, was `replaced` by a newer tab of the same Master, or is `unknown`.
const STREAM_STATE_HEADER: &str = "x-master-stream";

/// The finite JSON projection of the workspace's selected pages (the probe of the live list):
/// the same cursors, query and uniform 403 as the page, admitted read-only (no session write
/// of any kind) and rechecked in the projection's own read transaction. A row or body over its
/// limit is a visible 500, never a truncated answer. With a `stream` token it also reports,
/// only for the requesting Master's own streams, what became of that tab's stream, so a
/// displaced tab does not reclaim the slot.
async fn master_snapshot(
    State(state): State<AuthState>,
    headers: axum::http::HeaderMap,
    axum::Extension(expected): axum::Extension<crate::network::ExpectedOrigin>,
    Path(id): Path<i64>,
    Query(query): Query<SnapshotQuery>,
) -> ResultResponse {
    let page = super::master::MasterPage {
        players: query.players_after,
        npcs: query.npcs_after,
    };
    if !page.valid()
        || query
            .stream
            .as_ref()
            .is_some_and(|token| token.is_empty() || token.len() > 64)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let auth = auth::read_only_request_auth(&state, &headers, &expected).await?;
    let account = auth.account_id();
    let store = state.store();
    let workspace = store
        .master_workspace(auth, id, page)
        .await
        .map_err(error)?;
    let body = workspace
        .snapshot_json()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut response = (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response();
    if let Some(token) = &query.stream {
        let state = match store.streams().token_state(account, token) {
            crate::streams::TokenState::Live => axum::http::HeaderValue::from_static("live"),
            crate::streams::TokenState::Replaced => {
                axum::http::HeaderValue::from_static("replaced")
            }
            crate::streams::TokenState::Unknown => axum::http::HeaderValue::from_static("unknown"),
        };
        response.headers_mut().insert(STREAM_STATE_HEADER, state);
    }
    response.extensions_mut().insert(SheetPage);
    Ok(response)
}

/// The live Master list: a server-sent event stream of the same snapshots (see
/// `master_stream`), admitted read-only like the probe, with the same cursors. The handshake
/// finishes inside the request boundary; the response body is then lazy.
async fn master_events(
    State(state): State<AuthState>,
    headers: axum::http::HeaderMap,
    axum::Extension(expected): axum::Extension<crate::network::ExpectedOrigin>,
    Path(id): Path<i64>,
    Query(page): Query<super::master::MasterPage>,
) -> ResultResponse {
    if !page.valid() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let auth = auth::read_only_request_auth(&state, &headers, &expected).await?;
    let mut response = super::master_stream::open(state.store(), auth, id, page).await?;
    response.extensions_mut().insert(SheetPage);
    Ok(response)
}

/// Records that the current Master opened the rendered revision: CSRF, then the session, then
/// the closed key set. Answers 303 to the sheet, never a sheet document, and writes no sheet
/// revision.
async fn seen(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    body: axum::body::Bytes,
) -> ResultResponse {
    let pairs = super::transport::Pairs::parse(&body)?;
    auth::check_csrf(&session, pairs.csrf()?).await?;
    let auth = auth::request_auth(&state, &session).await?;
    let form = pairs.admit(&["csrf", "revision"].into_iter().collect())?;
    let revision = form["revision"]
        .parse::<i64>()
        .ok()
        .filter(|revision| *revision > 0)
        .ok_or(StatusCode::BAD_REQUEST)?;
    state
        .store()
        .acknowledge_seen(auth, id, revision)
        .await
        .map_err(error)?;
    Ok(Redirect::to(&format!("/private-sheets/{id}")).into_response())
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
    nav: crate::auth::nav::Nav,
    sheets: &'a [SheetSummary],
    next: Option<i64>,
}
#[derive(Template)]
#[template(path = "private_history.html")]
struct HistoryTemplate<'a> {
    nav: crate::auth::nav::Nav,
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
            nav: crate::auth::nav::Nav::here(crate::auth::nav::Section::Sheets),
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
            nav: crate::auth::nav::Nav::here(crate::auth::nav::Section::Sheets),
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
    /// The Master's classification of the sheet to create (`1` for an NPC sheet): it travels on
    /// the create URL, never as a form key, so it survives every re-render of the create form.
    /// Anything but 0 or 1 is refused.
    #[serde(default)]
    npc: u8,
}

/// The create page's identity: the pinned definition version and the classification.
#[derive(Clone, Copy)]
struct CreateKey {
    version: i64,
    npc: bool,
}

impl Version {
    fn key(&self) -> Result<CreateKey, StatusCode> {
        match self.npc {
            0 | 1 => Ok(CreateKey {
                version: self.version,
                npc: self.npc == 1,
            }),
            _ => Err(StatusCode::BAD_REQUEST),
        }
    }
}

/// A sheet view's optional focus after a row move (slice 1.5b review), `up-<f>-<r>` or
/// `down-<f>-<r>`, or after a quick-add (slice 1.6), `quick-<f>`. Read from the raw query:
/// anything else, including a repeated `focus` and any other parameter, is ignored, never
/// refused (a focus is a convenience).
struct SheetQuery(Option<String>, Option<(i64, i64)>);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for SheetQuery {
    type Rejection = std::convert::Infallible;
    fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let query = parts.uri.query().unwrap_or_default();
        let mut focus = form_urlencoded::parse(query.as_bytes())
            .filter(|(key, _)| key == "focus")
            .map(|(_, value)| value.into_owned());
        let first = focus.next();
        // The Master list's cursors, kept only when each appears at most once and is a
        // nonnegative integer; anything else falls back to the list's first page.
        let cursor = |name: &str| {
            let mut found = form_urlencoded::parse(query.as_bytes())
                .filter(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned());
            let one = found.next();
            if found.next().is_some() {
                return None;
            }
            one.map_or(Some(0), |value| {
                value.parse::<i64>().ok().filter(|n| *n >= 0)
            })
        };
        let back = cursor("players_after").zip(cursor("npcs_after"));
        std::future::ready(Ok(Self(first.filter(|_| focus.next().is_none()), back)))
    }
}

impl SheetQuery {
    fn focus(&self) -> Option<values::Focus> {
        let (verb, rest) = self.0.as_deref()?.split_once('-')?;
        if verb == "quick" {
            return values::index(rest).map(values::Focus::Quick);
        }
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
    /// The list's catalogue key property (step 3a): never rendered as a control.
    key: bool,
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
    /// The catalogue entry this saved name cell links to (step 3a), if any.
    link: Option<super::catalogue::EntryLink>,
    /// The warning of a saved name cell with several or no catalogue match (step 4).
    matched: Option<MatchView>,
    /// The suggest route of an editable catalogue-backed name input (step 4); the sheet
    /// module turns the input into a combobox.
    suggest: Option<String>,
}

/// Rules 3 and 4 as the page words them (no name echoed): the sentence, the "Which one?" link
/// of an editor and the Browse link.
struct MatchView {
    kind: super::catalogue::Match,
    text: &'static str,
    which: Option<String>,
    browse: Option<String>,
}

impl MatchView {
    fn of(
        kind: super::catalogue::Match,
        (view, f, r, name): (&SheetView, usize, usize, usize),
        member: &crate::definitions::CatalogueMember,
    ) -> Self {
        use super::catalogue::Match;
        let sheet = view.snapshot.sheet_id;
        let browse = Some(which_web::browse_href(
            sheet,
            (&member.id, member.entry_type.as_deref()),
        ));
        match kind {
            Match::Several => Self {
                kind,
                text: "Several catalogue entries have this name.",
                which: view.editable.then(|| {
                    format!(
                        "/private-sheets/{sheet}/cells/{f}/{r}/{name}/which?base={}",
                        view.snapshot.revision
                    )
                }),
                browse: None,
            },
            Match::None => Self {
                kind,
                text: "Not in the catalogue.",
                which: None,
                browse,
            },
            Match::NoCurrent => Self {
                kind,
                text: "No current entry of this type has this name.",
                which: None,
                browse,
            },
        }
    }
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
    /// How many other fields that Undo reverts with this one (a bundle, step 3b); 0 for none.
    with: usize,
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
        // The bundle's other members, from the page's one bundle query.
        // A bundled note offers Undo only while every member can still be undone together
        // (their notes current, their cells holding their values), as a single Undo is
        // withheld once its cell changed; `with` counts the other members of an offered Undo.
        let mut undo = undo;
        let mut with = 0;
        if undo.is_some()
            && let Some(bundle) = stored.bundle
        {
            match view.notes.bundles.get(&bundle) {
                Some(members)
                    if members.len() <= super::notes::MAX_BUNDLE
                        && assist_web::changed_members(view, members).is_none() =>
                {
                    with = members.len().saturating_sub(1);
                }
                _ => undo = None,
            }
        }
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
            with,
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
    (unknown, matched): (bool, bool),
) -> String {
    let mut ids = Vec::new();
    if unknown {
        ids.push("unknown-hint".to_owned());
    }
    if matched {
        ids.push(format!("match-{key}"));
    }
    if note.is_some() {
        ids.push(format!("note-{key}"));
    }
    if actions.is_some_and(|actions| actions.offer.is_some()) {
        ids.push(format!("offer-{key}"));
    }
    ids.join(" ")
}

/// The suggest route of property `p` of list field `f` for an editor of a saved sheet whose
/// definition links the property's catalogue (step 4); `None` otherwise.
fn suggest_path(
    view: &SheetView,
    (f, p): (usize, usize),
    field: &crate::definitions::Field,
) -> Option<String> {
    let pin = view.immutable.as_ref()?;
    let property = values::properties(field).get(p)?;
    values::linked_member(pin.definition(), property)?;
    (view.editable && view.snapshot.sheet_id != 0)
        .then(|| format!("/private-sheets/{}/suggest/{f}/{p}", view.snapshot.sheet_id))
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
            (
                self.integer && self.value.is_empty(),
                self.matched.is_some(),
            ),
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
    /// After a quick-add the line gets `autofocus` (`?focus=quick-<f>`, slice 1.6).
    focus_quick: bool,
    /// The quick-add line (slice 1.6), when the definition declares one; `None` when the
    /// list is full or not a unit list.
    quick: Option<QuickLine>,
    /// A refused draft that no control can take any more (review round 1): a cell whose row
    /// was removed, or a new row refused because the list is full. Shown read-only, to copy.
    kept: Option<Kept>,
}

/// A list's quick-add line: its text (a refused entry's draft, else empty), whether it
/// takes `autofocus`, and whether the list maps a count (the hint promises only what is
/// read).
struct QuickLine {
    value: String,
    autofocus: bool,
    counted: bool,
}

impl QuickLine {
    /// A refused entry with a line break is shown in a textarea, which keeps the break (an
    /// `<input>` would strip it and a retry would post altered text; review round 1).
    fn multiline(&self) -> bool {
        self.value.contains('\n')
    }
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
        let key = values::key_property(field);
        let properties = values::properties(field)
            .iter()
            .enumerate()
            .map(|(p, property)| PropertyView {
                label: property.label.clone(),
                control: control_name(property.control),
                optional: property.optional,
                options: choices(property.options.as_ref()),
                key: key == Some(p),
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
                            link: None,
                            matched: None,
                            suggest: None,
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
            focus_quick: focus == Some(values::Focus::Quick(f)),
            quick: None,
            kept: None,
        }
    }
    /// Puts the catalogue entry link, or the warning of rules 3 and 4, on each saved row's name
    /// cell (steps 3a and 4), and the suggest route on an editor's name inputs.
    fn link(&mut self, view: &SheetView, f: usize, field: &crate::definitions::Field) {
        let Some(name) = values::name_property(field) else {
            return;
        };
        let Some(member) = values::properties(field)[name].catalogue.as_ref() else {
            return;
        };
        for row in &mut self.rows {
            let Some(cell) = row.cells.get_mut(name) else {
                continue;
            };
            let at = (f, row.id.clone());
            if let Some(link) = view.entry_links.get(&at) {
                cell.link = Some(link.clone());
            } else if let Some(kind) = view.entry_matches.get(&at) {
                cell.matched = Some(MatchView::of(*kind, (view, f, row.r, name), member));
            }
            cell.suggest = suggest_path(view, (f, name), field);
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
        self.quick(field, draft);
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
                    link: None,
                    matched: None,
                    suggest: suggest_path(view, (self.f, p), field),
                })
                .collect(),
        );
    }
    /// The quick-add line of a list that declares one (slice 1.6), with a refused entry's
    /// draft, focused; at 256 rows no line, and a refused entry is kept read-only to copy.
    fn quick(&mut self, field: &crate::definitions::Field, draft: Option<&unit_web::UnitDraft>) {
        let quick = draft.filter(|draft| draft.unit == super::unit::Unit::QuickAdd(self.f));
        if self.full() {
            if let Some(quick) = quick {
                self.kept = Some(Kept {
                    message: "This list is full (256 rows), so your quick-add entry was not \
                              added. It is kept below to copy; remove a row first.",
                    entries: vec![KeptEntry {
                        label: "Quick add".into(),
                        state: None,
                        value: quick.value.clone(),
                    }],
                });
            }
        } else if let Some(mapping) = field
            .items
            .as_ref()
            .and_then(|items| items.quick_add.as_ref())
        {
            self.quick = Some(QuickLine {
                value: quick.map(|quick| quick.value.clone()).unwrap_or_default(),
                autofocus: quick.is_some() || self.focus_quick,
                counted: mapping.quantity.is_some(),
            });
        }
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
            (self.control == "integer" && self.value.is_empty(), false),
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
    nav: crate::auth::nav::Nav,
    view: &'a SheetView,
    csrf: &'a str,
    /// The Master workspace link of a current Master's current page.
    master_link: Option<String>,
    /// The acknowledgement a plain current read offers a current Master.
    seen_block: Option<SeenBlock>,
    action: String,
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
    create: bool,
    pin: String,
    /// The definition declares a list field: the page gets a top Save button (plan D6).
    lists: bool,
    mode: FormulaMode,
    /// Evaluation of the saved snapshot; never of an unsaved draft.
    formulas: Option<FormulaView>,
    /// The saved revision's validity flag (RR-3); `None` on the create page.
    validity: Option<super::formulas::Validity>,
    /// A history page's change notes, with raw text (D-E).
    revision_notes: Vec<NoteRow>,
    /// The switch and Undo forms this page needs (slice 1.3), outside the sheet form.
    assist_forms: AssistForms,
    /// Whether the page has a form per unit (slice 1.5a), and which metadata control a
    /// refused unit save focuses.
    unit_forms: UnitForms,
}

/// What the seen block of a Master's page shows: "Opened (revision N)" or the one-click form
/// carrying the exact rendered revision.
struct SeenBlock {
    opened: Option<i64>,
    revision: i64,
}

/// A saved runtime page the reader may edit has a form per unit (slice 1.5a): Name, General
/// notes and each scalar field save on their own, each list from its list form. Create,
/// read-only and history pages, and a refused full-form draft, keep the one full
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
    /// Unsupported or formula-free pin: no formula section.
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

fn render_private(
    nav: crate::auth::nav::Nav,
    view: &SheetView,
    csrf: &str,
    raw: Option<&BTreeMap<String, String>>,
    notice: &Notice,
) -> Result<String, AppError> {
    let create = view.snapshot.sheet_id == 0;
    let formulas = evaluated(view);
    // A full-form draft (a refused full-form POST, or a slice-1.5a list form's from a page
    // loaded before slice 1.5b) is re-rendered in the full form, which can save it.
    let units = !create && view.editable && view.immutable.is_some() && raw.is_none();
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
        let Pin::Runtime {
            version_id: version,
            ..
        } = view.snapshot.pin;
        // The classification travels on the create URL, never as a form key.
        format!(
            "/campaigns/{}/sheets/new?version={version}{}",
            view.snapshot.campaign_id,
            if view.npc { "&npc=1" } else { "" }
        )
    } else {
        format!("/private-sheets/{}", view.snapshot.sheet_id)
    };
    // A current Master's page (never a player's, never a history page) links back to the
    // workspace and may offer the acknowledgement of the rendered revision.
    let master_link = (!create && view.seen.is_some()).then(|| match notice.back {
        Some((players, npcs)) if players > 0 || npcs > 0 => format!(
            "/campaigns/{}/master?players_after={players}&npcs_after={npcs}",
            view.snapshot.campaign_id
        ),
        _ => format!("/campaigns/{}/master", view.snapshot.campaign_id),
    });
    let seen_block = notice
        .offer_seen
        .then_some(view.seen)
        .flatten()
        .map(|seen| SeenBlock {
            opened: match seen {
                super::sheets::Seen::Opened(revision) => Some(revision),
                super::sheets::Seen::Offer => None,
            },
            revision: view.snapshot.revision,
        });
    PrivateTemplate {
        nav,
        view,
        csrf,
        master_link,
        seen_block,
        action,
        groups: &groups,
        message: &notice.message,
        alert: &notice.alert,
        name,
        notes,
        revision_excerpt: excerpt(&revision),
        revision,
        create,
        pin: serde_json::to_string(&view.snapshot.pin)?,
        lists: view.immutable.as_ref().is_some_and(|pin| {
            pin.definition()
                .fields
                .iter()
                .any(|field| field.control == crate::definitions::Control::List)
        }),
        mode: FormulaMode::of(view, create, raw.is_some()),
        // Of the saved revision, from the same evaluation; never of a draft (RR-3).
        validity: (!create)
            .then(|| super::formulas::Validity::of(formulas.as_ref(), view.immutable.is_some())),
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

/// The evaluation of the saved snapshot, once per response: never on create (`{}` is not
/// valid input) and never of a rejected draft; `None` without a formula layer.
fn evaluated(view: &SheetView) -> Option<FormulaView> {
    if view.snapshot.sheet_id == 0 {
        return None;
    }
    view.immutable.as_ref().and_then(|pin| {
        pin.evaluate(&view.snapshot.data).map(|result| {
            if let Ok(evaluation) = &result {
                super::formulas::log_failures(view.snapshot.sheet_id, pin.raw.version, evaluation);
            }
            super::formulas::present(pin, result.as_ref())
        })
    })
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
        list.link(view, f, field);
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
    /// A plain current read (status 200, no history) of a current Master's page offers the
    /// acknowledgement; every other render (refused drafts, busy, history) never does.
    offer_seen: bool,
    /// Validated Master workspace cursors of the list page this sheet was opened from.
    back: Option<(i64, i64)>,
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
    // The blocking pool does not carry the request's task-local account context.
    let nav = crate::auth::nav::Nav::here_strict(crate::auth::nav::Section::Sheets);
    let html = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        render_private(nav, &view, &csrf, raw.as_ref(), &notice)
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
    response.extensions_mut().insert(SheetPage);
    Ok(response)
}

async fn new_sheet(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    Query(version): Query<Version>,
    Extension(admission): Extension<RuntimeAdmission>,
) -> ResultResponse {
    let key = version.key()?;
    let permit = admission.take()?;
    let auth = auth::request_auth(&state, &session).await?;
    let (view, permit) = state
        .store()
        .private_create_context_as(auth, id, (key.version, key.npc), permit)
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
    query: SheetQuery,
    Extension(admission): Extension<RuntimeAdmission>,
) -> ResultResponse {
    read_private(
        state,
        session,
        (id, None),
        (query.focus(), query.1),
        admission,
    )
    .await
}
async fn read_history(
    State(state): State<AuthState>,
    session: Session,
    Path((id, revision)): Path<(i64, i64)>,
    Extension(admission): Extension<RuntimeAdmission>,
) -> ResultResponse {
    read_private(
        state,
        session,
        (id, Some(revision)),
        (None, None),
        admission,
    )
    .await
}
async fn read_private(
    state: AuthState,
    session: Session,
    (id, history): (i64, Option<i64>),
    (focus, back): (Option<values::Focus>, Option<(i64, i64)>),
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
        offer_seen: history.is_none(),
        back: back.filter(|_| history.is_none()),
        ..Notice::default()
    };
    rendered(view, permit, csrf, None, StatusCode::OK, notice).await
}

async fn create_sheet(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    Query(version): Query<Version>,
    Extension(admission): Extension<RuntimeAdmission>,
    body: axum::body::Bytes,
) -> ResultResponse {
    let permit = admission.take()?;
    save_private(state, session, id, Some(version.key()?), permit, body).await
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
pub(super) async fn admit_form(
    view: SheetView,
    pairs: super::transport::Pairs,
    permit: OwnedSemaphorePermit,
) -> Result<(BTreeMap<String, String>, SheetView, OwnedSemaphorePermit), StatusCode> {
    tokio::task::spawn_blocking(move || {
        // A sheet without an admitted immutable pin (an unsupported one) admits no form at all.
        let pin = view.immutable.as_ref().ok_or(StatusCode::BAD_REQUEST)?;
        let keys = match pairs.value("list") {
            // A list form (slice 1.5a): one list's rows; every other value stays saved.
            // The create page has only the full form.
            Some(list) => values::index(list)
                .filter(|_| view.snapshot.sheet_id != 0)
                .and_then(|f| values::list_keys(pin.definition(), f, pairs.keys())),
            None => values::sheet_keys(pin.definition(), pairs.keys()),
        }
        .ok_or(StatusCode::BAD_REQUEST)?;
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
    let pin = view.immutable.as_ref().ok_or(AppError::Invalid(
        "Unsupported interpretation pin is read-only.",
    ))?;
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
    version: Option<CreateKey>,
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
    (id, version): (i64, Option<CreateKey>),
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
    // The full form and the list form are the only saves whose base is the page's own rendered
    // revision (checked by `apply_save` and again in the save transaction).
    view.ack = super::sheets::Ack::Witnessed;
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
    (id, version): (i64, Option<CreateKey>),
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
    (id, version): (i64, Option<CreateKey>),
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
    version: Option<CreateKey>,
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
    version: Option<CreateKey>,
    permit: OwnedSemaphorePermit,
) -> Result<(SheetView, OwnedSemaphorePermit), StatusCode> {
    if let Some(key) = version {
        state
            .store()
            .private_create_context_as(auth, id, (key.version, key.npc), permit)
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
