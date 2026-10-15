use super::{EvidenceSearch, PageReview, PageView, SourceSummary, UnitReview};
use crate::{
    AppError, Store,
    web::{check_origin, render},
};
use askama::Template;
use axum::{
    Form, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};
use serde::Deserialize;

#[derive(Template)]
#[template(path = "sources.html")]
struct SourcesTemplate {
    sources: Vec<SourceSummary>,
}

#[derive(Template)]
#[template(path = "source_setup.html")]
struct SetupTemplate;

#[derive(Template)]
#[template(path = "source_page.html")]
struct PageTemplate {
    page: PageView,
    message: String,
    actor: String,
    rationale: String,
}

#[derive(Deserialize)]
pub(crate) struct SearchQuery {
    #[serde(default = "system")]
    system: String,
    #[serde(default)]
    edition: String,
    #[serde(default)]
    edition_id: String,
    #[serde(default)]
    query: String,
}

fn system() -> String {
    "mork-borg".into()
}

#[derive(Template)]
#[template(path = "source_search.html")]
struct SearchTemplate {
    query: SearchQuery,
    result: Option<EvidenceSearch>,
}

pub(crate) fn routes() -> Router<Store> {
    Router::new()
        .route("/sources", get(index))
        .route("/sources/setup", get(|| async { render(&SetupTemplate) }))
        .route("/sources/search", get(search))
        .route("/sources/pages/{id}", get(page).post(complete))
        .route("/sources/pages/{id}/image", get(image))
        .route(
            "/sources/pages/{page}/units/{unit}/image",
            get(region_image),
        )
        .route(
            "/sources/pages/{page}/units/{unit}",
            axum::routing::post(review),
        )
}

async fn index(State(store): State<Store>) -> Result<Html<String>, AppError> {
    render(&SourcesTemplate {
        sources: store.sources().await?,
    })
}

async fn page(State(store): State<Store>, Path(id): Path<i64>) -> Result<Html<String>, AppError> {
    render(&PageTemplate {
        page: store.source_page(id).await?,
        message: String::new(),
        actor: String::new(),
        rationale: String::new(),
    })
}

async fn image(State(store): State<Store>, Path(id): Path<i64>) -> Result<Response, AppError> {
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        store.source_image(id).await?,
    )
        .into_response())
}

async fn region_image(
    State(store): State<Store>,
    Path((page, unit)): Path<(i64, i64)>,
) -> Result<Response, AppError> {
    let Some(permit) = super::region::admit() else {
        return Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            [
                (header::RETRY_AFTER, "1"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            "Region preview busy; retry or inspect the full-page image.",
        )
            .into_response());
    };
    let data = store.source_region_data(page, unit, permit).await?;
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = data.permit;
        super::region::crop(&data.image, &data.prepared, data.pdf_page, &data.polygon)
    })
    .await
    .map_err(|_| AppError::Invalid("Region preview worker failed; inspect full page."))??;
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        bytes,
    )
        .into_response())
}

async fn review(
    State(store): State<Store>,
    Path((page, unit)): Path<(i64, i64)>,
    headers: HeaderMap,
    Form(review): Form<UnitReview>,
) -> Result<Response, AppError> {
    check_origin(&headers)?;
    match store.review_source_unit(page, unit, review.clone()).await {
        Ok(()) => Ok(Redirect::to(&format!("/sources/pages/{page}")).into_response()),
        Err(error @ (AppError::Invalid(_) | AppError::Conflict)) => {
            let mut data = store.source_page(page).await?;
            if let Some(data) = data.units.iter_mut().find(|data| data.id == unit) {
                data.correction = review.correction;
                data.revision = review.revision;
                data.state = review.state;
            }
            Ok((
                error.status(),
                render(&PageTemplate {
                    page: data,
                    message: error.to_string(),
                    actor: review.actor,
                    rationale: review.rationale,
                })?,
            )
                .into_response())
        }
        Err(error) => Err(error),
    }
}

async fn complete(
    State(store): State<Store>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(review): Form<PageReview>,
) -> Result<Response, AppError> {
    check_origin(&headers)?;
    let actor = review.actor.clone();
    let rationale = review.rationale.clone();
    let printed_label = review.printed_label.clone();
    let revision = review.revision;
    match store.complete_source_page(id, review).await {
        Ok(()) => Ok(Redirect::to(&format!("/sources/pages/{id}")).into_response()),
        Err(error @ (AppError::Invalid(_) | AppError::Conflict)) => {
            let mut page = store.source_page(id).await?;
            page.printed_label = printed_label;
            page.revision = revision;
            Ok((
                error.status(),
                render(&PageTemplate {
                    page,
                    message: error.to_string(),
                    actor,
                    rationale,
                })?,
            )
                .into_response())
        }
        Err(error) => Err(error),
    }
}

async fn search(
    State(store): State<Store>,
    Query(query): Query<SearchQuery>,
) -> Result<Html<String>, AppError> {
    let result = if query.query.trim().is_empty() {
        None
    } else if !query.edition_id.is_empty() {
        if !query.edition.is_empty() {
            return Err(AppError::Invalid(
                "Select either a stable edition ID or a legacy/display label, not both.",
            ));
        }
        Some(
            store
                .search_ruleset_evidence(
                    query.system.clone(),
                    query.edition_id.clone(),
                    query.query.clone(),
                )
                .await?,
        )
    } else {
        Some(
            store
                .search_evidence(
                    query.system.clone(),
                    query.edition.clone(),
                    query.query.clone(),
                )
                .await?,
        )
    };
    render(&SearchTemplate { query, result })
}
