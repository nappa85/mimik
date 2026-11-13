//! Owner-only source comparison and explicitly attributed review forms.
use super::{AuthResult, AuthState, csrf_admission, owner_auth};
use crate::sources::agreement_view::{AgreementView, Show, Status, Tab};
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
        .route("/admin/materials/pages/{id}/merges/new", get(merge_form))
        .route(
            "/admin/materials/pages/{page}/units/{unit}/split",
            get(split_form),
        )
        .route(
            "/admin/materials/pages/{id}/selection/image",
            get(selection_image),
        )
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
                .route("/admin/materials/pages/{id}/merges", post(create_merge))
                .route(
                    "/admin/materials/pages/{page}/units/{unit}/split",
                    post(create_split),
                )
                .route(
                    "/admin/materials/pages/{page}/units/{unit}/split/areas",
                    post(split_areas),
                )
                .route("/admin/materials/pages/{id}/batch", post(batch_accept))
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
    /// An owner merge the Owner just saved (checked before it is announced).
    merge: i64,
    /// The first unit of a split the Owner just saved (checked before it is announced).
    split: i64,
    /// The filter tab (`attention`, `matching`, `reviewed`, `all`).
    show: Show,
    /// A unit the Owner asked for by link: a tab that would hide it shows `all`.
    unit: i64,
    /// Units a batch accept just saved (checked before it is announced).
    batch: i64,
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
    /// A page-notes window past the first was asked for: keep that task open.
    notes_open: bool,
    /// The agreement with the PDF text layer (an unavailable one when not computed).
    agreement: AgreementView,
    /// The tab the cards show, and why it is not the requested one.
    tab: Tab,
    tab_notice: Option<&'static str>,
    /// The bounded differences of the cards shown.
    shown: std::collections::BTreeMap<i64, crate::sources::agreement_view::ShownDiff>,
    /// The header's next-unit target on a filtered tab (unit, window cursor).
    next_override: Option<(i64, i64)>,
    /// The matching tab is a compact list, not cards.
    matching_list: bool,
    matching_rows: Vec<MatchingRow>,
    matching_next: Option<i64>,
    /// The batch accept form of the agreement bar (an independent page with matching units).
    batch: Option<BatchOffer>,
    /// A refused batch: why, and the Owner's rationale text to keep.
    batch_message: String,
    batch_text: Option<String>,
    /// A saved batch (checked before it is announced).
    batch_notice: String,
}
/// What the batch form carries and shows.
struct BatchOffer {
    revision: i64,
    words: i64,
    /// `id:revision` pairs, comma separated.
    value: String,
    ids: Vec<i64>,
    /// The first [`BATCH_LISTED`] of `ids`, shown as the list of numbers.
    listed: Vec<i64>,
    total: usize,
    more: usize,
    prefix: &'static str,
    text: String,
}
/// Characters of the Owner's rationale a refused batch gives back (the store's limit).
const BATCH_ECHO_CHARS: usize = 5_000;
/// Unit numbers the batch form lists; the hidden field carries them all.
const BATCH_LISTED: usize = 100;
/// The Owner's prefilled batch rationale (D2).
const BATCH_TEXT: &str = "matches the PDF text layer word for word; checked against the page image";
/// One row of the compact matching list.
struct MatchingRow {
    id: i64,
    kind: String,
    first_line: String,
    link: String,
}
/// A tab link of the agreement bar.
struct TabLink {
    label: &'static str,
    href: String,
    count: Option<usize>,
    current: bool,
}
impl Editor {
    /// The summary counts of the agreement bar.
    fn counts(&self) -> crate::sources::agreement_view::Counts {
        self.agreement.counts()
    }
    fn kind_label(&self) -> &'static str {
        self.agreement
            .kind()
            .map_or("", crate::sources::agreement_view::kind_label)
    }
    fn no_result(&self) -> &'static str {
        crate::sources::agreement_view::status_reason(self.agreement.status).unwrap_or("")
    }
    fn tabs(&self) -> Vec<TabLink> {
        let counted = !self.screen.reads_reduced();
        let mut tabs = Vec::new();
        let mut push = |tab: Tab, count: Option<usize>| {
            tabs.push(TabLink {
                label: tab.label(),
                href: format!("/admin/materials/pages/{}?show={}", self.page.id, tab.key()),
                count: count.filter(|_| counted),
                current: self.tab == tab,
            });
        };
        if self.agreement.has_result() {
            push(
                Tab::Attention,
                Some(self.agreement.tab_count(Tab::Attention)),
            );
            push(Tab::Matching, Some(self.agreement.tab_count(Tab::Matching)));
        }
        push(
            Tab::Reviewed,
            usize::try_from(self.screen.units_reviewed).ok(),
        );
        push(Tab::All, usize::try_from(self.screen.units_total).ok());
        tabs
    }
    /// The text-layer words in no unit as one path of at most 2,000 boxes (about 30 bytes each,
    /// so the drawing's worst case is fixed and small).
    fn orphan_path(&self) -> String {
        use std::fmt::Write;
        let mut path = String::new();
        for o in self
            .agreement
            .orphans
            .iter()
            .take(crate::sources::agreement_view::MAX_ORPHANS_DRAWN)
        {
            let _ = write!(
                path,
                "M{:.1} {:.1}h{:.1}v{:.1}h-{:.1}z",
                o[0],
                o[1],
                o[2] - o[0],
                o[3] - o[1],
                o[2] - o[0]
            );
        }
        path
    }
    fn orphans_drawn(&self) -> usize {
        self.agreement
            .orphans
            .len()
            .min(crate::sources::agreement_view::MAX_ORPHANS_DRAWN)
    }
    fn diff_too_long(&self, unit: i64) -> bool {
        self.agreement.diff_too_long(unit)
    }
    fn orphans_not_drawn(&self) -> usize {
        self.agreement
            .orphans
            .len()
            .saturating_sub(crate::sources::agreement_view::MAX_ORPHANS_DRAWN)
    }
    /// Verdict class and text of a card's header (none for a decided unit).
    fn verdict(&self, unit: i64) -> Option<(&'static str, &'static str)> {
        self.agreement.label(unit)
    }
    fn diff(&self, unit: i64) -> Option<&crate::sources::agreement_view::ShownDiff> {
        self.shown.get(&unit)
    }
    /// The header's next-unit link: the tab's own next unit, else the next unreviewed one.
    fn next_link(&self) -> Option<String> {
        self.next_override
            .or(self.screen.next_unit)
            .map(|target| self.unit_link(&target))
    }
    /// The notice of an owner table, merge or split just saved (each already verified).
    fn announce(&mut self, created: i64, merged: i64, split: i64) {
        if created > 0 {
            self.notice = format!(
                "Created table unit {created}; the selected units are rejected. Check the new table against the original page before accepting it."
            );
            self.notice_unit = created;
        }
        if merged > 0 {
            self.notice = format!(
                "Merged into unit {merged}; the selected units are rejected and the new unit holds your text as a correction. Check it against the original page."
            );
            self.notice_unit = merged;
        }
        if split > 0 {
            self.notice = format!(
                "Split: unit {split} is the first of the new units; the original unit is rejected and each new unit holds your text as a correction. Check them against the original page."
            );
            self.notice_unit = split;
        }
    }
    /// Install the agreement and the tab chosen for it.
    fn with_agreement(
        mut self,
        agreement: AgreementView,
        tab: Tab,
        notice: Option<&'static str>,
    ) -> Self {
        self.agreement = agreement;
        self.tab = tab;
        self.tab_notice = notice;
        self.shown = self
            .agreement
            .diffs(&self.page.units.iter().map(|u| u.id).collect::<Vec<_>>());
        self.batch = self.batch_offer();
        self
    }
    /// The batch form: offered only when the agreement allows it and a unit matches. Sets
    /// over the atomic bound go in parts, the first part first.
    fn batch_offer(&self) -> Option<BatchOffer> {
        use crate::sources::agreement_view::BATCH_MAX_UNITS;
        if !self.agreement.offers_batch() {
            return None;
        }
        let matching: Vec<_> = self
            .agreement
            .units
            .iter()
            .filter(|u| AgreementView::in_tab(u, Tab::Matching))
            .collect();
        if matching.is_empty() {
            return None;
        }
        let part = &matching[..matching.len().min(BATCH_MAX_UNITS)];
        Some(BatchOffer {
            revision: self.agreement.page_revision,
            words: self.agreement.words_len,
            value: part
                .iter()
                .map(|u| format!("{}:{}", u.id, u.revision))
                .collect::<Vec<_>>()
                .join(","),
            ids: part.iter().map(|u| u.id).collect(),
            listed: part.iter().take(BATCH_LISTED).map(|u| u.id).collect(),
            total: matching.len(),
            more: matching.len() - part.len(),
            prefix: crate::sources::agreement_view::BATCH_PREFIX,
            text: self
                .batch_text
                .clone()
                .unwrap_or_else(|| BATCH_TEXT.to_owned()),
        })
    }
    /// The screen card of a unit shown on this window.
    fn card(&self, unit: i64) -> Option<&crate::sources::review::Card> {
        self.screen.cards.iter().find(|card| card.unit == unit)
    }
    /// The refused decision of this unit, if its card is the error card.
    fn draft_for(&self, unit: i64) -> Option<&Draft> {
        if self.error_unit == unit {
            self.draft.as_ref()
        } else {
            None
        }
    }
    /// The revision all three forms of a card carry: the submitted one on the error card (so a
    /// stale page cannot be decided without reloading), else the stored one.
    fn revision_for(&self, unit: &crate::sources::UnitView) -> i64 {
        self.draft_for(unit.id)
            .map_or(unit.revision, |draft| draft.revision)
    }
    /// The Correct form's text: the submitted draft on a refused correction, else the stored
    /// correction of a corrected unit, else the extracted text.
    fn correct_text(&self, unit: &crate::sources::UnitView) -> String {
        match self.draft_for(unit.id) {
            Some(draft) if draft.refills("corrected") => draft.correction.clone(),
            _ if unit.state == "corrected" => unit.correction.clone(),
            _ => unit.raw_text.clone().unwrap_or_default(),
        }
    }
    /// The extracted text is too long to prefill the Correct form (and no draft refills it).
    fn prefill_missing(&self, unit: &crate::sources::UnitView) -> bool {
        unit.state != "corrected"
            && unit.raw_text.is_none()
            && !self
                .draft_for(unit.id)
                .is_some_and(|draft| draft.refills("corrected"))
    }
    /// The rationale a decision's form is refilled with after a refusal of that decision.
    fn rationale_for(&self, unit: i64, state: &str) -> String {
        self.draft_for(unit)
            .filter(|draft| draft.refills(state))
            .map_or_else(String::new, |draft| draft.rationale.clone())
    }
    /// Whether the refused decision of this card is `state` (its disclosure opens).
    fn refused(&self, unit: i64, state: &str) -> bool {
        self.draft_for(unit)
            .is_some_and(|draft| draft.state == state)
    }
    fn correction_invalid(&self, unit: i64) -> bool {
        self.draft_for(unit).is_some_and(Draft::correction_missing)
    }
    fn rationale_invalid(&self, unit: i64, state: &str) -> bool {
        self.draft_for(unit)
            .is_some_and(|draft| draft.state == state && draft.rationale_missing())
    }
    /// The stored decision of the refused form is not one of the three (forged request).
    fn forged_state(&self, unit: i64) -> Option<String> {
        self.draft_for(unit)
            .filter(|draft| !matches!(draft.state.as_str(), "accepted" | "corrected" | "rejected"))
            .map(|draft| draft.state.clone())
    }
    /// Every unit of the page is decided (known only from a complete read).
    fn all_reviewed(&self) -> bool {
        !self.screen.reads_reduced() && self.screen.next_unit.is_none()
    }
    fn unit_link(&self, target: &(i64, i64)) -> String {
        let (unit, cursor) = target;
        if self.page.units.iter().any(|row| row.id == *unit) {
            format!("#unit-{unit}")
        } else {
            format!(
                "?units={cursor}&unit={unit}&show={}#unit-{unit}",
                self.tab.key()
            )
        }
    }
    fn region_link(&self, region: &crate::sources::review::Overlay) -> String {
        if region.unit > 0 {
            let (unit, cursor) = (region.unit, region.cursor);
            if self.page.units.iter().any(|row| row.id == unit) {
                format!("#unit-{unit}")
            } else {
                // A short link: 2,000 overlay links share the response budget.
                format!("?units={cursor}&unit={unit}#unit-{unit}")
            }
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
        let unit = if self.draft.is_some() {
            format!(
                "&unit={}",
                self.page.units.first().map_or(0, |unit| unit.id)
            )
        } else {
            String::new()
        };
        format!(
            "/admin/materials/pages/{}?units={}{unit}&show=all#{anchor}",
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
            notes_open: c.notes > 0,
            agreement: AgreementView::unavailable(Status::TimedOut),
            tab: Tab::All,
            tab_notice: None,
            shown: std::collections::BTreeMap::new(),
            next_override: None,
            matching_list: false,
            matching_rows: Vec::new(),
            matching_next: None,
            batch: None,
            batch_message: String::new(),
            batch_text: None,
            batch_notice: String::new(),
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
/// The agreement of a page; a failed read is "no comparison", never a failed page.
async fn view_of(state: &AuthState, auth: super::RequestAuth, page: i64) -> AgreementView {
    match state.sessions.store.material_agreement(auth, page).await {
        Ok(view) => view,
        Err(e) => {
            eprintln!("Mimik: agreement for page {page} failed: {e}");
            AgreementView::unavailable(Status::Unreadable)
        }
    }
}
/// The compact list of the matching tab: a window of rows after the cursor, and the cursor
/// of the next window.
fn matching_rows(view: &AgreementView, page: i64, after: i64) -> (Vec<MatchingRow>, Option<i64>) {
    let limit = crate::sources::materials::REVIEW_LIST_UNITS;
    let mut rows: Vec<&crate::sources::agreement_view::UnitAgreement> = view
        .units
        .iter()
        .filter(|u| u.position > after && AgreementView::in_tab(u, Tab::Matching))
        .take(limit + 1)
        .collect();
    let more = rows.len() > limit;
    rows.truncate(limit);
    let next = more.then(|| rows.last().map(|u| u.position)).flatten();
    let rows = rows
        .into_iter()
        .map(|u| MatchingRow {
            id: u.id,
            kind: u.kind.clone(),
            first_line: u.first_line.clone(),
            link: format!(
                "/admin/materials/pages/{page}?units={}&unit={}&show=all#unit-{}",
                u.position - 1,
                u.id,
                u.id
            ),
        })
        .collect();
    (rows, next)
}
/// The window the tab lists, and whether it is the compact matching list (no cards).
fn window_filter(
    view: &AgreementView,
    tab: Tab,
) -> (crate::sources::materials::WindowFilter, bool) {
    use crate::sources::materials::WindowFilter;
    if tab == Tab::Matching && view.offers_batch() {
        return (WindowFilter::Ids(Vec::new()), true);
    }
    let filter = match tab {
        Tab::Attention | Tab::Matching => WindowFilter::Ids(view.ids(tab)),
        Tab::Reviewed => WindowFilter::Reviewed,
        Tab::All => WindowFilter::All,
    };
    (filter, false)
}
/// The owner table, merge and split a redirect announces, each checked against the store.
async fn verified_replacements(
    state: &AuthState,
    auth: &super::RequestAuth,
    page: i64,
    c: PageCursor,
) -> (i64, i64, i64) {
    let mut found = [0; 3];
    for (slot, (unit, kind)) in
        found
            .iter_mut()
            .zip([(c.table, "table"), (c.merge, "merge"), (c.split, "split")])
    {
        if unit > 0
            && state
                .sessions
                .store
                .material_is_owner_replacement(auth.clone(), page, unit, kind)
                .await
                .unwrap_or(false)
        {
            *slot = unit;
        }
    }
    (found[0], found[1], found[2])
}
async fn page(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    Query(mut c): Query<PageCursor>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let (created, merged, split) = verified_replacements(&state, &auth, id, c).await;
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
    // The agreement is computed before the card window is read: the tab decides which units
    // the window lists.
    let view = view_of(&state, auth.clone(), id).await;
    let focus = if c.unit > 0 { c.unit } else { c.to };
    // Without a result the view knows no unit: ask the unit itself whether it is decided.
    let focus_reviewed = if focus > 0 && !view.has_result() {
        state
            .sessions
            .store
            .material_unit_state(auth.clone(), id, focus)
            .await
            .ok()
            .map(|before| before != "unreviewed")
    } else {
        None
    };
    let (tab, tab_notice) = view.resolve(
        c.show,
        c.units > 0,
        focus,
        view.offers_batch(),
        focus_reviewed,
    );
    let (filter, matching_list) = window_filter(&view, tab);
    let data = state
        .sessions
        .store
        .material_page_filtered(auth.clone(), id, c.units, c.history, filter)
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
    let next_override = matches!(tab, Tab::Attention | Tab::Matching)
        .then(|| view.next_in(tab, 0).map(|u| (u.id, u.position - 1)))
        .flatten();
    let (matching_rows, matching_next) = if matching_list {
        matching_rows(&view, id, c.units)
    } else {
        (Vec::new(), None)
    };
    let mut editor = Editor::new(data, screen, csrf, c).with_agreement(view, tab, tab_notice);
    editor.proposals = proposals;
    editor.next_override = next_override;
    editor.matching_list = matching_list;
    editor.matching_rows = matching_rows;
    editor.matching_next = matching_next;
    editor.announce(created, merged, split);
    editor.batch_notice = verified_batch_notice(&state, &auth, id, c.batch).await;
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
    crop_response(&state, auth, input).await
}
/// The reference image of the table and merge forms: the page cropped to the area the selected
/// units would cover.
async fn selection_image(
    State(state): State<AuthState>,
    session: Session,
    Path(page): Path<i64>,
    Query(fields): Query<Vec<(String, String)>>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let (ids, _, malformed) = table_query(&fields);
    if let Some(message) = malformed {
        return Err(error(&AppError::Invalid(message)));
    }
    let permit = crate::sources::region::admit().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let input = state
        .sessions
        .store
        .material_selection_region_data(auth.clone(), page, ids, permit)
        .await
        .map_err(|e| error(&e))?;
    crop_response(&state, auth, input).await
}
/// Crops off the async threads under the admission permit, rechecks the Owner, answers a PNG.
async fn crop_response(
    state: &AuthState,
    auth: crate::auth::RequestAuth,
    input: crate::sources::region::RegionInput,
) -> AuthResult {
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
    /// The tab the decision was made on (the redirect continues in it).
    #[serde(default)]
    show: Show,
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
    /// Whether a refused draft refills the form of `state`: its own decision's form, and the
    /// Correct form (the one with every field) for a forged decision, so no text is lost.
    fn refills(&self, state: &str) -> bool {
        self.state == state
            || (state == "corrected" && !matches!(self.state.as_str(), "accepted" | "rejected"))
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
/// The `show` a redirect carries (an absent `show` means the page's default tab).
fn carried(tab: Tab) -> String {
    format!("&show={}", tab.key())
}
fn saved_target(page: i64, unit: i64, state: &str, next: &ReviewNext, tab: Tab) -> String {
    let label = Saved::of(state).key();
    let show = carried(tab);
    match *next {
        ReviewNext::Unit { unit: to, cursor } => {
            let units = if cursor > 0 {
                format!("units={cursor}&")
            } else {
                String::new()
            };
            format!(
                "/admin/materials/pages/{page}?{units}saved={label}&saved_unit={unit}&to={to}&then=unit{show}#unit-{to}"
            )
        }
        ReviewNext::Page(next) => format!(
            "/admin/materials/pages/{next}?saved={label}&saved_unit={unit}&then=page{show}#saved-notice"
        ),
        ReviewNext::Done => format!(
            "/admin/materials/pages/{page}?saved={label}&saved_unit={unit}&then=done#complete"
        ),
        ReviewNext::Unknown => format!(
            "/admin/materials/pages/{page}?saved={label}&saved_unit={unit}&to={unit}&then=unknown{show}#unit-{unit}"
        ),
    }
}
/// The redirect after re-deciding a reviewed unit: back to that unit, with its card window.
fn stay_target(page: i64, unit: i64, state: &str, cursor: i64, tab: Tab) -> String {
    let label = Saved::of(state).key();
    let units = if cursor >= 50 {
        format!("units={cursor}&")
    } else {
        String::new()
    };
    let show = carried(tab);
    format!(
        "/admin/materials/pages/{page}?{units}saved={label}&saved_unit={unit}&to={unit}&then=stay{show}#unit-{unit}"
    )
}
/// Where a first decision continues: the tab's own next unit, else the next unreviewed unit.
async fn next_after_save(
    state: &AuthState,
    auth: super::RequestAuth,
    page: i64,
    unit: i64,
    view: &AgreementView,
    tab: Tab,
) -> ReviewNext {
    if matches!(tab, Tab::Attention | Tab::Matching) {
        let after = view.unit(unit).map_or(0, |u| u.position);
        if let Some(next) = view.next_in(tab, after) {
            return ReviewNext::Unit {
                unit: next.id,
                // The default window already shows positions 1..=50.
                cursor: if next.position <= 50 {
                    0
                } else {
                    next.position - 1
                },
            };
        }
    }
    state
        .sessions
        .store
        .material_next_after_review(auth, page, unit)
        .await
        .unwrap_or(ReviewNext::Unknown)
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
            // The decision is stored; a failed lookup only loses the jump, not the save. The
            // tab the decision was made on continues: its next unit comes from the agreement
            // read of the new revision (memoised, recomputed on a miss).
            if was_reviewed
                && let Ok(cursor) = state
                    .sessions
                    .store
                    .material_unit_cursor(auth.clone(), page, unit)
                    .await
            {
                // A re-decision needs no agreement: the unit stays in view.
                let tab = form.show.tab();
                return Ok(
                    Redirect::to(&stay_target(page, unit, &form.state, cursor, tab))
                        .into_response(),
                );
            }
            let view = view_of(&state, auth.clone(), page).await;
            let (tab, _) = view.resolve(form.show, false, 0, view.offers_batch(), None);
            let next = next_after_save(&state, auth, page, unit, &view, tab).await;
            Ok(Redirect::to(&saved_target(page, unit, &form.state, &next, tab)).into_response())
        }
        Err(e @ (AppError::Invalid(_) | AppError::Conflict)) => {
            let editor_auth = auth.clone();
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
            if !editor.page.units.iter().any(|u| u.id == unit) {
                return Err(StatusCode::FORBIDDEN);
            }
            editor.message = e.to_string();
            editor.error_unit = unit;
            editor.draft = Some(form);
            // A refused save renders with every unit (so the draft binding and its link stay
            // valid); its agreement bar is computed like any view.
            let view = view_of(&state, editor_auth, page).await;
            let editor = editor.with_agreement(view, Tab::All, None);
            trusted_editor(&editor, error(&e))
        }
        Err(e) => Err(error(&e)),
    }
}
/// The notice of a batch the link claims, empty unless the page's latest batch is that size.
async fn verified_batch_notice(
    state: &AuthState,
    auth: &super::RequestAuth,
    page: i64,
    count: i64,
) -> String {
    if count > 0
        && state
            .sessions
            .store
            .material_batch_matches(auth.clone(), page, count)
            .await
            .unwrap_or(false)
    {
        batch_notice(count)
    } else {
        String::new()
    }
}
/// The saved-batch notice.
fn batch_notice(count: i64) -> String {
    if count == 1 {
        "Accepted 1 unit that matches the PDF text layer.".to_owned()
    } else {
        format!("Accepted {count} units that match the PDF text layer.")
    }
}
/// The batch accept form: the page's revision and stored-words size, the units with their
/// revisions, and the Owner's rationale (the server writes its prefix).
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchForm {
    csrf: String,
    revision: i64,
    words: i64,
    units: String,
    #[serde(deserialize_with = "crate::form_text::lf")]
    rationale: String,
}
/// Bytes of the `units` field: `BATCH_MAX_UNITS` pairs of two 20-character numbers (the widest
/// `i64`), a colon and a comma.
const BATCH_FIELD_BYTES: usize = crate::sources::agreement_view::BATCH_MAX_UNITS * 42;
impl BatchForm {
    fn parse_units(&self) -> Option<Vec<(i64, i64)>> {
        if self.units.len() > BATCH_FIELD_BYTES {
            return None;
        }
        let mut units = Vec::new();
        for pair in self.units.split(',') {
            let (id, revision) = pair.split_once(':')?;
            units.push((id.parse().ok()?, revision.parse().ok()?));
            if units.len() > crate::sources::agreement_view::BATCH_MAX_UNITS {
                return None;
            }
        }
        Some(units)
    }
}
async fn batch_accept(
    State(state): State<AuthState>,
    session: Session,
    Path(page): Path<i64>,
    Form(form): Form<BatchForm>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let csrf = csrf_admission(&session, &form.csrf).await?;
    let result = match form.parse_units() {
        None => Err(AppError::Invalid(
            "The batch form was incomplete or too large; reload the page.",
        )),
        Some(units) => {
            // The verdicts are recomputed from the stored rows, never read from the memo.
            let request = crate::sources::agreement_view::BatchAccept {
                page_revision: form.revision,
                words_len: form.words,
                units,
                text: form.rationale.clone(),
            };
            match state
                .sessions
                .store
                .material_agreement_fresh(auth.clone(), page)
                .await
            {
                // A read that failed is not a refusal of the form: every error maps as on the
                // other routes (a timeout is already an unavailable view).
                Err(e) => return Err(error(&e)),
                Ok(view) => match view.check_batch(&request) {
                    Err(reason) => Err(AppError::Invalid(reason)),
                    Ok(()) => {
                        state
                            .sessions
                            .store
                            .material_batch_accept_agreeing(auth.clone(), csrf, page, request)
                            .await
                    }
                },
            }
        }
    };
    match result {
        Ok(events) => Ok(Redirect::to(&format!(
            "/admin/materials/pages/{page}?batch={}#agreement-bar",
            events.len()
        ))
        .into_response()),
        Err(e @ (AppError::Invalid(_) | AppError::Conflict)) => {
            let message = match &e {
                AppError::Invalid(reason) => (*reason).to_owned(),
                other => other.to_string(),
            };
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
                    auth.clone(),
                    page,
                    data.page.units.iter().map(|u| u.id).collect(),
                    0,
                )
                .await
                .map_err(|e| error(&e))?;
            let view = view_of(&state, auth, page).await;
            let mut editor = Editor::new(data, screen, form.csrf.clone(), PageCursor::default());
            editor.batch_message = message;
            // Echo at most the length the store accepts: the field is not checked before.
            editor.batch_text = Some(form.rationale.chars().take(BATCH_ECHO_CHARS).collect());
            let editor = editor.with_agreement(view, Tab::All, None);
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
            let view = view_of(&state, auth.clone(), page).await;
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
            let mut editor = Editor::new(data, screen, form.csrf.clone(), PageCursor::default())
                .with_agreement(view, Tab::All, None);
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
    let view = view_of(state, auth.clone(), page).await;
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
    Ok(Editor::new(data, screen, csrf, PageCursor::default()).with_agreement(view, Tab::All, None))
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
    /// Reference images: the page size in PDF points and the selected units' outlines.
    /// Set only after a successful prefill with usable page geometry.
    reference: Option<Reference>,
}

struct Reference {
    pdf_page: i64,
    width: f64,
    height: f64,
    crop_url: String,
    outlines: Vec<(i64, String)>,
    /// Selected units with no four-point outline.
    missing: usize,
}

impl Reference {
    /// The reference images of a selection (`units`, the ids the crop route takes), from a
    /// prefill's page size and outlines; none without page geometry.
    fn of(
        page: i64,
        pdf_page: i64,
        page_size: Option<(f64, f64)>,
        units: &[(i64, i64)],
        outlines: &[(i64, String)],
    ) -> Option<Self> {
        page_size.map(|(width, height)| {
            let query: Vec<String> = units.iter().map(|(id, _)| format!("unit={id}")).collect();
            Self {
                pdf_page,
                width,
                height,
                crop_url: format!(
                    "/admin/materials/pages/{page}/selection/image?{}",
                    query.join("&")
                ),
                outlines: outlines.to_vec(),
                missing: units.len().saturating_sub(outlines.len()),
            }
        })
    }
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
            reference: None,
        }
    }

    /// The reference images, from the prefill's own unit ids; none without page geometry.
    fn set_outlines(&mut self, prefill: &crate::sources::materials::TablePrefill) {
        self.reference = Reference::of(
            self.page,
            prefill.pdf_page,
            prefill.page_size,
            &prefill.units,
            &prefill.outlines,
        );
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
/// The table form's query: `unit` (repeated) and at most one `shape`; nothing else is read from
/// the URL. The message is set when anything else was sent or a value does not parse.
fn table_query(fields: &[(String, String)]) -> (Vec<i64>, Option<Shape>, Option<&'static str>) {
    let mut shape = None;
    let mut ids = Vec::new();
    let mut malformed = None;
    for (key, value) in fields {
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
    (ids, shape, malformed)
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
    let (ids, shape, malformed) = table_query(&fields);
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
            editor.set_outlines(&prefill);
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
        editor.set_outlines(&prefill);
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
                "/admin/materials/pages/{page}?units={cursor}&table={unit}&unit={unit}#unit-{unit}"
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

#[derive(Template)]
#[template(path = "material_merge.html")]
struct MergeEditor {
    page: i64,
    csrf: String,
    revision: i64,
    /// Hidden `unit` fields, `id:revision`, in reading order.
    units: Vec<String>,
    text: String,
    rationale: String,
    message: String,
    /// Selected units that are tables or hold no text.
    warnings: Vec<String>,
    overlaps: String,
    overlap_unchecked: bool,
    /// The prefilled text was cut at 64 KiB.
    cut: bool,
    /// The prefilled text is over the 20,000 characters a save takes.
    too_long: bool,
    reference: Option<Reference>,
}

impl MergeEditor {
    fn new(page: i64, csrf: String) -> Self {
        Self {
            page,
            csrf,
            revision: 0,
            units: Vec::new(),
            text: String::new(),
            rationale: String::new(),
            message: String::new(),
            warnings: Vec::new(),
            overlaps: String::new(),
            overlap_unchecked: false,
            cut: false,
            too_long: false,
            reference: None,
        }
    }

    /// The notes, reference images and revisions a prefill computed; the text stays as it is.
    fn set_notes(&mut self, prefill: &crate::sources::materials::MergePrefill) {
        self.reference = Reference::of(
            self.page,
            prefill.pdf_page,
            prefill.page_size,
            &prefill.units,
            &prefill.outlines,
        );
        self.warnings.clone_from(&prefill.warnings);
        self.overlaps = overlap_note(&prefill.overlaps);
        self.overlap_unchecked = !prefill.overlap_checked;
    }
}

fn merge_editor_response(editor: &MergeEditor, status: StatusCode) -> AuthResult {
    let mut response = html(editor)?;
    *response.status_mut() = status;
    response.extensions_mut().insert(TrustedDraft);
    Ok(response)
}

/// The merge form's query: `unit` (repeated); nothing else is read from the URL.
fn merge_query(fields: &[(String, String)]) -> (Vec<i64>, Option<&'static str>) {
    const CHOOSE: &str = "Choose 2-200 different units to merge.";
    let mut ids = Vec::new();
    let mut malformed = None;
    for (key, value) in fields {
        match (key.as_str(), value.parse::<i64>()) {
            ("unit", Ok(id)) => ids.push(id),
            _ => malformed = Some(CHOOSE),
        }
    }
    (ids, malformed)
}

/// The merge form, prefilled from the units chosen with the cards' checkboxes.
async fn merge_form(
    State(state): State<AuthState>,
    session: Session,
    Path(page): Path<i64>,
    Query(fields): Query<Vec<(String, String)>>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let csrf = super::token(&session).await?.ok_or(StatusCode::FORBIDDEN)?;
    let mut editor = MergeEditor::new(page, csrf);
    let (ids, malformed) = merge_query(&fields);
    let prefill = match malformed {
        None => {
            state
                .sessions
                .store
                .material_merge_prefill(auth, page, ids)
                .await
        }
        Some(message) => Err(AppError::Invalid(message)),
    };
    match prefill {
        Ok(prefill) => {
            editor.set_notes(&prefill);
            editor.revision = prefill.page_revision;
            editor.units = prefill
                .units
                .iter()
                .map(|(u, r)| format!("{u}:{r}"))
                .collect();
            editor.cut = prefill.cut;
            editor.too_long =
                prefill.text.chars().count() > crate::sources::documents::MAX_MERGE_CHARS;
            editor.text = prefill.text;
            html(&editor)
        }
        Err(e @ AppError::Invalid(_)) => {
            editor.message = e.to_string();
            merge_editor_response(&editor, error(&e))
        }
        Err(e) => Err(error(&e)),
    }
}

async fn create_merge(
    State(state): State<AuthState>,
    session: Session,
    Path(page): Path<i64>,
    Form(fields): Form<Vec<(String, String)>>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let (mut csrf_token, mut revision, mut text, mut rationale) =
        (None, None, None, None::<String>);
    let mut units = Vec::new();
    let mut malformed = false;
    for (key, value) in fields {
        let value = crate::form_text::newlines_to_lf(value);
        match key.as_str() {
            "csrf" if csrf_token.is_none() => csrf_token = Some(value),
            "revision" if revision.is_none() => revision = value.parse::<i64>().ok(),
            "text" if text.is_none() => text = Some(value),
            "rationale" if rationale.is_none() => rationale = Some(value),
            "unit" => units.push(value),
            _ => malformed = true,
        }
    }
    let csrf_token = csrf_token.ok_or(StatusCode::FORBIDDEN)?;
    let csrf = csrf_admission(&session, &csrf_token).await?;
    let mut editor = MergeEditor {
        revision: revision.unwrap_or(0),
        units,
        text: text.unwrap_or_default(),
        rationale: rationale.unwrap_or_default(),
        ..MergeEditor::new(page, csrf_token)
    };
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
            state
                .sessions
                .store
                .material_create_merge(
                    auth.clone(),
                    csrf,
                    page,
                    crate::sources::OwnerMerge {
                        revision: editor.revision,
                        sources,
                        text: editor.text.clone(),
                        actor: String::new(),
                        rationale: editor.rationale.clone(),
                    },
                )
                .await
        }
        _ => Err(AppError::Invalid(
            "The merge form was incomplete; reopen it from the page.",
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
                "/admin/materials/pages/{page}?units={cursor}&merge={unit}&unit={unit}#unit-{unit}"
            ))
            .into_response())
        }
        Err(e @ (AppError::Invalid(_) | AppError::Conflict)) => {
            editor.message = e.to_string();
            let ids: Vec<i64> = editor
                .units
                .iter()
                .filter_map(|u| u.split_once(':')?.0.parse().ok())
                .collect();
            if let Ok(prefill) = state
                .sessions
                .store
                .material_merge_prefill(auth, page, ids)
                .await
            {
                editor.set_notes(&prefill);
            }
            merge_editor_response(&editor, error(&e))
        }
        Err(e) => Err(error(&e)),
    }
}

#[derive(Template)]
#[template(path = "material_split.html")]
struct SplitEditor {
    page: i64,
    unit: i64,
    csrf: String,
    /// The page and unit revisions the Owner saw, carried through every step.
    revision: i64,
    unit_revision: i64,
    text: String,
    rationale: String,
    message: String,
    /// The unit was read and may be split (the form shows).
    open: bool,
    pdf_page: i64,
    cut: bool,
    has_marker: bool,
    /// The crop and the page size, and the boxes (submitted values) when the areas are shown.
    view: Option<SplitView>,
    boxes: Vec<PartBox>,
    areas_note: String,
}

struct SplitView {
    width: f64,
    height: f64,
    /// The crop's box as an SVG `viewBox` in PDF points, and the size of the part numbers.
    view_box: String,
    label: String,
}

/// One part's box as the four fields hold it, with its outline.
struct PartBox {
    number: usize,
    values: [String; 4],
    points: String,
}

impl PartBox {
    fn new(number: usize, values: [String; 4]) -> Self {
        let parsed: Option<Vec<f64>> = values
            .iter()
            .map(|v| v.trim().parse::<f64>().ok().filter(|n| n.is_finite()))
            .collect();
        let points = parsed.map_or_else(String::new, |n| {
            format!(
                "{},{} {},{} {},{} {},{}",
                n[0], n[1], n[2], n[1], n[2], n[3], n[0], n[3]
            )
        });
        Self {
            number,
            values,
            points,
        }
    }
}

impl SplitEditor {
    fn new(page: i64, unit: i64, csrf: String) -> Self {
        Self {
            page,
            unit,
            csrf,
            revision: 0,
            unit_revision: 0,
            text: String::new(),
            rationale: String::new(),
            message: String::new(),
            open: false,
            pdf_page: 0,
            cut: false,
            has_marker: false,
            view: None,
            boxes: Vec::new(),
            areas_note: String::new(),
        }
    }

    /// The crop and page size of a read; nothing else is taken from it (the draft stays).
    fn set_view(&mut self, read: &crate::sources::materials::SplitRead) {
        self.pdf_page = read.pdf_page;
        self.view = read.page_size.zip(read.crop).map(|((width, height), c)| {
            let size = ((c[2] - c[0]).min(c[3] - c[1]) / 6.0).max(2.0);
            SplitView {
                width,
                height,
                view_box: format!("{} {} {} {}", c[0], c[1], c[2] - c[0], c[3] - c[1]),
                label: format!("{size:.1}"),
            }
        });
    }

    /// Show the boxes to two decimals, rounded outward (left and top down, right and
    /// bottom up) and never past the page, so a box on the page's edge still fits the page when
    /// the shown values are saved as they are.
    fn set_boxes(&mut self, boxes: &[[f64; 4]]) {
        let page = self
            .view
            .as_ref()
            .map_or((f64::MAX, f64::MAX), |v| (v.width, v.height));
        self.boxes = boxes
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let values = std::array::from_fn(|k| {
                    let limit = if k % 2 == 0 { page.0 } else { page.1 };
                    outward_value(b[k], k >= 2, limit)
                });
                PartBox::new(i + 1, values)
            })
            .collect();
    }
}

/// One box value with two decimals, rounded down (`up` false) or up, and the page's own limit
/// printed in full when rounding up would pass it.
fn outward_value(value: f64, up: bool, limit: f64) -> String {
    let scaled = value * 100.0;
    let rounded = if up { scaled.ceil() } else { scaled.floor() } / 100.0;
    if rounded > limit {
        limit.to_string()
    } else {
        format!("{:.2}", rounded.max(0.0))
    }
}

fn split_editor_response(editor: &SplitEditor, status: StatusCode) -> AuthResult {
    let mut response = html(editor)?;
    *response.status_mut() = status;
    response.extensions_mut().insert(TrustedDraft);
    Ok(response)
}

/// The split form for one unit, prefilled with its text.
async fn split_form(
    State(state): State<AuthState>,
    session: Session,
    Path((page, unit)): Path<(i64, i64)>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let csrf = super::token(&session).await?.ok_or(StatusCode::FORBIDDEN)?;
    let mut editor = SplitEditor::new(page, unit, csrf);
    match state
        .sessions
        .store
        .material_split_read(auth, page, unit, None)
        .await
    {
        Ok(read) => {
            editor.open = true;
            editor.revision = read.page_revision;
            editor.unit_revision = read.unit_revision;
            editor.cut = read.cut;
            editor.has_marker = read.has_marker;
            editor.set_view(&read);
            editor.text = read.text;
            html(&editor)
        }
        Err(e @ AppError::Invalid(_)) => {
            editor.message = e.to_string();
            split_editor_response(&editor, error(&e))
        }
        Err(e) => Err(error(&e)),
    }
}

/// The fields of the split forms. Box fields (`box_{part}_{corner}`) are kept as submitted.
struct SplitFields {
    csrf: Option<String>,
    revision: Option<i64>,
    unit_revision: Option<i64>,
    text: Option<String>,
    rationale: Option<String>,
    boxes: std::collections::BTreeMap<(usize, usize), String>,
    malformed: bool,
}

fn split_fields(fields: Vec<(String, String)>) -> SplitFields {
    let mut out = SplitFields {
        csrf: None,
        revision: None,
        unit_revision: None,
        text: None,
        rationale: None,
        boxes: std::collections::BTreeMap::new(),
        malformed: false,
    };
    for (key, value) in fields {
        let value = crate::form_text::newlines_to_lf(value);
        match key.as_str() {
            "csrf" if out.csrf.is_none() => out.csrf = Some(value),
            "revision" if out.revision.is_none() => out.revision = value.parse().ok(),
            "unit_revision" if out.unit_revision.is_none() => {
                out.unit_revision = value.parse().ok();
            }
            "text" if out.text.is_none() => out.text = Some(value),
            "rationale" if out.rationale.is_none() => out.rationale = Some(value),
            _ => {
                let corner = key
                    .strip_prefix("box_")
                    .and_then(|rest| rest.split_once('_'))
                    .and_then(|(n, k)| Some((n.parse::<usize>().ok()?, k.parse::<usize>().ok()?)))
                    .filter(|(n, k)| {
                        (1..=crate::sources::documents::MAX_SPLIT_PARTS).contains(n) && *k < 4
                    });
                match corner {
                    Some(at) if !out.boxes.contains_key(&at) => {
                        out.boxes.insert(at, value);
                    }
                    _ => out.malformed = true,
                }
            }
        }
    }
    out
}

impl SplitFields {
    /// An editor holding the submitted draft, revisions and boxes.
    fn editor(&self, page: i64, unit: i64, csrf: String) -> SplitEditor {
        let mut editor = SplitEditor::new(page, unit, csrf);
        editor.open = true;
        editor.revision = self.revision.unwrap_or(0);
        editor.unit_revision = self.unit_revision.unwrap_or(0);
        editor.text = self.text.clone().unwrap_or_default();
        editor.rationale = self.rationale.clone().unwrap_or_default();
        let parts = self.boxes.keys().map(|(n, _)| *n).max().unwrap_or(0);
        editor.boxes = (1..=parts)
            .map(|n| {
                PartBox::new(
                    n,
                    std::array::from_fn(|k| self.boxes.get(&(n, k)).cloned().unwrap_or_default()),
                )
            })
            .collect();
        editor
    }
}

/// "Show the areas": computes every part's area and shows them on the unit's crop, one box
/// editor per part. Writes nothing; the text comes in the body, never the URL.
async fn split_areas(
    State(state): State<AuthState>,
    session: Session,
    Path((page, unit)): Path<(i64, i64)>,
    Form(fields): Form<Vec<(String, String)>>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let fields = split_fields(fields);
    let token = fields.csrf.clone().ok_or(StatusCode::FORBIDDEN)?;
    csrf_admission(&session, &token).await?;
    let mut editor = fields.editor(page, unit, token);
    let submitted = std::mem::take(&mut editor.boxes);
    let checked = if fields.malformed || fields.revision.is_none() || fields.unit_revision.is_none()
    {
        Err(AppError::Invalid(
            "The split form was incomplete; reopen it from the page.",
        ))
    } else {
        crate::sources::documents::check_split_text(&editor.text)
    };
    // Boxes shown before are kept while the part count holds and each is a set of four numbers;
    // otherwise the areas are computed again from the text.
    let (keep, read) = match checked {
        Ok(parts) => {
            let keep = !submitted.is_empty()
                && submitted.len() == parts.len()
                && submitted.iter().all(|b| !b.points.is_empty());
            let read = state
                .sessions
                .store
                .material_split_read(auth.clone(), page, unit, (!keep).then_some(parts))
                .await;
            (keep, read)
        }
        Err(e) => (false, Err(e)),
    };
    match read {
        Ok(read) => {
            editor.set_view(&read);
            if keep {
                editor.boxes = submitted;
                editor.areas_note = "These are the boxes you submitted, kept because the number of parts has not changed. Change the number of parts to compute the areas again.".into();
            } else if let Some(areas) = &read.areas {
                editor.set_boxes(&areas.boxes);
                editor.areas_note = if submitted.is_empty() {
                    areas.path.note().into()
                } else {
                    areas.path.replaced_note()
                };
            } else {
                editor.message =
                    "This page has no usable geometry, so the areas cannot be shown.".into();
                return split_editor_response(&editor, StatusCode::UNPROCESSABLE_ENTITY);
            }
            html(&editor)
        }
        Err(e @ AppError::Invalid(_)) => {
            editor.message = e.to_string();
            split_editor_response(&editor, error(&e))
        }
        Err(e) => Err(error(&e)),
    }
}

/// The boxes of a save: none when no box field came (computed on save), else exactly four per
/// part, each a finite number.
fn split_boxes(fields: &SplitFields, parts: usize) -> Result<Option<Vec<[[f64; 2]; 4]>>, AppError> {
    if fields.boxes.is_empty() {
        return Ok(None);
    }
    if fields.boxes.len() != parts * 4 || fields.boxes.keys().any(|(n, _)| *n > parts) {
        return Err(AppError::Invalid(
            "The boxes do not match the parts; show the areas again.",
        ));
    }
    (1..=parts)
        .map(|n| {
            let value = |k: usize| {
                fields.boxes[&(n, k)]
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .ok_or(AppError::Invalid("A box value is not a number."))
            };
            Ok(crate::sources::documents::box_polygon([
                value(0)?,
                value(1)?,
                value(2)?,
                value(3)?,
            ]))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

async fn create_split(
    State(state): State<AuthState>,
    session: Session,
    Path((page, unit)): Path<(i64, i64)>,
    Form(fields): Form<Vec<(String, String)>>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let fields = split_fields(fields);
    let token = fields.csrf.clone().ok_or(StatusCode::FORBIDDEN)?;
    let csrf = csrf_admission(&session, &token).await?;
    let editor_base = fields.editor(page, unit, token);
    // The part count is judged first, so a text with too many parts says so whatever else came.
    let parts = if fields.text.is_some() {
        crate::sources::documents::check_split_text(&editor_base.text).map(|parts| parts.len())
    } else {
        Err(AppError::Invalid(
            "The split form was incomplete; reopen it from the page.",
        ))
    };
    let result = match (
        parts,
        fields.malformed,
        fields.revision,
        fields.unit_revision,
    ) {
        (Err(e), ..) => Err(e),
        (Ok(parts), false, Some(revision), Some(unit_revision)) => {
            match split_boxes(&fields, parts) {
                Ok(boxes) => {
                    state
                        .sessions
                        .store
                        .material_create_split(
                            auth.clone(),
                            csrf,
                            page,
                            crate::sources::OwnerSplit {
                                revision,
                                unit: crate::sources::TableSource {
                                    unit,
                                    revision: unit_revision,
                                },
                                text: editor_base.text.clone(),
                                boxes,
                                actor: String::new(),
                                rationale: editor_base.rationale.clone(),
                            },
                        )
                        .await
                }
                Err(e) => Err(e),
            }
        }
        _ => Err(AppError::Invalid(
            "The split form was incomplete; reopen it from the page.",
        )),
    };
    match result {
        Ok((units, _)) => {
            let first = units[0];
            let cursor = state
                .sessions
                .store
                .material_unit_cursor(auth, page, first)
                .await
                .unwrap_or(0);
            Ok(Redirect::to(&format!(
                "/admin/materials/pages/{page}?units={cursor}&split={first}&unit={first}#unit-{first}"
            ))
            .into_response())
        }
        Err(e @ (AppError::Invalid(_) | AppError::Conflict)) => {
            let mut editor = editor_base;
            editor.message = e.to_string();
            if let Ok(read) = state
                .sessions
                .store
                .material_split_read(auth, page, unit, None)
                .await
            {
                editor.set_view(&read);
            }
            if !editor.boxes.is_empty() {
                editor.areas_note = "These are the boxes you submitted.".into();
            }
            split_editor_response(&editor, error(&e))
        }
        Err(e) => Err(error(&e)),
    }
}
