//! Owner-only source comparison and explicitly attributed review forms.
use super::{AuthResult, AuthState, csrf_admission, owner_auth};
use crate::sources::shapes::Shape;
use crate::{
    AppError,
    sources::{
        PageReview, PageView, UnitReview,
        materials::ReviewNext,
        materials::{Material, MaterialPage},
    },
};
use askama::Template;
use axum::{
    Router,
    extract::{DefaultBodyLimit, Form, Path, Query, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse, Redirect},
    routing::{get, post},
};
use serde::Deserialize;
use tower_sessions::Session;

#[derive(Clone)]
pub(super) struct TrustedDraft;
/// Fixed worker-probe diagnostics, rendered through the escaped upload template.
#[derive(Clone)]
pub(super) struct WorkerUnavailable;
pub(super) fn trusted(response: &axum::response::Response) -> bool {
    (matches!(
        response.status(),
        StatusCode::CONFLICT | StatusCode::UNPROCESSABLE_ENTITY
    ) && response.extensions().get::<TrustedDraft>().is_some())
        || (response.status() == StatusCode::SERVICE_UNAVAILABLE
            && response.extensions().get::<WorkerUnavailable>().is_some())
}
pub(super) fn routes() -> Router<AuthState> {
    let router = Router::new();
    #[cfg(not(target_os = "linux"))]
    let router = router.route("/admin/materials/uploads", get(|State(state): State<AuthState>, session: Session| async move {
        owner_auth(&state, &session).await?;
        Ok::<_, StatusCode>(Html("<!doctype html><title>Upload unavailable</title><h1>Upload is not available on this platform</h1><p>PDF upload requires Linux and its sandbox.</p>"))
    }));
    router
        .route("/admin/materials", get(directory))
        .route("/admin/materials/sources/{id}", get(pages))
        .route("/admin/materials/sources/{id}/review", get(review_list))
        .route("/admin/materials/pages/{id}", get(page))
        .route("/admin/materials/suggestions/{id}", get(suggestion))
        .route(
            "/admin/materials/pages/{page}/units/{unit}/suggestions",
            get(suggestion_list),
        )
        .route("/admin/materials/pages/{id}/image", get(image))
        .route("/admin/materials/pages/{id}/tables/new", get(table_form))
        .route(
            "/assets/source-review.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../assets/source-review.js"),
                )
            }),
        )
        .route(
            "/admin/materials/pages/{page}/units/{unit}/image",
            get(region),
        )
        .route(
            "/assets/source-regions.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../assets/source-regions.js"),
                )
            }),
        )
        .merge(
            Router::new()
                .route("/admin/materials/pages/{id}", post(complete))
                .route("/admin/materials/suggestions/{id}/accept", post(accept))
                .route("/admin/materials/suggestions/{id}/dismiss", post(dismiss))
                .route("/admin/materials/pages/{id}/owner-units", post(add_owner))
                .route("/admin/materials/pages/{id}/tables", post(create_table))
                .route("/admin/materials/pages/{id}/primary", post(primary))
                .route("/admin/materials/sources/{id}/primary", post(primary_pages))
                .route("/admin/materials/pages/{page}/units/{unit}", post(review))
                .layer(DefaultBodyLimit::max(320 * 1024)),
        )
        .layer(axum::middleware::map_response(strict_policy))
}

/// Owner material pages render sanitised extraction HTML: they get the strict sheet policy
/// (`normalize` reads the marker), not only `frame-ancestors 'none'`.
async fn strict_policy(mut response: axum::response::Response) -> axum::response::Response {
    response
        .extensions_mut()
        .insert(crate::campaigns::web::SheetPage);
    response
}

pub(super) fn error(e: &AppError) -> StatusCode {
    match e {
        AppError::NotFound | AppError::SourceNotFound => StatusCode::FORBIDDEN,
        AppError::Conflict => StatusCode::CONFLICT,
        AppError::Invalid(crate::sources::review::READ_TIMEOUT) => StatusCode::REQUEST_TIMEOUT,
        AppError::Invalid(_) => StatusCode::UNPROCESSABLE_ENTITY,
        AppError::AuthStorageBusy => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
pub(super) fn html(t: &impl Template) -> AuthResult {
    Ok(Html(t.render().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?).into_response())
}
#[derive(Clone, Copy, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Cursor {
    after: i64,
}
#[derive(Clone, Copy, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PageCursor {
    units: i64,
    history: i64,
    notes: i64,
    saved: Saved,
    saved_unit: i64,
    to: i64,
    then: Then,
    /// An owner table the Owner just created (checked before it is announced).
    table: i64,
}
/// The decision a redirect confirms; only these fixed words are ever rendered.
#[derive(Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Saved {
    #[default]
    None,
    Accepted,
    Corrected,
    Rejected,
}
impl Saved {
    fn of(state: &str) -> Self {
        match state {
            "accepted" => Self::Accepted,
            "corrected" => Self::Corrected,
            "rejected" => Self::Rejected,
            _ => Self::None,
        }
    }
    fn key(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Accepted => "accepted",
            Self::Corrected => "corrected",
            Self::Rejected => "rejected",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Accepted => "accepted as extracted",
            Self::Corrected => "corrected with source evidence",
            Self::Rejected => "rejected / not evidence",
        }
    }
}
/// Where the redirect after a saved decision landed.
#[derive(Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Then {
    #[default]
    Unit,
    Page,
    Done,
    Unknown,
    /// The unit was already reviewed: the save stays on it.
    Stay,
}
impl Then {
    fn key(self) -> &'static str {
        match self {
            Self::Unit => "unit",
            Self::Page => "page",
            Self::Done => "done",
            Self::Unknown => "unknown",
            Self::Stay => "stay",
        }
    }
}
#[derive(Template)]
#[template(path = "materials.html")]
struct Directory {
    rows: Vec<Material>,
    next: Option<i64>,
}
async fn directory(
    State(state): State<AuthState>,
    session: Session,
    Query(c): Query<Cursor>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let listing = state
        .sessions
        .store
        .material_sources(auth, c.after)
        .await
        .map_err(|e| error(&e))?;
    html(&Directory {
        rows: listing.rows,
        next: listing.next,
    })
}
#[derive(Template)]
#[template(path = "material_pages.html")]
struct Pages {
    source: i64,
    rows: Vec<crate::sources::materials::PrimaryMaterialPage>,
    next: Option<i64>,
    csrf: String,
    message: String,
    rationale: String,
    after: i64,
}
async fn pages(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    Query(c): Query<Cursor>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let (_, listing) = state
        .sessions
        .store
        .material_primary_pages(auth, id, c.after)
        .await
        .map_err(|e| error(&e))?;
    html(&Pages {
        source: id,
        rows: listing.rows,
        next: listing.next,
        csrf: super::token(&session).await?.ok_or(StatusCode::FORBIDDEN)?,
        message: String::new(),
        rationale: String::new(),
        after: c.after,
    })
}

#[derive(Template)]
#[template(path = "material_review.html")]
struct ReviewListPage {
    source: i64,
    list: crate::sources::materials::ReviewList,
}
async fn review_list(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    Query(c): Query<Cursor>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let list = state
        .sessions
        .store
        .material_review_list(auth, id, c.after)
        .await
        .map_err(|e| error(&e))?;
    html(&ReviewListPage { source: id, list })
}

async fn primary_pages(
    State(state): State<AuthState>,
    session: Session,
    Path(source): Path<i64>,
    Form(mut form): Form<std::collections::BTreeMap<String, String>>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let token = form.remove("csrf").ok_or(StatusCode::FORBIDDEN)?;
    let csrf = csrf_admission(&session, &token).await?;
    let rationale = crate::form_text::newlines_to_lf(form.remove("rationale").unwrap_or_default());
    let after = form
        .remove("after")
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|n| *n >= 0)
        .ok_or(StatusCode::UNPROCESSABLE_ENTITY)?;
    let (document, mut listing) = state
        .sessions
        .store
        .material_primary_pages(auth.clone(), source, after)
        .await
        .map_err(|e| error(&e))?;
    let parsed = form
        .iter()
        .map(|(key, value)| {
            let pdf_page = key
                .strip_prefix("page_")
                .and_then(|s| s.parse::<i64>().ok())
                .ok_or(AppError::Invalid("Invalid selected page."))?;
            let (previous, event) = value
                .split_once(':')
                .ok_or(AppError::Invalid("Invalid expected primary tokens."))?;
            let previous = previous
                .parse::<i64>()
                .map_err(|_| AppError::Invalid("Invalid primary source token."))?;
            let event = event
                .parse::<i64>()
                .map_err(|_| AppError::Invalid("Invalid primary event token."))?;
            Ok(crate::sources::PrimaryPageState {
                pdf_page,
                source_id: (previous != 0).then_some(previous),
                event_id: (event != 0).then_some(event),
            })
        })
        .collect::<Result<Vec<_>, AppError>>();
    let result = match &parsed {
        Ok(pages) => {
            state
                .sessions
                .store
                .material_set_primary_source(
                    auth,
                    csrf,
                    document,
                    pages.clone(),
                    source,
                    rationale.clone(),
                )
                .await
        }
        Err(_) => Err(AppError::Invalid(
            "Invalid selected page or expected tokens.",
        )),
    };
    match result {
        Ok(_) => Ok(Redirect::to(&format!(
            "/admin/materials/sources/{source}?after={after}#primary-pages"
        ))
        .into_response()),
        Err(e @ (AppError::Invalid(_) | AppError::Conflict)) => {
            if let Ok(pages) = parsed {
                for row in &mut listing.rows {
                    if let Some(expected) = pages.iter().find(|p| p.pdf_page == row.page.pdf_page) {
                        row.selected = true;
                        row.submitted = Some(expected.clone());
                    }
                }
            }
            let mut response = html(&Pages {
                source,
                rows: listing.rows,
                next: listing.next,
                csrf: token,
                message: e.to_string(),
                rationale,
                after,
            })?;
            *response.status_mut() = error(&e);
            response.extensions_mut().insert(TrustedDraft);
            Ok(response)
        }
        Err(e) => Err(error(&e)),
    }
}
#[derive(Template)]
#[template(path = "material_page.html")]
struct Editor {
    page: PageView,
    csrf: String,
    message: String,
    units: i64,
    history: i64,
    units_next: Option<i64>,
    history_next: Option<i64>,
    draft: Option<Draft>,
    completion: Option<Completion>,
    screen: crate::sources::review::Screen,
    owner_draft: Option<OwnerDraft>,
    primary_draft: Option<PrimaryDraft>,
    notice: String,
    notice_unit: i64,
    /// The unit whose refused decision is shown with its error (0: none).
    error_unit: i64,
    /// Likely tables among the unreviewed units: `(label, link to the table form)`.
    proposals: Vec<(String, String)>,
}
impl Editor {
    fn unit_link(&self, target: &(i64, i64)) -> String {
        let (unit, cursor) = target;
        if self.page.units.iter().any(|row| row.id == *unit) {
            format!("#unit-{unit}")
        } else {
            format!("?units={cursor}#unit-{unit}")
        }
    }
    fn region_link(&self, region: &crate::sources::review::Overlay) -> String {
        if region.unit > 0 {
            self.unit_link(&(region.unit, region.cursor))
        } else if self
            .screen
            .notes
            .iter()
            .any(|row| row.id == region.suggestion)
        {
            format!("#suggestion-{}-unit-0", region.suggestion)
        } else {
            format!("/admin/materials/suggestions/{}", region.suggestion)
        }
    }
    fn draft_link(&self) -> String {
        let anchor = if self.draft.is_some() {
            format!("unit-{}", self.page.units.first().map_or(0, |unit| unit.id))
        } else if self.owner_draft.is_some() {
            "add-text".into()
        } else if self.primary_draft.is_some() {
            "primary".into()
        } else {
            "complete".into()
        };
        format!(
            "/admin/materials/pages/{}?units={}#{anchor}",
            self.page.id, self.units
        )
    }
    fn is_primary(&self) -> bool {
        self.screen.primary.source_id == Some(self.page.source_id)
    }
    fn new(
        data: MaterialPage,
        mut screen: crate::sources::review::Screen,
        csrf: String,
        c: PageCursor,
    ) -> Self {
        if screen.reads_reduced()
            && let Some(unit) = data.page.units.first()
        {
            // The first card has the known request cursor. Reserve its form
            // even after a read deadline so a refused POST keeps its draft.
            screen.cards.push(crate::sources::review::Card {
                unit: unit.id,
                cursor: c.units,
                lookup: String::new(),
                advice: Vec::new(),
                next: None,
                preview: true,
                form_omitted: false,
            });
        }
        Self {
            page: data.page,
            csrf,
            message: String::new(),
            units: c.units,
            history: c.history,
            units_next: data.units_next,
            history_next: data.history_next,
            draft: None,
            completion: None,
            screen,
            owner_draft: None,
            primary_draft: None,
            notice: saved_notice(c),
            notice_unit: if c.saved == Saved::None { 0 } else { c.to },
            error_unit: 0,
            proposals: Vec::new(),
        }
    }
}
fn saved_notice(c: PageCursor) -> String {
    if c.saved == Saved::None {
        return String::new();
    }
    let tail = match c.then {
        Then::Unit => " Now at the next unit still to review.",
        Then::Page => {
            " Every unit on the previous page is reviewed; this is the next page with units to review."
        }
        Then::Done => {
            " No unit is left to review on this page or after it; confirm page completeness below."
        }
        Then::Unknown => " The next unit could not be found in time; use \"Next unreviewed unit\".",
        Then::Stay => " Staying on this unit, which was already reviewed.",
    };
    format!("Saved unit {}: {}.{tail}", c.saved_unit, c.saved.label())
}
async fn page(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    Query(mut c): Query<PageCursor>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let mut created = 0;
    if c.table > 0
        && state
            .sessions
            .store
            .material_is_owner_table(auth.clone(), id, c.table)
            .await
            .unwrap_or(false)
    {
        created = c.table;
    }
    if c.saved != Saved::None {
        // A hand-made link must not claim a save that did not happen.
        let true_link = state
            .sessions
            .store
            .material_saved_matches(auth.clone(), id, c.saved_unit, c.saved.key(), c.then.key())
            .await
            .unwrap_or(false);
        if !true_link {
            c.saved = Saved::None;
        }
    }
    let data = state
        .sessions
        .store
        .material_page(auth.clone(), id, c.units, c.history)
        .await
        .map_err(|e| error(&e))?;
    let csrf = super::token(&session).await?.ok_or(StatusCode::FORBIDDEN)?;
    let screen = state
        .sessions
        .store
        .material_screen(
            auth.clone(),
            id,
            data.page.units.iter().map(|u| u.id).collect(),
            c.notes,
        )
        .await
        .map_err(|e| error(&e))?;
    // Proposals are a hint: a failed read shows none and never fails the page. The first few
    // that touch the cards shown are listed.
    let proposals = match state
        .sessions
        .store
        .material_table_proposals(auth.clone(), id)
        .await
    {
        Ok(found) => found,
        Err(
            AppError::Invalid(crate::sources::review::READ_TIMEOUT) | AppError::AuthStorageBusy,
        ) => std::sync::Arc::default(),
        Err(e) => {
            eprintln!("Mimik: table proposals for page {id} failed: {e}");
            std::sync::Arc::default()
        }
    };
    let shown: Vec<i64> = data.page.units.iter().map(|u| u.id).collect();
    let proposals: Vec<(String, String)> = proposals
        .iter()
        .filter(|p| p.units.iter().any(|u| shown.contains(u)))
        .take(crate::sources::proposals::MAX_PROPOSALS)
        .map(|p| (p.label(), p.link(id)))
        .collect();
    let mut editor = Editor::new(data, screen, csrf, c);
    editor.proposals = proposals;
    if created > 0 {
        editor.notice = format!(
            "Created table unit {created}; the selected units are rejected. Check the new table against the original page before accepting it."
        );
        editor.notice_unit = created;
    }
    html(&editor)
}
fn png(bytes: Vec<u8>) -> axum::response::Response {
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        bytes,
    )
        .into_response()
}
async fn image(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    Ok(png(state
        .sessions
        .store
        .material_image(auth, id)
        .await
        .map_err(|e| error(&e))?))
}
async fn region(
    State(state): State<AuthState>,
    session: Session,
    Path((page, unit)): Path<(i64, i64)>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let permit = crate::sources::region::admit().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let input = state
        .sessions
        .store
        .material_region_data(auth.clone(), page, unit, permit)
        .await
        .map_err(|e| error(&e))?;
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = input.permit;
        crate::sources::region::crop(
            &input.image,
            &input.prepared,
            input.pdf_page,
            &input.polygon,
        )
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| error(&e))?;
    #[cfg(test)]
    if let Some(hook) = crate::sources::materials::AFTER_CROP.lock().unwrap().take() {
        hook();
    }
    state
        .sessions
        .store
        .material_recheck(auth)
        .await
        .map_err(|e| error(&e))?;
    Ok(png(bytes))
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Draft {
    csrf: String,
    revision: i64,
    state: String,
    #[serde(deserialize_with = "crate::form_text::lf")]
    correction: String,
    #[serde(deserialize_with = "crate::form_text::lf")]
    rationale: String,
}
impl Draft {
    /// The fields that matter for the chosen decision (SRC-4): corrected text and rationale for a
    /// correction, a rationale for a rejection, nothing for an acceptance. Other fields are ignored.
    fn relevant(&self) -> (String, String) {
        match self.state.as_str() {
            "accepted" => (String::new(), String::new()),
            "rejected" => (String::new(), self.rationale.clone()),
            _ => (self.correction.clone(), self.rationale.clone()),
        }
    }
    fn correction_missing(&self) -> bool {
        self.state == "corrected" && self.correction.trim().is_empty()
    }
    fn rationale_missing(&self) -> bool {
        matches!(self.state.as_str(), "corrected" | "rejected") && self.rationale.trim().is_empty()
    }
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Completion {
    csrf: String,
    revision: i64,
    printed_label: String,
    #[serde(deserialize_with = "crate::form_text::lf")]
    rationale: String,
}
/// The redirect after a stored unit decision; only fixed words and numbers are placed in it.
fn saved_target(page: i64, unit: i64, state: &str, next: &ReviewNext) -> String {
    let label = Saved::of(state).key();
    match *next {
        ReviewNext::Unit { unit: to, cursor } => {
            let units = if cursor > 0 {
                format!("units={cursor}&")
            } else {
                String::new()
            };
            format!(
                "/admin/materials/pages/{page}?{units}saved={label}&saved_unit={unit}&to={to}&then=unit#unit-{to}"
            )
        }
        ReviewNext::Page(next) => format!(
            "/admin/materials/pages/{next}?saved={label}&saved_unit={unit}&then=page#saved-notice"
        ),
        ReviewNext::Done => format!(
            "/admin/materials/pages/{page}?saved={label}&saved_unit={unit}&then=done#complete"
        ),
        ReviewNext::Unknown => format!(
            "/admin/materials/pages/{page}?saved={label}&saved_unit={unit}&then=unknown#unit-{unit}"
        ),
    }
}
/// The redirect after re-deciding a reviewed unit: back to that unit, with its card window.
fn stay_target(page: i64, unit: i64, state: &str, cursor: i64) -> String {
    let label = Saved::of(state).key();
    let units = if cursor >= 50 {
        format!("units={cursor}&")
    } else {
        String::new()
    };
    format!(
        "/admin/materials/pages/{page}?{units}saved={label}&saved_unit={unit}&to={unit}&then=stay#unit-{unit}"
    )
}
async fn review(
    State(state): State<AuthState>,
    session: Session,
    Path((page, unit)): Path<(i64, i64)>,
    Form(form): Form<Draft>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let csrf = csrf_admission(&session, &form.csrf).await?;
    // A unit that was already decided stays in view after a re-decision; the state is read before
    // the save (a stale read is harmless: the revision check refuses a conflicting save).
    let was_reviewed = state
        .sessions
        .store
        .material_unit_state(auth.clone(), page, unit)
        .await
        .is_ok_and(|before| Saved::of(&before) != Saved::None);
    let result = state
        .sessions
        .store
        .material_review_unit(
            auth.clone(),
            csrf,
            page,
            unit,
            UnitReview {
                revision: form.revision,
                state: form.state.clone(),
                correction: form.relevant().0,
                rationale: form.relevant().1,
                actor: String::new(),
            },
        )
        .await;
    match result {
        Ok(()) => {
            // The decision is stored; a failed lookup only loses the jump, not the save.
            if was_reviewed
                && let Ok(cursor) = state
                    .sessions
                    .store
                    .material_unit_cursor(auth.clone(), page, unit)
                    .await
            {
                return Ok(
                    Redirect::to(&stay_target(page, unit, &form.state, cursor)).into_response()
                );
            }
            let next = state
                .sessions
                .store
                .material_next_after_review(auth, page, unit)
                .await
                .unwrap_or(ReviewNext::Unknown);
            Ok(Redirect::to(&saved_target(page, unit, &form.state, &next)).into_response())
        }
        Err(e @ (AppError::Invalid(_) | AppError::Conflict)) => {
            let unit_cursor = state
                .sessions
                .store
                .material_unit_cursor(auth.clone(), page, unit)
                .await
                .map_err(|e| error(&e))?;
            let data = state
                .sessions
                .store
                .material_page(auth.clone(), page, unit_cursor, 0)
                .await
                .map_err(|e| error(&e))?;
            let screen = state
                .sessions
                .store
                .material_screen(
                    auth,
                    page,
                    data.page.units.iter().map(|u| u.id).collect(),
                    0,
                )
                .await
                .map_err(|e| error(&e))?;
            let mut editor = Editor::new(
                data,
                screen,
                form.csrf.clone(),
                PageCursor {
                    units: unit_cursor,
                    history: 0,
                    notes: 0,
                    ..PageCursor::default()
                },
            );
            // Preserve only a verified unit in this page, never a browser-selected identity.
            let data = editor
                .page
                .units
                .iter_mut()
                .find(|u| u.id == unit)
                .ok_or(StatusCode::FORBIDDEN)?;
            data.correction.clone_from(&form.correction);
            data.state.clone_from(&form.state);
            editor.message = e.to_string();
            editor.error_unit = unit;
            editor.draft = Some(form);
            trusted_editor(&editor, error(&e))
        }
        Err(e) => Err(error(&e)),
    }
}
async fn complete(
    State(state): State<AuthState>,
    session: Session,
    Path(page): Path<i64>,
    Form(form): Form<Completion>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let csrf = csrf_admission(&session, &form.csrf).await?;
    let result = state
        .sessions
        .store
        .material_complete(
            auth.clone(),
            csrf,
            page,
            PageReview {
                revision: form.revision,
                printed_label: form.printed_label.clone(),
                rationale: form.rationale.clone(),
                actor: String::new(),
            },
        )
        .await;
    match result {
        Ok(()) => Ok(Redirect::to(&format!("/admin/materials/pages/{page}")).into_response()),
        Err(e @ (AppError::Invalid(_) | AppError::Conflict)) => {
            let data = state
                .sessions
                .store
                .material_page(auth.clone(), page, 0, 0)
                .await
                .map_err(|e| error(&e))?;
            let screen = state
                .sessions
                .store
                .material_screen(
                    auth,
                    page,
                    data.page.units.iter().map(|u| u.id).collect(),
                    0,
                )
                .await
                .map_err(|e| error(&e))?;
            let mut editor = Editor::new(data, screen, form.csrf.clone(), PageCursor::default());
            editor.message = e.to_string();
            editor.completion = Some(form);
            trusted_editor(&editor, error(&e))
        }
        Err(e) => Err(error(&e)),
    }
}
fn trusted_editor(editor: &Editor, status: StatusCode) -> AuthResult {
    let mut response = html(editor)?;
    *response.status_mut() = status;
    response.extensions_mut().insert(TrustedDraft);
    Ok(response)
}

#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SuggestionDraft {
    csrf: String,
    #[serde(deserialize_with = "crate::form_text::lf")]
    rationale: String,
    #[serde(deserialize_with = "crate::form_text::lf_option")]
    edited_text: Option<String>,
    edit: String,
    #[serde(deserialize_with = "crate::form_text::lf")]
    reason: String,
}
#[derive(Template)]
#[template(path = "material_suggestion.html")]
struct SuggestionEditor {
    advice: crate::sources::review::Advice,
    form: SuggestionDraft,
    message: String,
    show_difference: bool,
    text_preview: bool,
}
impl SuggestionEditor {
    async fn bounded_html(self, store: &crate::Store, auth: super::RequestAuth) -> AuthResult {
        let response = tokio::task::spawn_blocking(move || self.render_bounded())
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        store.material_recheck(auth).await.map_err(|e| error(&e))?;
        response
    }
    fn rendered_upper_bound(&self) -> usize {
        // Escaping needs at most six bytes per source byte. Fixed markup,
        // identifiers and every target link also have reserved space.
        16 * 1024
            + self.advice.target_links.len() * 256
            + [
                &self.advice.lookup,
                &self.advice.suggested,
                &self.advice.rationale,
                &self.advice.origin,
                &self.advice.target_label,
                &self.message,
                &self.form.csrf,
                &self.form.csrf,
                &self.form.rationale,
                self.form.edited_text.as_deref().unwrap_or(""),
                &self.form.reason,
            ]
            .iter()
            .map(|text| text.len().saturating_mul(6))
            .sum::<usize>()
    }
    fn render_bounded(mut self) -> AuthResult {
        const NORMAL: usize = 256 * 1024;
        #[cfg(test)]
        if let Some(hook) = crate::sources::materials::DETAIL_COMPARISON
            .lock()
            .unwrap()
            .take()
        {
            hook();
        }
        self.show_difference = false;
        if self.rendered_upper_bound() > NORMAL {
            self.text_preview = true;
            self.advice.lookup_truncated |= self.advice.lookup.chars().count() > 2000;
            self.advice.suggested_truncated |= self.advice.suggested.chars().count() > 2000;
            self.advice.lookup = self.advice.lookup.chars().take(2000).collect();
            self.advice.suggested = self.advice.suggested.chars().take(2000).collect();
            self.advice.rationale = self.advice.rationale.chars().take(2000).collect();
            self.advice.target_links.truncate(8);
        }
        // Reserve the entire escaped form, metadata and evidence first. A large
        // invalid draft is retained intact and uses the explicit error bound.
        let reserved = self.rendered_upper_bound();
        if reserved <= NORMAL && !self.text_preview {
            let remaining = NORMAL - reserved;
            let full = !self.advice.lookup_truncated
                && !self.advice.suggested_truncated
                && (self.advice.lookup.len() + self.advice.suggested.len()).saturating_mul(20)
                    <= remaining;
            self.advice.difference = crate::sources::review::difference(
                &self.advice.lookup,
                &self.advice.suggested,
                full.then(|| std::time::Instant::now() + std::time::Duration::from_millis(50)),
            );
            let difference_bound = if full {
                (self.advice.lookup.len() + self.advice.suggested.len()).saturating_mul(20)
            } else {
                (self.advice.difference.old.len() + self.advice.difference.new.len())
                    .saturating_mul(6)
                    + 1024
            };
            self.show_difference = difference_bound <= remaining;
        }
        let rendered = self
            .render()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if rendered.len() > 2 * 1024 * 1024 {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        Ok(Html(rendered).into_response())
    }
}
async fn suggestion(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let advice = state
        .sessions
        .store
        .material_advice(auth.clone(), id)
        .await
        .map_err(|e| error(&e))?;
    let form = SuggestionDraft {
        csrf: super::token(&session).await?.ok_or(StatusCode::FORBIDDEN)?,
        rationale: format!("Accepted suggestion #{} ({})", id, advice.origin),
        edited_text: Some(advice.suggested.clone()),
        ..SuggestionDraft::default()
    };
    SuggestionEditor {
        advice,
        form,
        message: String::new(),
        show_difference: false,
        text_preview: false,
    }
    .bounded_html(&state.sessions.store, auth)
    .await
}
#[derive(Template)]
#[template(path = "material_suggestions.html")]
struct SuggestionList {
    page: i64,
    unit: i64,
    rows: Vec<crate::sources::review::Advice>,
    next: Option<i64>,
    cursor: i64,
}
async fn suggestion_list(
    State(state): State<AuthState>,
    session: Session,
    Path((page, unit)): Path<(i64, i64)>,
    Query(c): Query<Cursor>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let cursor = state
        .sessions
        .store
        .material_unit_cursor(auth.clone(), page, unit)
        .await
        .map_err(|e| error(&e))?;
    let (rows, next) = state
        .sessions
        .store
        .material_advice_list(auth, page, unit, c.after)
        .await
        .map_err(|e| error(&e))?;
    html(&SuggestionList {
        page,
        unit,
        rows,
        next,
        cursor,
    })
}
async fn accept(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    Form(form): Form<SuggestionDraft>,
) -> AuthResult {
    suggestion_write(state, session, id, form, false).await
}
async fn dismiss(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    Form(form): Form<SuggestionDraft>,
) -> AuthResult {
    suggestion_write(state, session, id, form, true).await
}
async fn suggestion_write(
    state: AuthState,
    session: Session,
    id: i64,
    form: SuggestionDraft,
    dismissal: bool,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let csrf = csrf_admission(&session, &form.csrf).await?;
    let result = if dismissal {
        state
            .sessions
            .store
            .material_dismiss(auth.clone(), csrf, id, form.reason.clone())
            .await
    } else if form.edited_text.is_some() && !matches!(form.edit.as_str(), "yes" | "suggested") {
        Err(AppError::Invalid(
            "Choose Accept my edit or Accept as suggested explicitly.",
        ))
    } else {
        let checked = async {
            if form.edit == "suggested"
                && let Some(edited_text) = &form.edited_text
            {
                let advice = state
                    .sessions
                    .store
                    .material_advice(auth.clone(), id)
                    .await?;
                if *edited_text != crate::form_text::newlines_to_lf(advice.suggested) {
                    return Err(AppError::Invalid(
                        "The edited text differs from the stored suggestion. Choose Accept my edit or restore the suggested text.",
                    ));
                }
            }
            state
                .sessions
                .store
                .material_accept(
                    auth.clone(),
                    csrf,
                    id,
                    form.rationale.clone(),
                    (form.edit == "yes").then(|| form.edited_text.clone().unwrap_or_default()),
                )
                .await
        };
        checked.await
    };
    match result {
        Ok(back_link) => {
            #[cfg(test)]
            if let Some(hook) = crate::sources::materials::AFTER_SUGGESTION_COMMIT
                .lock()
                .unwrap()
                .take()
            {
                hook();
            }
            Ok(Redirect::to(&back_link).into_response())
        }
        Err(e @ (AppError::Invalid(_) | AppError::Conflict)) => {
            let advice = state
                .sessions
                .store
                .material_advice(auth.clone(), id)
                .await
                .map_err(|e| error(&e))?;
            let mut response = SuggestionEditor {
                advice,
                form,
                message: e.to_string(),
                show_difference: false,
                text_preview: false,
            }
            .bounded_html(&state.sessions.store, auth)
            .await?;
            *response.status_mut() = error(&e);
            response.extensions_mut().insert(TrustedDraft);
            Ok(response)
        }
        Err(e) => Err(error(&e)),
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerDraft {
    csrf: String,
    revision: i64,
    polygon: String,
    #[serde(deserialize_with = "crate::form_text::lf")]
    text: String,
    #[serde(deserialize_with = "crate::form_text::lf")]
    rationale: String,
}
async fn add_owner(
    State(state): State<AuthState>,
    session: Session,
    Path(page): Path<i64>,
    Form(form): Form<OwnerDraft>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let csrf = csrf_admission(&session, &form.csrf).await?;
    let result = match serde_json::from_str(&form.polygon) {
        Ok(polygon) => state
            .sessions
            .store
            .material_add_owner_unit(
                auth.clone(),
                csrf,
                page,
                crate::sources::OwnerUnit {
                    revision: form.revision,
                    polygon,
                    text: form.text.clone(),
                    actor: String::new(),
                    rationale: form.rationale.clone(),
                },
            )
            .await
            .map(|_| ()),
        Err(_) => Err(AppError::Invalid(
            "Enter four corners as [[x,y],[x,y],[x,y],[x,y]] in PDF points.",
        )),
    };
    match result {
        Ok(()) => {
            Ok(Redirect::to(&format!("/admin/materials/pages/{page}#add-text")).into_response())
        }
        Err(e @ (AppError::Invalid(_) | AppError::Conflict)) => {
            let mut editor = reload_editor(&state, auth, page, form.csrf.clone()).await?;
            editor.message = e.to_string();
            editor.owner_draft = Some(form);
            trusted_editor(&editor, error(&e))
        }
        Err(e) => Err(error(&e)),
    }
}
async fn reload_editor(
    state: &AuthState,
    auth: super::RequestAuth,
    page: i64,
    csrf: String,
) -> Result<Editor, StatusCode> {
    let data = state
        .sessions
        .store
        .material_page(auth.clone(), page, 0, 0)
        .await
        .map_err(|e| error(&e))?;
    let screen = state
        .sessions
        .store
        .material_screen(
            auth,
            page,
            data.page.units.iter().map(|u| u.id).collect(),
            0,
        )
        .await
        .map_err(|e| error(&e))?;
    Ok(Editor::new(data, screen, csrf, PageCursor::default()))
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrimaryDraft {
    csrf: String,
    source: i64,
    previous: i64,
    event: i64,
    #[serde(deserialize_with = "crate::form_text::lf")]
    rationale: String,
}
async fn primary(
    State(state): State<AuthState>,
    session: Session,
    Path(page): Path<i64>,
    Form(form): Form<PrimaryDraft>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let csrf = csrf_admission(&session, &form.csrf).await?;
    let (document, pdf_page) = state
        .sessions
        .store
        .material_primary_identity(auth.clone(), page)
        .await
        .map_err(|e| error(&e))?;
    let result = state
        .sessions
        .store
        .material_set_primary_source(
            auth.clone(),
            csrf,
            document,
            vec![crate::sources::PrimaryPageState {
                pdf_page,
                source_id: (form.previous != 0).then_some(form.previous),
                event_id: (form.event != 0).then_some(form.event),
            }],
            form.source,
            form.rationale.clone(),
        )
        .await;
    match result {
        Ok(_) => {
            Ok(Redirect::to(&format!("/admin/materials/pages/{page}#primary")).into_response())
        }
        Err(e @ (AppError::Invalid(_) | AppError::Conflict)) => {
            let mut editor = reload_editor(&state, auth, page, form.csrf.clone()).await?;
            editor.message = e.to_string();
            editor.primary_draft = Some(form);
            trusted_editor(&editor, error(&e))
        }
        Err(e) => Err(error(&e)),
    }
}

#[derive(Template)]
#[template(path = "material_table.html")]
struct TableEditor {
    page: i64,
    csrf: String,
    revision: i64,
    /// Hidden `unit` fields, `id:revision`, in reading order.
    units: Vec<String>,
    grid: String,
    /// The first row is the header (the form's checkbox).
    header: bool,
    /// "Read as" choices; scores only after a draft was computed.
    shapes: Vec<ShapeChoice>,
    /// Draft notes: problems found, units the table's box would cover, the overlap check's state.
    problems: Vec<String>,
    overlaps: String,
    overlap_unchecked: bool,
    /// Why Column grid read text instead of word boxes (empty when it had them).
    words_note: &'static str,
    /// The shape the draft was read as (a hidden field, so a refused save can show the notes).
    shape_key: String,
    rationale: String,
    message: String,
}

struct ShapeChoice {
    key: &'static str,
    label: &'static str,
    score: Option<u32>,
    current: bool,
}

impl TableEditor {
    fn new(page: i64, csrf: String) -> Self {
        Self {
            page,
            csrf,
            revision: 0,
            units: Vec::new(),
            grid: String::new(),
            header: false,
            shapes: Vec::new(),
            problems: Vec::new(),
            overlaps: String::new(),
            overlap_unchecked: false,
            words_note: "",
            shape_key: String::new(),
            rationale: String::new(),
            message: String::new(),
        }
    }

    /// The plain unit ids (no revisions) the "Read as" form submits back.
    fn unit_ids(&self) -> Vec<&str> {
        self.units
            .iter()
            .map(|unit| unit.split_once(':').map_or(unit.as_str(), |(id, _)| id))
            .collect()
    }
}

/// Every shape the form offers; scores are shown for those a draft was computed for.
fn shape_choices(
    current: Option<Shape>,
    scores: &[(Shape, u32)],
    units: usize,
) -> Vec<ShapeChoice> {
    Shape::ALL
        .into_iter()
        .filter(|shape| {
            let offered = scores.is_empty() || scores.iter().any(|(s, _)| s == shape);
            let plain_ok = *shape != Shape::Plain || units <= crate::sources::tables::MAX_COLUMNS;
            plain_ok && (offered || current == Some(*shape))
        })
        .map(|shape| ShapeChoice {
            key: shape.key(),
            label: shape.label(),
            score: scores.iter().find(|(s, _)| *s == shape).map(|(_, n)| *n),
            current: current == Some(shape),
        })
        .collect()
}

fn table_editor_response(editor: &TableEditor, status: StatusCode) -> AuthResult {
    let mut response = html(editor)?;
    *response.status_mut() = status;
    response.extensions_mut().insert(TrustedDraft);
    Ok(response)
}
/// The create-table form, prefilled from the units chosen with the cards' checkboxes.
async fn table_form(
    State(state): State<AuthState>,
    session: Session,
    Path(page): Path<i64>,
    Query(fields): Query<Vec<(String, String)>>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let csrf = super::token(&session).await?.ok_or(StatusCode::FORBIDDEN)?;
    let mut editor = TableEditor::new(page, csrf);
    // `unit` (repeated) and at most one `shape`; nothing else is read from the URL.
    let mut shape = None;
    let mut ids = Vec::new();
    let mut malformed = None;
    for (key, value) in &fields {
        match key.as_str() {
            "unit" => match value.parse::<i64>() {
                Ok(id) => ids.push(id),
                Err(_) => malformed = Some("Choose 1-200 different units for a table."),
            },
            "shape" if shape.is_none() => match Shape::from_key(value) {
                Some(s) => shape = Some(s),
                None => malformed = Some("Unknown table shape."),
            },
            _ => malformed = Some("Choose 1-200 different units for a table."),
        }
    }
    let prefill = match malformed {
        None => {
            state
                .sessions
                .store
                .material_table_prefill(auth, page, ids, shape)
                .await
        }
        Some(message) => Err(AppError::Invalid(message)),
    };
    match prefill {
        Ok(prefill) => {
            editor.revision = prefill.page_revision;
            editor.units = prefill
                .units
                .iter()
                .map(|(u, r)| format!("{u}:{r}"))
                .collect();
            editor.grid = prefill.grid;
            editor.header = prefill.header;
            editor.shapes = shape_choices(Some(prefill.shape), &prefill.scores, editor.units.len());
            editor.problems = prefill.problems;
            editor.overlaps = overlap_note(&prefill.overlaps);
            editor.overlap_unchecked = !prefill.overlap_checked;
            editor.words_note = prefill.words.note();
            prefill.shape.key().clone_into(&mut editor.shape_key);
            html(&editor)
        }
        Err(e @ AppError::Invalid(_)) => {
            editor.message = e.to_string();
            table_editor_response(&editor, error(&e))
        }
        Err(e) => Err(error(&e)),
    }
}
/// Names the unselected units the table's combined box would cover (the first few).
fn overlap_note(units: &[i64]) -> String {
    const NAMED: usize = 8;
    if units.is_empty() {
        return String::new();
    }
    let named: Vec<String> = units.iter().take(NAMED).map(i64::to_string).collect();
    let more = units.len().saturating_sub(NAMED);
    let tail = if more > 0 {
        format!(" and {more} more")
    } else {
        String::new()
    };
    format!("unit {}{tail}", named.join(", unit "))
}
/// A refused save re-renders with the draft notes recomputed for the same units and shape; the
/// cells, header and rationale stay as the Owner typed them.
async fn refresh_table_notes(
    state: &AuthState,
    auth: crate::auth::RequestAuth,
    page: i64,
    editor: &mut TableEditor,
    shape: Option<Shape>,
) {
    let ids: Vec<i64> = editor
        .units
        .iter()
        .filter_map(|u| u.split_once(':')?.0.parse().ok())
        .collect();
    if let Ok(prefill) = state
        .sessions
        .store
        .material_table_prefill(auth, page, ids, shape)
        .await
    {
        editor.shapes = shape_choices(Some(prefill.shape), &prefill.scores, editor.units.len());
        editor.problems = prefill.problems;
        editor.overlaps = overlap_note(&prefill.overlaps);
        editor.overlap_unchecked = !prefill.overlap_checked;
        editor.words_note = prefill.words.note();
        prefill.shape.key().clone_into(&mut editor.shape_key);
    }
}
async fn create_table(
    State(state): State<AuthState>,
    session: Session,
    Path(page): Path<i64>,
    Form(fields): Form<Vec<(String, String)>>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let (mut csrf_token, mut revision, mut cells, mut rationale) =
        (None, None, None, None::<String>);
    let mut header = None::<bool>;
    let mut shape = None::<Shape>;
    let mut units = Vec::new();
    let mut malformed = false;
    for (key, value) in fields {
        let value = crate::form_text::newlines_to_lf(value);
        match key.as_str() {
            "csrf" if csrf_token.is_none() => csrf_token = Some(value),
            "revision" if revision.is_none() => revision = value.parse::<i64>().ok(),
            "cells" if cells.is_none() => cells = Some(value),
            "rationale" if rationale.is_none() => rationale = Some(value),
            "header" if header.is_none() && value == "1" => header = Some(true),
            "shape" if shape.is_none() => match Shape::from_key(&value) {
                Some(s) => shape = Some(s),
                None => malformed = true,
            },
            "unit" => units.push(value),
            _ => malformed = true,
        }
    }
    let csrf_token = csrf_token.ok_or(StatusCode::FORBIDDEN)?;
    let csrf = csrf_admission(&session, &csrf_token).await?;
    let mut editor = TableEditor {
        revision: revision.unwrap_or(0),
        units,
        grid: cells.unwrap_or_default(),
        header: header.unwrap_or(false),
        rationale: rationale.unwrap_or_default(),
        ..TableEditor::new(page, csrf_token)
    };
    editor.shapes = shape_choices(None, &[], editor.units.len());
    let sources = editor
        .units
        .iter()
        .map(|unit| {
            let (id, revision) = unit.split_once(':')?;
            Some(crate::sources::TableSource {
                unit: id.parse().ok()?,
                revision: revision.parse().ok()?,
            })
        })
        .collect::<Option<Vec<_>>>();
    let result = match (malformed || revision.is_none(), sources) {
        (false, Some(sources)) => {
            match crate::sources::tables::parse_grid(&editor.grid, editor.header) {
                Ok(cells) => {
                    state
                        .sessions
                        .store
                        .material_create_table(
                            auth.clone(),
                            csrf,
                            page,
                            crate::sources::OwnerTable {
                                revision: editor.revision,
                                sources,
                                cells,
                                header: editor.header,
                                actor: String::new(),
                                rationale: editor.rationale.clone(),
                            },
                        )
                        .await
                }
                Err(e) => Err(e),
            }
        }
        _ => Err(AppError::Invalid(
            "The table form was incomplete; reopen it from the page.",
        )),
    };
    match result {
        Ok((unit, _)) => {
            let cursor = state
                .sessions
                .store
                .material_unit_cursor(auth, page, unit)
                .await
                .unwrap_or(0);
            Ok(Redirect::to(&format!(
                "/admin/materials/pages/{page}?units={cursor}&table={unit}#unit-{unit}"
            ))
            .into_response())
        }
        Err(e @ (AppError::Invalid(_) | AppError::Conflict)) => {
            editor.message = e.to_string();
            refresh_table_notes(&state, auth, page, &mut editor, shape).await;
            table_editor_response(&editor, error(&e))
        }
        Err(e) => Err(error(&e)),
    }
}
