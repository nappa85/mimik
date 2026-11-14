//! Read-only catalogue routes reached through a private sheet (catalogue plan D11). Pages
//! load no script. Every author or manual string is escaped by Askama and wrapped in `<bdi>`,
//! so bidirectional controls in owner or unit text cannot reorder the surrounding names,
//! citations or controls; long text keeps its line breaks (`white-space: pre-wrap`).
//!
//! Errors explain themselves (plan §19): a malformed address answers 400 before any lookup;
//! every refusal (unreadable or unknown sheet, native or unsupported pin, unlinked, unknown,
//! other games' or unsealed catalogue, revoked grant or session) answers one uniform 403 page
//! whose only variable is the sheet link taken from the address; an entry key or entry type
//! missing from a catalogue the reader may browse answers 404.
use super::{
    catalogue::{Body, CataloguePage, Request},
    web::{CatalogueNotice, PrivateTrustedEditor, ResultResponse, RuntimeAdmission, error},
};
use crate::{
    AppError,
    auth::{self, AuthState},
    catalogues::read::{self, Badge, BrowseQuery, Citation, Description, EntryPage},
};
use askama::Template;
use axum::{
    Extension,
    extract::{
        Path, Query, State,
        rejection::{PathRejection, QueryRejection},
    },
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

/// A malformed address (syntax only, nothing looked up).
struct Malformed;

/// An optional slug parameter: empty means absent (the form's "All types"); anything else
/// must be a registry id.
fn slug(value: Option<String>) -> Result<Option<String>, Malformed> {
    match value {
        Some(value) if value.is_empty() => Ok(None),
        Some(value) if crate::registry::valid_id(&value) => Ok(Some(value)),
        Some(_) => Err(Malformed),
        None => Ok(None),
    }
}

fn browse_query(params: BrowseParams) -> Result<BrowseQuery, Malformed> {
    let q = params
        .q
        .map(|q| q.trim().to_owned())
        .filter(|q| !q.is_empty());
    if q.as_ref()
        .is_some_and(|q| q.chars().count() > read::MAX_QUERY_SCALARS)
    {
        return Err(Malformed);
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
    path: Result<Path<(i64, String)>, PathRejection>,
    query: Result<Query<BrowseParams>, QueryRejection>,
    Extension(admission): Extension<RuntimeAdmission>,
) -> ResultResponse {
    let (Ok(Path((id, catalogue))), Ok(Query(params))) = (path, query) else {
        return notice(&Notice::Malformed);
    };
    if !crate::registry::valid_id(&catalogue) {
        return notice(&Notice::Malformed);
    }
    let Ok(query) = browse_query(params) else {
        return notice(&Notice::Malformed);
    };
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
    path: Result<Path<(i64, String, String)>, PathRejection>,
    Extension(admission): Extension<RuntimeAdmission>,
) -> ResultResponse {
    let Ok(Path((id, catalogue, key))) = path else {
        return notice(&Notice::Malformed);
    };
    if !crate::registry::valid_id(&catalogue) || !crate::registry::valid_id(&key) {
        return notice(&Notice::Malformed);
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
    let refused = || {
        notice(&Notice::Refused {
            sheet_href: format!("/private-sheets/{id}"),
        })
    };
    let auth = match auth::request_auth(&state, &session).await {
        Ok(auth) => auth,
        // The production Owner reads no sheet: the same refusal as any other reader who
        // cannot read this one. Other statuses (an ended session) keep their meaning.
        Err(StatusCode::FORBIDDEN) => return refused(),
        Err(status) => return Err(status),
    };
    let (page, permit) = match state
        .store()
        .private_catalogue(auth, id, catalogue, request, permit)
        .await
    {
        Ok(found) => found,
        // Every refusal reaches here as `NotFound` and answers the same page.
        Err(AppError::NotFound) => return refused(),
        Err(other) => return Err(error(other)),
    };
    // Rendering keeps the runtime permit (R6 phase 4).
    // The blocking pool does not carry the request's task-local account context.
    let nav = crate::auth::nav::Nav::here_strict(crate::auth::nav::Section::Sheets);
    let (status, html) = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        render(nav, &page, &echo)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;
    let mut response = (status, Html(html)).into_response();
    if status == StatusCode::UNPROCESSABLE_ENTITY {
        response.extensions_mut().insert(PrivateTrustedEditor);
    }
    if status == StatusCode::NOT_FOUND {
        response.extensions_mut().insert(CatalogueNotice);
    }
    Ok(response)
}

/// The routes' explanatory error pages (plan §19).
enum Notice<'a> {
    /// 400: the address is malformed; nothing was looked up.
    Malformed,
    /// 403: the uniform refusal, the same for every cause.
    Refused { sheet_href: String },
    /// 404: no such entry in a catalogue the reader may browse.
    NoEntry(Missing404<'a>),
    /// 404: no such entry type in a catalogue the reader may browse.
    NoType(Missing404<'a>),
}

struct Missing404<'a> {
    sheet_href: String,
    browse_href: String,
    title: &'a str,
    key: &'a str,
}

/// The notice kinds the template distinguishes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NoticeKind {
    Malformed,
    Refused,
    NoEntry,
    NoType,
}

#[derive(Template)]
#[template(path = "catalogue_notice.html")]
struct NoticeTemplate<'a> {
    heading: &'static str,
    kind: NoticeKind,
    sheet_href: &'a str,
    browse_href: &'a str,
    title: &'a str,
    key: &'a str,
}

fn notice_page(notice: &Notice) -> (StatusCode, askama::Result<String>) {
    let (status, heading, kind, sheet_href, missing) = match notice {
        Notice::Malformed => (
            StatusCode::BAD_REQUEST,
            "Invalid catalogue address",
            NoticeKind::Malformed,
            "",
            None,
        ),
        Notice::Refused { sheet_href } => (
            StatusCode::FORBIDDEN,
            "Catalogue unavailable",
            NoticeKind::Refused,
            sheet_href.as_str(),
            None,
        ),
        Notice::NoEntry(missing) => (
            StatusCode::NOT_FOUND,
            "Entry not found",
            NoticeKind::NoEntry,
            missing.sheet_href.as_str(),
            Some(missing),
        ),
        Notice::NoType(missing) => (
            StatusCode::NOT_FOUND,
            "Entry type not found",
            NoticeKind::NoType,
            missing.sheet_href.as_str(),
            Some(missing),
        ),
    };
    let html = NoticeTemplate {
        heading,
        kind,
        sheet_href,
        browse_href: missing.map_or("", |missing| missing.browse_href.as_str()),
        title: missing.map_or("", |missing| missing.title),
        key: missing.map_or("", |missing| missing.key),
    }
    .render();
    (status, html)
}

/// A 400 or 403 notice, kept by the auth layer.
fn notice(notice: &Notice) -> ResultResponse {
    let (status, html) = notice_page(notice);
    let html = html.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut response = (status, Html(html)).into_response();
    response.extensions_mut().insert(CatalogueNotice);
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
    nav: crate::auth::nav::Nav,
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
    nav: crate::auth::nav::Nav,
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
    nav: crate::auth::nav::Nav,
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
        nav,
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

fn render(
    nav: crate::auth::nav::Nav,
    page: &CataloguePage,
    echo: &Echo,
) -> Result<(StatusCode, String), StatusCode> {
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
                    nav,
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
                nav,
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
        Body::Entry(entry) => (
            StatusCode::OK,
            entry_page(nav, page, entry, sheet_href, base),
        ),
        Body::NoEntry(key) => notice_page(&Notice::NoEntry(Missing404 {
            sheet_href,
            browse_href: base,
            title: &page.title,
            key,
        })),
        Body::NoType(key) => notice_page(&Notice::NoType(Missing404 {
            sheet_href,
            browse_href: base,
            title: &page.title,
            key,
        })),
    };
    let (status, html) = rendered;
    Ok((status, html.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?))
}
