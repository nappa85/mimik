//! Read-only catalogue routes reached through a private sheet (catalogue plan D11). Pages
//! load no script. Every author or manual string is escaped by Askama and wrapped in `<bdi>`,
//! so bidirectional controls in owner or unit text cannot reorder the surrounding names,
//! citations or controls; long text keeps its line breaks (`white-space: pre-wrap`).
use super::{
    catalogue::{Body, CataloguePage, Request},
    web::{PrivateTrustedEditor, ResultResponse, RuntimeAdmission, error},
};
use crate::{
    auth::{self, AuthState},
    catalogues::read::{self, Badge, BrowseQuery, Citation, Description, EntryPage},
};
use askama::Template;
use axum::{
    Extension,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse},
};
use serde::Deserialize;
use tower_sessions::Session;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BrowseParams {
    #[serde(default, rename = "type")]
    entry_type: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    after: Option<String>,
}

/// An optional slug parameter: empty means absent (the form's "All types"); anything else
/// must be a registry id.
fn slug(value: Option<String>) -> Result<Option<String>, StatusCode> {
    match value {
        Some(value) if value.is_empty() => Ok(None),
        Some(value) if crate::registry::valid_id(&value) => Ok(Some(value)),
        Some(_) => Err(StatusCode::BAD_REQUEST),
        None => Ok(None),
    }
}

fn browse_query(params: BrowseParams) -> Result<BrowseQuery, StatusCode> {
    let q = params
        .q
        .map(|q| q.trim().to_owned())
        .filter(|q| !q.is_empty());
    if q.as_ref()
        .is_some_and(|q| q.chars().count() > read::MAX_QUERY_SCALARS)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(BrowseQuery {
        entry_type: slug(params.entry_type)?,
        q,
        after: slug(params.after)?,
    })
}

pub(super) async fn browse(
    State(state): State<AuthState>,
    session: Session,
    Path((id, catalogue)): Path<(i64, String)>,
    Query(params): Query<BrowseParams>,
    Extension(admission): Extension<RuntimeAdmission>,
) -> ResultResponse {
    if !crate::registry::valid_id(&catalogue) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let query = browse_query(params)?;
    let echo = Echo {
        entry_type: query.entry_type.clone().unwrap_or_default(),
        q: query.q.clone().unwrap_or_default(),
        paged: query.after.is_some(),
    };
    respond(
        state,
        session,
        id,
        catalogue,
        Request::Browse(query),
        echo,
        admission,
    )
    .await
}

pub(super) async fn entry(
    State(state): State<AuthState>,
    session: Session,
    Path((id, catalogue, key)): Path<(i64, String, String)>,
    Extension(admission): Extension<RuntimeAdmission>,
) -> ResultResponse {
    if !crate::registry::valid_id(&catalogue) || !crate::registry::valid_id(&key) {
        return Err(StatusCode::BAD_REQUEST);
    }
    respond(
        state,
        session,
        id,
        catalogue,
        Request::Entry(key),
        Echo::default(),
        admission,
    )
    .await
}

/// The browse filters echoed into the search form and the paging links.
#[derive(Default)]
struct Echo {
    entry_type: String,
    q: String,
    paged: bool,
}

async fn respond(
    state: AuthState,
    session: Session,
    id: i64,
    catalogue: String,
    request: Request,
    echo: Echo,
    admission: RuntimeAdmission,
) -> ResultResponse {
    let permit = admission.take()?;
    let auth = auth::request_auth(&state, &session).await?;
    let (page, permit) = state
        .store()
        .private_catalogue(auth, id, catalogue, request, permit)
        .await
        .map_err(error)?;
    // Rendering keeps the runtime permit (R6 phase 4).
    let (status, html) = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        render(&page, &echo)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;
    let mut response = (status, Html(html)).into_response();
    if status == StatusCode::UNPROCESSABLE_ENTITY {
        response.extensions_mut().insert(PrivateTrustedEditor);
    }
    Ok(response)
}

struct ResultView {
    href: String,
    name: String,
    type_label: String,
    badge: &'static str,
    retired: bool,
}

#[derive(Template)]
#[template(path = "catalogue_browse.html")]
struct BrowseTemplate<'a> {
    sheet_href: String,
    action: String,
    title: &'a str,
    q: &'a str,
    entry_type: &'a str,
    types: &'a [(String, String)],
    message: &'a str,
    results: Vec<ResultView>,
    searched: bool,
    next: Option<String>,
    first: Option<String>,
}

struct CitationView {
    source: String,
    printed_label: String,
    pdf_page: i64,
}

impl From<&Citation> for CitationView {
    fn from(citation: &Citation) -> Self {
        Self {
            source: citation.source.clone(),
            printed_label: citation.printed_label.clone(),
            pdf_page: citation.pdf_page,
        }
    }
}

#[derive(Template)]
#[template(path = "catalogue_entry.html")]
struct EntryTemplate<'a> {
    sheet_href: String,
    browse_href: String,
    title: &'a str,
    name: &'a str,
    type_label: &'a str,
    retired: bool,
    badge: &'static str,
    sentence: &'static str,
    owner_written: bool,
    properties: &'a [(String, String)],
    owner_text: Option<&'a str>,
    parts: &'a [String],
    cut: Option<CitationView>,
    missing: Missing,
    citations: Vec<CitationView>,
}

/// Why no description text is shown, if none is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Missing {
    No,
    /// A cited unit is rejected now.
    Rejected,
    /// A cited unit's text is not text, holds NUL or exceeds its bound.
    Malformed,
}

fn browse_link(base: &str, echo: &Echo, after: Option<&str>) -> Result<String, StatusCode> {
    let pairs: Vec<(&str, &str)> = [
        ("type", echo.entry_type.as_str()),
        ("q", echo.q.as_str()),
        ("after", after.unwrap_or_default()),
    ]
    .into_iter()
    .filter(|(_, value)| !value.is_empty())
    .collect();
    if pairs.is_empty() {
        return Ok(base.to_owned());
    }
    let query =
        serde_urlencoded::to_string(pairs).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(format!("{base}?{query}"))
}

fn entry_page(
    page: &CataloguePage,
    entry: &EntryPage,
    sheet_href: String,
    browse_href: String,
) -> askama::Result<String> {
    let (owner_text, parts, cut): (Option<&str>, &[String], Option<CitationView>) =
        match &entry.description {
            Description::OwnerText(text) => (Some(text.as_str()), &[], None),
            Description::Units { parts, cut } => {
                (None, parts.as_slice(), cut.as_ref().map(CitationView::from))
            }
            Description::Unavailable | Description::Malformed => (None, &[], None),
        };
    EntryTemplate {
        sheet_href,
        browse_href,
        title: &page.title,
        name: &entry.name,
        type_label: &entry.type_label,
        retired: entry.retired,
        badge: entry.badge.label(),
        sentence: entry.badge.sentence(),
        owner_written: entry.badge == Badge::OwnerWritten,
        properties: &entry.properties,
        owner_text,
        parts,
        cut,
        missing: match entry.description {
            Description::Unavailable => Missing::Rejected,
            Description::Malformed => Missing::Malformed,
            _ => Missing::No,
        },
        citations: entry.citations.iter().map(CitationView::from).collect(),
    }
    .render()
}

fn render(page: &CataloguePage, echo: &Echo) -> Result<(StatusCode, String), StatusCode> {
    let sheet_href = format!("/private-sheets/{}", page.sheet_id);
    let base = format!("{sheet_href}/catalogue/{}", page.catalogue);
    let rendered = match &page.body {
        Body::Browse(found) => {
            let results = found
                .rows
                .iter()
                .map(|row| ResultView {
                    href: format!("{base}/{}", row.key),
                    name: row.name.clone(),
                    type_label: row.type_label.clone(),
                    badge: row.badge.label(),
                    retired: row.retired,
                })
                .collect();
            let next = if found.more {
                found
                    .rows
                    .last()
                    .map(|row| browse_link(&base, echo, Some(&row.key)))
                    .transpose()?
            } else {
                None
            };
            let first = if echo.paged {
                Some(browse_link(&base, echo, None)?)
            } else {
                None
            };
            (
                StatusCode::OK,
                BrowseTemplate {
                    sheet_href,
                    action: base,
                    title: &page.title,
                    q: &echo.q,
                    entry_type: &echo.entry_type,
                    types: &found.types,
                    message: "",
                    results,
                    searched: !echo.q.is_empty() || !echo.entry_type.is_empty(),
                    next,
                    first,
                }
                .render(),
            )
        }
        Body::Problem(message) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            BrowseTemplate {
                sheet_href,
                action: base,
                title: &page.title,
                q: &echo.q,
                entry_type: &echo.entry_type,
                types: &[],
                message,
                results: Vec::new(),
                searched: true,
                next: None,
                first: None,
            }
            .render(),
        ),
        Body::Entry(entry) => (StatusCode::OK, entry_page(page, entry, sheet_href, base)),
    };
    let (status, html) = rendered;
    Ok((status, html.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?))
}
