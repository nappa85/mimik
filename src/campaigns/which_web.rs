//! The "Which one?" route (assisted sheet step 4, plan item 6): `GET
//! /private-sheets/{id}/cells/{f}/{r}/{p}/which?base=…`, the candidates of a saved name cell
//! that several catalogue entries share (linking rule 3). Without script it is its own page, a
//! form per candidate posting `csrf`, `base`, `value`, `set` (an optional name) and `entry` to
//! the cell route; with `Accept: application/json` it answers the list for the sheet module.
//!
//! Under the `runtime` layer like the catalogue pages (`JOBS`, 503 when busy). The current
//! revision is read as R6/R25 prescribe (two transactions, the pin validated outside the
//! mutex, a save in between 409). The row index is mapped to its host at `base` (ASSIST-6);
//! a host that is gone, whose name's norm changed since, or that no longer is under rule 3
//! answers 404 with a link back, never another row's candidates. Malformed addresses are 400;
//! every other refusal, the editor right included, is the uniform catalogue 403 (D11).
use super::{CatalogueNotice, RuntimeAdmission, SheetPage, error, sheet_json};
use crate::{
    AppError,
    auth::{self, AuthState},
    campaigns::{
        catalogue::{Candidate, Match},
        rows,
        snapshots::PrivateSnapshot,
        unit::Unit,
        values,
    },
};
use askama::Template;
use axum::{
    Extension,
    extract::{
        Path, Query, State,
        rejection::{PathRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;
use tower_sessions::Session;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WhichParams {
    base: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Malformed,
    Refused,
    Gone,
    List,
}

/// One candidate as the page and the JSON answer carry it.
struct Shown {
    key: String,
    type_label: String,
    hint: String,
    open: String,
}

#[derive(Template)]
#[template(path = "catalogue_which.html")]
struct WhichTemplate {
    kind: Kind,
    heading: &'static str,
    sheet_href: String,
    back_href: String,
    action: String,
    csrf: String,
    revision: i64,
    name: String,
    optional: bool,
    candidates: Vec<Shown>,
    more: bool,
    browse: String,
}

impl WhichTemplate {
    fn of(kind: Kind, heading: &'static str, sheet: i64) -> Self {
        Self {
            kind,
            heading,
            sheet_href: format!("/private-sheets/{sheet}"),
            back_href: format!("/private-sheets/{sheet}"),
            action: String::new(),
            csrf: String::new(),
            revision: 0,
            name: String::new(),
            optional: false,
            candidates: Vec::new(),
            more: false,
            browse: String::new(),
        }
    }
}

fn page(status: StatusCode, template: &WhichTemplate) -> super::ResultResponse {
    let html = template
        .render()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut response = (status, Html(html)).into_response();
    if status != StatusCode::OK {
        response.extensions_mut().insert(CatalogueNotice);
    }
    Ok(response)
}

fn malformed() -> super::ResultResponse {
    page(
        StatusCode::BAD_REQUEST,
        &WhichTemplate::of(Kind::Malformed, "Invalid address", 0),
    )
}

fn refused(sheet: i64) -> super::ResultResponse {
    page(
        StatusCode::FORBIDDEN,
        &WhichTemplate::of(Kind::Refused, "Catalogue unavailable", sheet),
    )
}

fn gone(sheet: i64, (f, r, p): (usize, usize, usize)) -> super::ResultResponse {
    let mut template = WhichTemplate::of(Kind::Gone, "This row has changed", sheet);
    template.back_href = format!("/private-sheets/{sheet}#label-{f}-{r}-{p}");
    page(StatusCode::NOT_FOUND, &template)
}

/// Why a row has no candidates: the uniform refusal, or "this row changed".
enum Unlisted {
    Refused,
    Gone,
}

/// The row a request names, once every check has passed.
struct Row {
    /// Its index in the live revision.
    live: usize,
    /// The name property's index and whether it is optional.
    name: (usize, bool),
    /// The saved name.
    saved: String,
    member: crate::definitions::CatalogueMember,
}

/// Maps the address to its row: an editor, a runtime pin and a catalogue-backed name (else the
/// uniform refusal); then the row at `base` found in the live revision by its host ID, with the
/// same name norm and still under rule 3 (else gone).
fn row(
    view: &crate::campaigns::sheets::SheetView,
    at_base: Option<&PrivateSnapshot>,
    (f, r, p): (usize, usize, usize),
) -> Result<Row, Unlisted> {
    let pin = view
        .immutable
        .as_ref()
        .filter(|_| view.editable)
        .ok_or(Unlisted::Refused)?;
    let definition = pin.definition();
    let unit = Unit::cell(definition, (&f.to_string(), &r.to_string(), &p.to_string()))
        .ok_or(Unlisted::Refused)?;
    let (_, name) = unit.entry_target(definition).ok_or(Unlisted::Refused)?;
    let list = &definition.fields[f];
    let property = &values::properties(list)[name];
    let member = property.catalogue.clone().ok_or(Unlisted::Refused)?;
    let base = at_base.ok_or(Unlisted::Gone)?;
    let host = rows::host(pin, base, (f, r)).ok_or(Unlisted::Gone)?;
    let live = rows::host_index(pin, &view.snapshot, f, &host).ok_or(Unlisted::Gone)?;
    let text = |snapshot: &PrivateSnapshot, at: usize| {
        snapshot.data[&list.id][at][&property.id]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    let saved = text(&view.snapshot, live);
    if !values::keeps_key(&text(base, r), &saved) {
        return Err(Unlisted::Gone);
    }
    let at = (f, host);
    match view.entry_matches.get(&at) {
        Some(Match::Several) => {}
        // Linked now, or no longer several: the row changed.
        Some(_) => return Err(Unlisted::Gone),
        None if view.entry_links.contains_key(&at) => return Err(Unlisted::Gone),
        // Neither a link nor an outcome: the lookup did not run (no sealed version, or it
        // failed), which is the uniform refusal, not a changed row.
        None => return Err(Unlisted::Refused),
    }
    Ok(Row {
        live,
        name: (name, property.optional),
        saved,
        member,
    })
}

pub(super) async fn which(
    State(state): State<AuthState>,
    session: Session,
    path: Result<Path<(i64, String, String, String)>, PathRejection>,
    query: Result<Query<WhichParams>, QueryRejection>,
    headers: HeaderMap,
    Extension(admission): Extension<RuntimeAdmission>,
) -> super::ResultResponse {
    let (Ok(Path((id, f, r, p))), Ok(Query(params))) = (path, query) else {
        return malformed();
    };
    let (Some(f), Some(r), Some(p), Some(base)) = (
        values::index(&f),
        values::index(&r),
        values::index(&p),
        values::integer(&params.base).ok().filter(|base| *base >= 1),
    ) else {
        return malformed();
    };
    let permit = admission.take()?;
    let auth = match auth::request_auth(&state, &session).await {
        Ok(auth) => auth,
        Err(StatusCode::FORBIDDEN) => return refused(id),
        Err(status) => return Err(status),
    };
    let csrf = auth::token(&session)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let (view, at_base, permit) = match state
        .store()
        .private_read_base(auth.clone(), id, Some(base), permit)
        .await
    {
        Ok(read) => read,
        // A sheet that cannot be read as a runtime sheet (a corrupt, unsupported or native
        // pin, or damaged stored data) is the same uniform refusal as an unreadable one.
        Err(AppError::NotFound | AppError::Invalid(_) | AppError::SheetData(_)) => {
            return refused(id);
        }
        Err(other) => return Err(error(other)),
    };
    let row = match row(&view, at_base.as_ref(), (f, r, p)) {
        Ok(row) => row,
        Err(Unlisted::Refused) => return refused(id),
        Err(Unlisted::Gone) => return gone(id, (f, r, p)),
    };
    let revision = view.snapshot.revision;
    let pair = (
        view.snapshot.game_id.clone(),
        view.snapshot.edition_id.clone(),
    );
    let member = (
        row.member.id.clone(),
        row.member.entry_type.clone(),
        crate::text_fold::name_norm(&row.saved),
    );
    let ((candidates, more), permit) = match state
        .store()
        .which_candidates(auth, (id, revision), pair, member, permit)
        .await
    {
        Ok(found) => found,
        Err(AppError::NotFound) => return refused(id),
        Err(other) => return Err(error(other)),
    };
    if candidates.is_empty() {
        return gone(id, (f, r, p));
    }
    let catalogue = row.member.id.clone();
    let mut template = WhichTemplate::of(Kind::List, "Which one?", id);
    let (live, (name, optional)) = (row.live, row.name);
    template.back_href = format!("/private-sheets/{id}#label-{f}-{live}-{name}");
    template.action = format!("/private-sheets/{id}/cells/{f}/{live}/{name}");
    template.csrf = csrf;
    template.revision = revision;
    template.name = row.saved;
    template.optional = optional;
    template.more = more;
    template.browse = browse_href(id, (&catalogue, row.member.entry_type.as_deref()));
    template.candidates = candidates
        .into_iter()
        .map(
            |Candidate {
                 key,
                 type_label,
                 hint,
             }| Shown {
                open: format!("/private-sheets/{id}/catalogue/{catalogue}/{key}"),
                key,
                type_label,
                hint,
            },
        )
        .collect();
    let json = sheet_json::wanted(&headers);
    // Rendering and the JSON body keep the runtime permit (R6 phase 4).
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        if json {
            Ok(json_list(&template))
        } else {
            page(StatusCode::OK, &template)
        }
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
}

/// The JSON answer of the list for the sheet module.
fn json_list(template: &WhichTemplate) -> Response {
    let body = json!({
        "status": "ok",
        "candidates": template.candidates.iter().map(|one| json!({
            "key": one.key, "type": one.type_label, "hint": one.hint, "open": one.open,
        })).collect::<Vec<_>>(),
        "more": template.more,
        "browse": template.more.then_some(&template.browse),
    });
    let mut response = (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response();
    response.extensions_mut().insert(SheetPage);
    response
}

/// The catalogue's browse page, filtered by the member's entry type when it has one.
pub(crate) fn browse_href(sheet: i64, (catalogue, entry_type): (&str, Option<&str>)) -> String {
    match entry_type {
        Some(entry_type) => {
            format!("/private-sheets/{sheet}/catalogue/{catalogue}?type={entry_type}")
        }
        None => format!("/private-sheets/{sheet}/catalogue/{catalogue}"),
    }
}
