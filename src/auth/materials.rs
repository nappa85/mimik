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
        .route("/admin/materials/pages/{id}/image", get(image))
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
                .route("/admin/materials/pages/{page}/units/{unit}", post(review))
                .layer(DefaultBodyLimit::max(320 * 1024)),
        )
}
pub(super) fn error(e: &AppError) -> StatusCode {
    match e {
        AppError::NotFound => StatusCode::FORBIDDEN,
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
    rows: Vec<crate::sources::PageSummary>,
    next: Option<i64>,
}
async fn pages(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    Query(c): Query<Cursor>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let listing = state
        .sessions
        .store
        .material_pages(auth, id, c.after)
        .await
        .map_err(|e| error(&e))?;
    html(&Pages {
        source: id,
        rows: listing.rows,
        next: listing.next,
    })
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
}
impl Editor {
    fn new(data: MaterialPage, csrf: String, c: PageCursor) -> Self {
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
        .material_page(auth, id, c.units, c.history)
        .await
        .map_err(|e| error(&e))?;
    let csrf = super::token(&session).await?.ok_or(StatusCode::FORBIDDEN)?;
    html(&Editor::new(data, csrf, c))
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
    correction: String,
    rationale: String,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Completion {
    csrf: String,
    revision: i64,
    printed_label: String,
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
            let data = state
                .sessions
                .store
                .material_page(auth, page, unit.saturating_sub(1), 0)
                .await
                .map_err(|e| error(&e))?;
            let mut editor = Editor::new(
                data,
                form.csrf.clone(),
                PageCursor {
                    units: unit.saturating_sub(1),
                    history: 0,
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
                .material_page(auth, page, 0, 0)
                .await
                .map_err(|e| error(&e))?;
            let mut editor = Editor::new(data, form.csrf.clone(), PageCursor::default());
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
