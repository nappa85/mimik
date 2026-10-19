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

/// Whether each `rejected_editor` call received the rejected save's own permit (tests only).
#[cfg(test)]
pub(super) static REJECTED_EDITOR_KEPT_PERMIT: Mutex<Vec<bool>> = Mutex::new(Vec::new());

/// Runs once between a failed save and the re-render's permit request (tests only).
#[cfg(test)]
pub(super) static AFTER_FAILED_SAVE: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);

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

pub(crate) fn routes() -> Router<AuthState> {
    let sheets = Router::new()
        .route(
            "/campaigns/{id}/sheets/new",
            get(new_sheet).merge(sheet_post(create_sheet)),
        )
        .route(
            "/private-sheets/{id}",
            get(read_sheet).merge(sheet_post(edit_sheet)),
        )
        .route("/private-sheets/{id}/history/{revision}", get(read_history))
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

struct FieldView {
    id: String,
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
}

/// At most this many characters of a value shown in a second, visible context (plan D7
/// RE3-1): the full value is echoed once, in its control.
const EXCERPT_CHARS: usize = 64;

fn excerpt(value: &str) -> String {
    match value.char_indices().nth(EXCERPT_CHARS) {
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
    value: String,
    state: String,
}

impl CellView {
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
    properties: Vec<PropertyView>,
    rows: Vec<RowView>,
    /// The Add button gets `autofocus` (after Remove).
    focus_add: bool,
}

impl ListView {
    fn new(
        f: usize,
        field: &crate::definitions::Field,
        draft: values::ListDraft,
        focus: Option<values::Focus>,
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
        let rows = draft
            .rows
            .into_iter()
            .enumerate()
            .map(|(r, row)| RowView {
                r,
                id: row.id,
                cells: row
                    .values
                    .into_iter()
                    .zip(row.states)
                    .enumerate()
                    .map(|(p, (value, state))| CellView { p, value, state })
                    .collect(),
                focus: row_focus(r),
            })
            .collect();
        Self {
            f,
            properties,
            rows,
            focus_add: focus == Some(values::Focus::Add(f)),
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
    let groups = groups(view, raw, formulas.as_ref(), notice.focus)?;
    let name = raw
        .and_then(|r| r.get("name"))
        .map_or(view.snapshot.name.as_str(), String::as_str);
    let notes = raw
        .and_then(|r| r.get("notes"))
        .map_or(view.snapshot.notes.as_str(), String::as_str);
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
        mode: match (
            view.immutable
                .as_ref()
                .is_some_and(|pin| pin.formulas().is_some()),
            create,
            raw.is_some(),
        ) {
            (false, _, _) => FormulaMode::Absent,
            (true, true, _) => FormulaMode::Create,
            (true, false, false) => FormulaMode::Saved,
            (true, false, true) => FormulaMode::Draft,
        },
        formulas,
    }
    .render()
    .map_err(|_| AppError::Template)
}

fn groups(
    view: &SheetView,
    raw: Option<&BTreeMap<String, String>>,
    formulas: Option<&FormulaView>,
    focus: Option<values::Focus>,
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
                let list = (field.control == crate::definitions::Control::List).then(|| {
                    let draft = raw.map_or_else(
                        || values::ListDraft::saved(field, saved),
                        |raw| values::ListDraft::submitted(f, field, raw),
                    );
                    ListView::new(f, field, draft, focus)
                });
                let value = raw
                    .and_then(|r| r.get(&format!("value-{id}")))
                    .cloned()
                    .unwrap_or_else(|| match saved {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Null => String::new(),
                        v => v.to_string(),
                    });
                let value_state = raw
                    .and_then(|r| r.get(&format!("null-{id}")))
                    .cloned()
                    .unwrap_or_else(|| if saved.is_null() { "unknown" } else { "known" }.into());
                fields.push(FieldView {
                    id: id.clone(),
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
/// `role="alert"`, and the single `autofocus` target after a draft operation.
#[derive(Default)]
struct Notice {
    message: String,
    alert: String,
    focus: Option<values::Focus>,
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
    Extension(admission): Extension<RuntimeAdmission>,
) -> ResultResponse {
    read_private(state, session, id, None, admission).await
}
async fn read_history(
    State(state): State<AuthState>,
    session: Session,
    Path((id, revision)): Path<(i64, i64)>,
    Extension(admission): Extension<RuntimeAdmission>,
) -> ResultResponse {
    read_private(state, session, id, Some(revision), admission).await
}
async fn read_private(
    state: AuthState,
    session: Session,
    id: i64,
    history: Option<i64>,
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
    rendered(view, permit, csrf, None, StatusCode::OK, Notice::default()).await
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
    save_private(state, session, id, Some(version.version), admission, body).await
}
async fn edit_sheet(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    Extension(admission): Extension<RuntimeAdmission>,
    body: axum::body::Bytes,
) -> ResultResponse {
    save_private(state, session, id, None, admission, body).await
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
            values::sheet_keys(pin.definition(), pairs.keys()).ok_or(StatusCode::BAD_REQUEST)?
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

/// Validates an admitted save form into `view.snapshot` (plan D4 steps 5 and 6).
fn apply_save(view: &mut SheetView, form: &BTreeMap<String, String>) -> Result<(), AppError> {
    // The revision check precedes value validation: a stale draft answers 409 even when its
    // values are invalid.
    let revision = values::integer(&form["revision"])?;
    if revision != view.snapshot.revision {
        return Err(AppError::Conflict);
    }
    let name = super::name_notes(&form["name"], &form["notes"])?;
    let data = if let Some(pin) = &view.immutable {
        let data = values::form(
            pin.definition(),
            form,
            &view.snapshot.data,
            view.snapshot.revision,
        )?;
        pin.validate_values(&data)?;
        data
    } else {
        serde_json::to_value(native_draft(form).validate()?)?
    };
    view.snapshot.name = name;
    view.snapshot.notes.clone_from(&form["notes"]);
    view.snapshot.data = data;
    Ok(())
}

async fn save_private(
    state: AuthState,
    session: Session,
    id: i64,
    version: Option<i64>,
    admission: RuntimeAdmission,
    body: axum::body::Bytes,
) -> ResultResponse {
    let permit = admission.take()?;
    let (pairs, permit) = tokio::task::spawn_blocking(move || {
        Ok::<_, StatusCode>((
            super::transport::Pairs::parse_limited(&body, super::transport::SHEET_FORM_PAIRS)?,
            permit,
        ))
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;
    // Precedence (plan D4): parse, then CSRF and authorization, then key admission, so a
    // bad key set never answers anything but 403 to a caller without a token or rights.
    auth::check_csrf(&session, pairs.csrf()?).await?;
    let auth = auth::request_auth(&state, &session).await?;
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
    if let Err(failure) = result {
        let message = match &failure {
            AppError::Invalid(reason) => format!(
                "No changes saved: {reason} Check the full draft and its submitted revision."
            ),
            _ => "No changes saved. Check the full draft and its submitted revision.".into(),
        };
        let (view, permit) = rejected_editor(&state, auth, id, version, Some(permit)).await?;
        return rendered(
            view,
            permit,
            csrf,
            Some(form),
            error(failure),
            Notice::status(message),
        )
        .await;
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
