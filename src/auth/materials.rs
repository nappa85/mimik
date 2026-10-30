//! Owner-only source comparison and explicitly attributed review forms.
use super::{AuthResult, AuthState, csrf_admission, owner_auth};
use crate::{
    AppError,
    sources::{
        PageReview, PageView, UnitReview,
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
pub(super) fn trusted(response: &axum::response::Response) -> bool {
    matches!(
        response.status(),
        StatusCode::CONFLICT | StatusCode::UNPROCESSABLE_ENTITY
    ) && response.extensions().get::<TrustedDraft>().is_some()
}
pub(super) fn routes() -> Router<AuthState> {
    Router::new()
        .route("/admin/materials", get(directory))
        .route("/admin/materials/sources/{id}", get(pages))
        .route("/admin/materials/pages/{id}", get(page))
        .route("/admin/materials/suggestions/{id}", get(suggestion))
        .route(
            "/admin/materials/pages/{page}/units/{unit}/suggestions",
            get(suggestion_list),
        )
        .route("/admin/materials/pages/{id}/image", get(image))
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
                .route("/admin/materials/pages/{id}/primary", post(primary))
                .route("/admin/materials/sources/{id}/primary", post(primary_pages))
                .route("/admin/materials/pages/{page}/units/{unit}", post(review))
                .layer(DefaultBodyLimit::max(320 * 1024)),
        )
}
pub(super) fn error(e: &AppError) -> StatusCode {
    match e {
        AppError::NotFound | AppError::SourceNotFound => StatusCode::FORBIDDEN,
        AppError::Conflict => StatusCode::CONFLICT,
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
        screen: crate::sources::review::Screen,
        csrf: String,
        c: PageCursor,
    ) -> Self {
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
        }
    }
}
async fn page(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    Query(c): Query<PageCursor>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
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
            auth,
            id,
            data.page.units.iter().map(|u| u.id).collect(),
            c.notes,
        )
        .await
        .map_err(|e| error(&e))?;
    html(&Editor::new(data, screen, csrf, c))
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
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Completion {
    csrf: String,
    revision: i64,
    printed_label: String,
    #[serde(deserialize_with = "crate::form_text::lf")]
    rationale: String,
}
async fn review(
    State(state): State<AuthState>,
    session: Session,
    Path((page, unit)): Path<(i64, i64)>,
    Form(form): Form<Draft>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let csrf = csrf_admission(&session, &form.csrf).await?;
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
                correction: form.correction.clone(),
                rationale: form.rationale.clone(),
                actor: String::new(),
            },
        )
        .await;
    match result {
        Ok(()) => Ok(Redirect::to(&format!("/admin/materials/pages/{page}")).into_response()),
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
    #[serde(deserialize_with = "crate::form_text::lf")]
    edited_text: String,
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
    fn bounded_html(mut self) -> AuthResult {
        const NORMAL: usize = 256 * 1024;
        self.show_difference = false;
        let mut rendered = self
            .render()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if rendered.len() > NORMAL {
            self.text_preview = true;
            self.advice.lookup = self.advice.lookup.chars().take(2000).collect();
            self.advice.suggested = self.advice.suggested.chars().take(2000).collect();
            self.advice.rationale = self.advice.rationale.chars().take(2000).collect();
            self.advice.target_links.truncate(8);
            rendered = self
                .render()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        // Reserve the entire escaped form, metadata and evidence first. A large
        // invalid draft is retained intact and uses the explicit error bound.
        if rendered.len() <= NORMAL && !self.text_preview {
            let remaining = NORMAL - rendered.len();
            let full = !self.advice.lookup_truncated
                && (self.advice.lookup.len() + self.advice.suggested.len()).saturating_mul(20)
                    <= remaining;
            self.advice.difference = crate::sources::review::difference(
                &self.advice.lookup,
                &self.advice.suggested,
                full.then(|| std::time::Instant::now() + std::time::Duration::from_millis(50)),
            );
            self.show_difference = true;
            let candidate = self
                .render()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            if candidate.len() <= NORMAL {
                rendered = candidate;
            }
        }
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
        .material_advice(auth, id)
        .await
        .map_err(|e| error(&e))?;
    let form = SuggestionDraft {
        csrf: super::token(&session).await?.ok_or(StatusCode::FORBIDDEN)?,
        rationale: format!("Accepted suggestion #{} ({})", id, advice.origin),
        edited_text: advice.suggested.clone(),
        ..SuggestionDraft::default()
    };
    SuggestionEditor {
        advice,
        form,
        message: String::new(),
        show_difference: false,
        text_preview: false,
    }
    .bounded_html()
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
    } else if !form.edited_text.is_empty() && !matches!(form.edit.as_str(), "yes" | "suggested") {
        Err(AppError::Invalid(
            "Choose Accept my edit or Accept as suggested explicitly.",
        ))
    } else {
        state
            .sessions
            .store
            .material_accept(
                auth.clone(),
                csrf,
                id,
                form.rationale.clone(),
                (form.edit == "yes").then(|| form.edited_text.clone()),
            )
            .await
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
                .material_advice(auth, id)
                .await
                .map_err(|e| error(&e))?;
            let mut response = SuggestionEditor {
                advice,
                form,
                message: e.to_string(),
                show_difference: false,
                text_preview: false,
            }
            .bounded_html()?;
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
