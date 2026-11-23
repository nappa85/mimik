//! Owner upload HTTP boundary. PDF bytes are never parsed in this process.
use super::{AuthResult, AuthState, csrf_admission, materials, owner_auth};
use crate::{
    AppError,
    sources::{
        uploads::{
            Metadata, OwnerWrite, PageModel, Upload, UploadPhase, UploadProgress, Uploads,
            exclusions::{
                BoxPoints, COMPARISON_LIMIT, Confirmation, ExclusionRequest, Proposal, RangeMode,
                RangeRefusal, RangeRequest, box_extent,
            },
            format_page_ranges, parse_page_ranges,
        },
        worker,
    },
};
use askama::Template;
use axum::{
    Router,
    body::Body,
    extract::{DefaultBodyLimit, Form, Path, Query, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Redirect},
    routing::{get, post},
};
use bytes::Bytes;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering},
    },
    time::Duration,
};
use tower_sessions::Session;

/// A client that sends nothing for this long loses the upload: it holds the single slot and a
/// spool. There is no total deadline and no minimum rate.
const IDLE: Duration = Duration::from_secs(30);
/// Before the CSRF token is admitted only this much of the body is read (unauthenticated memory).
const PRE_ADMISSION: u64 = 4096;
/// After admission, the bytes handed to the parser that are not file data (part headers, the
/// four form fields, the closing boundary and anything after it) may not exceed this: multer
/// buffers a part header until its blank line, so without a bound a header that never ends
/// would grow memory (and the search cost) with the stream. The budget covers the form fields
/// (under 1 KiB), headers, a 64 KiB trailer, and the parser's read-ahead of file data not yet
/// credited (two 64 KiB batches), with slack.
const NON_FILE: u64 = 512 * 1024;

pub(super) fn routes() -> Router<AuthState> {
    Router::new()
        .route(
            "/admin/materials/uploads",
            // No static size limit: the bound is the free-space reservation of the upload.
            get(index).post(upload).layer(DefaultBodyLimit::disable()),
        )
        .route("/admin/materials/uploads/{id}", get(page))
        .route("/admin/materials/uploads/{id}/progress", get(progress))
        .route(
            "/admin/materials/uploads/{id}/{action}",
            post(action).layer(DefaultBodyLimit::max(131_072)),
        )
        .route(
            "/admin/materials/uploads/{id}/pages/{page}/image",
            get(image),
        )
        .route(
            "/assets/source-upload.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../assets/source-upload.js"),
                )
            }),
        )
        .layer(axum::middleware::map_response(
            |mut r: axum::response::Response| async move {
                r.extensions_mut().insert(crate::campaigns::web::SheetPage);
                r
            },
        ))
}

/// The header of an upload page, served under the strict sheet policy (no layout script).
fn upload_nav() -> super::nav::Nav {
    super::nav::Nav::here_strict(super::nav::Section::Upload)
}

fn manager(state: &AuthState) -> Result<Uploads, StatusCode> {
    state
        .options
        .uploads
        .clone()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)
}
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, AppError> + Send + 'static,
) -> Result<T, StatusCode> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|e| materials::error(&e))
}

// Construct this inside the blocking task as well: if its JoinHandle is dropped,
// the abandoned task result still has asynchronous-safe destruction.
struct Phase(Option<UploadPhase>);
impl Drop for Phase {
    fn drop(&mut self) {
        if let Some(phase) = self.0.take() {
            tokio::task::spawn_blocking(move || drop(phase));
        }
    }
}
impl Phase {
    /// Reserve the body (its declared length, or a floor) before its first byte is written.
    async fn reserve(mut self, declared: Option<u64>) -> Result<Self, StatusCode> {
        blocking(move || {
            self.0
                .as_mut()
                .ok_or(AppError::Worker)?
                .reserve_body(declared)?;
            Ok(self)
        })
        .await
    }
    async fn write(mut self, bytes: Bytes) -> Result<Self, StatusCode> {
        blocking(move || {
            self.0.as_mut().ok_or(AppError::Worker)?.write(&bytes)?;
            Ok(self)
        })
        .await
    }
}

struct Counting {
    body: axum::body::BodyDataStream,
    pending: Bytes,
    /// Bytes handed to the parser.
    count: u64,
    /// File bytes the route has taken from the parser; `count` minus this is bounded by
    /// [`NON_FILE`] after admission (and `count` by [`PRE_ADMISSION`] before it).
    file_bytes: Arc<AtomicU64>,
    admitted: Arc<AtomicBool>,
    status: Arc<AtomicU16>,
    idle: Duration,
    since_yield: usize,
}
impl Counting {
    async fn next(&mut self) -> Option<Result<Bytes, std::io::Error>> {
        let result = self.read().await;
        match result {
            Ok(value) => value.map(Ok),
            Err(status) => {
                self.status.store(status.as_u16(), Ordering::Release);
                Some(Err(std::io::Error::other("Upload stream refused.")))
            }
        }
    }
    async fn read(&mut self) -> Result<Option<Bytes>, StatusCode> {
        // Multer drains an always-ready stream before examining its buffer.
        // Explicit Pending boundaries: one byte before admission, 64 KiB after.
        // Multer can retain two batches (next_field then chunk) plus delimiter
        // lookbehind; this does not depend on Tokio's cooperative task budget.
        if !self.admitted.load(Ordering::Acquire) || self.since_yield >= 65536 {
            self.since_yield = 0;
            tokio::task::yield_now().await;
        }
        // Only time waiting for input is client idle: server work between calls (metadata
        // validation, reservations and blocking file writes) is not counted, and nothing bounds
        // the total.
        let deadline = tokio::time::Instant::now() + self.idle;
        while self.pending.is_empty() {
            match tokio::time::timeout_at(deadline, self.body.next()).await {
                Ok(Some(Ok(bytes))) => {
                    if bytes.is_empty() {
                        tokio::task::yield_now().await;
                        continue;
                    }
                    self.pending = bytes;
                }
                Ok(None) => return Ok(None),
                Ok(Some(Err(_))) => return Err(StatusCode::BAD_REQUEST),
                Err(_) => return Err(StatusCode::REQUEST_TIMEOUT),
            }
        }
        let admitted = self.admitted.load(Ordering::Acquire);
        if !admitted && self.count >= PRE_ADMISSION {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
        if admitted
            && self
                .count
                .saturating_sub(self.file_bytes.load(Ordering::Acquire))
                > NON_FILE
        {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
        // Before admission, give the parser only the bytes it needs, so it cannot
        // read ahead into metadata/file while finding the first field boundary.
        let n = if admitted {
            self.pending.len().min(16384).min(65536 - self.since_yield)
        } else {
            self.pending.len().min(1)
        };
        self.count += n as u64;
        self.since_yield += n;
        Ok(Some(self.pending.split_to(n)))
    }
}

fn multipart_error(error: &multer::Error, status: &AtomicU16) -> StatusCode {
    let stored = status.load(Ordering::Acquire);
    if stored != 0 {
        return StatusCode::from_u16(stored).unwrap_or(StatusCode::BAD_REQUEST);
    }
    match error {
        multer::Error::FieldSizeExceeded { .. } => StatusCode::PAYLOAD_TOO_LARGE,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    }
}
async fn text(
    m: &mut multer::Multipart<'_>,
    expected: &str,
    status: &AtomicU16,
) -> Result<String, StatusCode> {
    let field = m
        .next_field()
        .await
        .map_err(|e| multipart_error(&e, status))?
        .ok_or(StatusCode::UNPROCESSABLE_ENTITY)?;
    if field.name() != Some(expected) {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    field.text().await.map_err(|e| multipart_error(&e, status))
}
async fn upload(State(state): State<AuthState>, session: Session, request: Request) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let mut lengths = request.headers().get_all(header::CONTENT_LENGTH).iter();
    let declared = if let Some(length) = lengths.next() {
        let n = length
            .to_str()
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or(StatusCode::BAD_REQUEST)?;
        if lengths.next().is_some() {
            return Err(StatusCode::BAD_REQUEST);
        }
        Some(n)
    } else {
        None
    };
    let boundary = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| multer::parse_boundary(v).ok())
        .ok_or(StatusCode::UNSUPPORTED_MEDIA_TYPE)?;
    let m = manager(&state)?;
    let probe = m.clone();
    if let worker::Availability::Unavailable { reason, how_to_fix } =
        blocking(move || Ok(probe.availability())).await?
    {
        let mut response = materials::html(&UploadForm {
            nav: upload_nav(),
            csrf: String::new(),
            reason,
            remedy: how_to_fix,
        })?;
        *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
        response
            .extensions_mut()
            .insert(materials::WorkerUnavailable);
        return Ok(response);
    }
    let reserve = m.clone();
    let phase = match blocking(move || reserve.begin_upload().map(|p| Phase(Some(p)))).await {
        Ok(phase) => phase,
        Err(StatusCode::CONFLICT) => {
            let current = m.clone();
            let owner = auth.clone();
            let retained = blocking(move || current.current_for_owner(&owner)).await?;
            let mut response = if let Some(id) = retained {
                materials::html(&StagedConflict {
                    nav: upload_nav(),
                    id,
                })?
            } else {
                (StatusCode::CONFLICT, "an upload is in progress").into_response()
            };
            *response.status_mut() = StatusCode::CONFLICT;
            response.extensions_mut().insert(materials::TrustedDraft);
            return Ok(response);
        }
        Err(e) => return Err(e),
    };
    receive(
        m,
        phase,
        auth,
        session,
        request.into_body(),
        boundary,
        declared,
    )
    .await
}
async fn receive(
    m: Uploads,
    mut phase: Phase,
    auth: super::RequestAuth,
    session: Session,
    body: Body,
    boundary: String,
    declared: Option<u64>,
) -> AuthResult {
    let admitted = Arc::new(AtomicBool::new(false));
    let status = Arc::new(AtomicU16::new(0));
    let file_bytes = Arc::new(AtomicU64::new(0));
    let counting = Arc::new(tokio::sync::Mutex::new(Counting {
        body: body.into_data_stream(),
        pending: Bytes::new(),
        count: 0,
        file_bytes: file_bytes.clone(),
        admitted: admitted.clone(),
        status: status.clone(),
        idle: IDLE,
        since_yield: 0,
    }));
    let stream = futures_util::stream::unfold(counting.clone(), |c| async move {
        let next = c.lock().await.next().await;
        next.map(|b| (b, c))
    });
    let constraints = multer::Constraints::new()
        .allowed_fields(vec!["csrf", "system", "edition", "language", "file"])
        // The file field has no limit here: the reservation bounds it.
        .size_limit(
            multer::SizeLimit::new()
                .for_field("csrf", 256)
                .for_field("system", 100)
                .for_field("edition", 400)
                .for_field("language", 30),
        );
    let mut multipart = multer::Multipart::with_constraints(stream, boundary, constraints);
    let token = text(&mut multipart, "csrf", &status).await?;
    let csrf = csrf_admission(&session, &token).await?;
    admitted.store(true, Ordering::Release);
    let metadata = Metadata {
        system_id: text(&mut multipart, "system", &status).await?,
        edition: text(&mut multipart, "edition", &status).await?,
        language: text(&mut multipart, "language", &status).await?,
    };
    let owner = OwnerWrite::new(auth, csrf);
    let check_manager = m.clone();
    let check_owner = owner.clone();
    let check_metadata = metadata.clone();
    blocking(move || check_manager.validate_metadata(&check_owner, &check_metadata)).await?;
    // Admission passed and the metadata is valid: only now does the upload hold free space.
    phase = phase.reserve(declared).await?;
    let mut file = multipart
        .next_field()
        .await
        .map_err(|e| multipart_error(&e, &status))?
        .ok_or(StatusCode::UNPROCESSABLE_ENTITY)?;
    if file.name() != Some("file") {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    let mut magic = Vec::with_capacity(1024);
    while let Some(bytes) = file
        .chunk()
        .await
        .map_err(|e| multipart_error(&e, &status))?
    {
        size += bytes.len() as u64;
        file_bytes.fetch_add(bytes.len() as u64, Ordering::Release);
        magic.extend_from_slice(&bytes[..bytes.len().min(1024 - magic.len())]);
        digest.update(&bytes);
        phase = phase.write(bytes).await?;
    }
    drop(file);
    if multipart
        .next_field()
        .await
        .map_err(|e| multipart_error(&e, &status))?
        .is_some()
    {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    drop(multipart);
    // Multer stops at the closing boundary. Count the epilogue too, including
    // unread transport frames, before publishing anything.
    // The non-file budget bounds it: an endless trailer is refused.
    while let Some(chunk) = counting.lock().await.next().await {
        chunk.map_err(|_| {
            StatusCode::from_u16(status.load(Ordering::Acquire)).unwrap_or(StatusCode::BAD_REQUEST)
        })?;
    }
    if !magic.windows(5).any(|s| s == b"%PDF-") {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let hash = hex::encode(digest.finalize());
    let id = blocking(move || {
        m.create(
            &owner,
            phase.0.take().ok_or(AppError::Worker)?,
            &hash,
            size,
            &metadata,
        )
    })
    .await?;
    Ok(Redirect::to(&format!("/admin/materials/uploads/{id}")).into_response())
}

#[derive(Template)]
#[template(
    source = "{% extends \"material_base.html\" %}{% block content %}<p>an upload is already staged; open or discard it: <a href=\"/admin/materials/uploads/{{ id }}\">Open staged upload</a></p>{% endblock %}",
    ext = "html"
)]
struct StagedConflict {
    nav: super::nav::Nav,
    id: String,
}
#[derive(Template)]
#[template(
    source = "{% extends \"material_base.html\" %}{% block content %}<p>{{ message }}</p>{% endblock %}",
    ext = "html"
)]
struct ProposalError {
    nav: super::nav::Nav,
    message: &'static str,
}

fn proposal_error(status: StatusCode, message: &'static str) -> AuthResult {
    let mut response = materials::html(&ProposalError {
        nav: upload_nav(),
        message,
    })?;
    *response.status_mut() = status;
    response.extensions_mut().insert(materials::TrustedDraft);
    Ok(response)
}
#[derive(Template)]
#[template(path = "upload_form.html")]
struct UploadForm {
    nav: super::nav::Nav,
    csrf: String,
    reason: String,
    remedy: String,
}
async fn index(State(state): State<AuthState>, session: Session) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let m = manager(&state)?;
    let current = m.clone();
    if let Some(id) = blocking(move || current.current_for_owner(&auth)).await? {
        return Ok(Redirect::to(&format!("/admin/materials/uploads/{id}")).into_response());
    }
    let availability = blocking(move || Ok(m.availability())).await?;
    let (reason, remedy) = match availability {
        worker::Availability::Available => (String::new(), String::new()),
        worker::Availability::Unavailable { reason, how_to_fix } => (reason, how_to_fix),
    };
    materials::html(&UploadForm {
        nav: upload_nav(),
        csrf: super::token(&session).await?.ok_or(StatusCode::FORBIDDEN)?,
        reason,
        remedy,
    })
}
/// Pages listed on one screen: the selection list, the rendered pages and the box editors cover
/// the same 100 pages, so the page never renders 20,000 entries.
const SCREEN_PAGES: u32 = 100;

#[derive(Template)]
#[template(path = "upload_page.html")]
struct Screen {
    nav: super::nav::Nav,
    upload: Upload,
    csrf: String,
    refresh: bool,
    /// The selection as the range text of the input (`1-500,520`).
    selection: String,
    selected_count: usize,
    window: Window,
    /// The inspected pages of the window.
    listed: Vec<Listed>,
    /// The rendered pages of the window with their confirmed boxes.
    rendered: Vec<Rendered>,
    proposals: Vec<ProposalView>,
    /// The selected rendered pages of the window, for manual boxes.
    manual: Vec<EditorPage>,
    /// The "Boxes on a page range" form: prefilled, or the refused post with its message.
    range: RangeView,
}
/// The values of the range form and the words above it.
#[derive(Default)]
struct RangeView {
    pages: String,
    all: bool,
    /// `add`, `remove` or `clear`.
    mode: String,
    source_page: String,
    boxes: String,
    clear_ack: bool,
    rationale: String,
    /// Why the last post changed nothing.
    refusal: Option<String>,
    /// What the last post changed (from the redirect's counts).
    notice: Option<String>,
}
impl RangeView {
    fn posted(form: &BTreeMap<String, String>) -> Self {
        let get = |key: &str| form.get(key).cloned().unwrap_or_default();
        Self {
            pages: get("pages"),
            all: form.get("all").is_some_and(|v| v == "1"),
            mode: get("mode"),
            source_page: get("source_page"),
            boxes: get("boxes"),
            clear_ack: form.get("clear_ack").is_some_and(|v| v == "1"),
            rationale: get("rationale"),
            refusal: None,
            notice: None,
        }
    }
}
fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}
/// The words of a refusal that names its page.
fn refusal_text(refusal: RangeRefusal) -> String {
    match refusal {
        RangeRefusal::NotRendered(n) => {
            format!("Page {n} is not a selected rendered page; nothing was changed.")
        }
        RangeRefusal::SourceNotRendered(n) => {
            format!("Page {n}, where the boxes were drawn, is not a selected rendered page.")
        }
        RangeRefusal::TooManyBoxes(n) => {
            format!("Page {n} would have more than eight boxes; nothing was changed.")
        }
        RangeRefusal::Collapsed(n) => {
            format!("A box has no area on page {n} after scaling; nothing was changed.")
        }
        RangeRefusal::NoPages => "There are no selected rendered pages to edit.".into(),
        RangeRefusal::NoExclusion => "There is no saved exclusion to remove boxes from.".into(),
        RangeRefusal::NoMatch => "No page has a matching box; nothing was changed.".into(),
        RangeRefusal::NoChange => "Nothing to change on these pages.".into(),
    }
}
struct Window {
    number: u32,
    count: u32,
    from: u32,
    to: u32,
}
impl Window {
    fn new(requested: u32, last_page: u32) -> Self {
        let count = last_page.div_ceil(SCREEN_PAGES).max(1);
        let number = requested.clamp(1, count);
        Self {
            number,
            count,
            from: (number - 1) * SCREEN_PAGES + 1,
            to: number * SCREEN_PAGES,
        }
    }
    fn contains(&self, n: u32) -> bool {
        (self.from..=self.to).contains(&n)
    }
    fn previous(&self) -> Option<u32> {
        (self.number > 1).then(|| self.number - 1)
    }
    fn next(&self) -> Option<u32> {
        (self.number < self.count).then(|| self.number + 1)
    }
}
struct Listed {
    n: u32,
    width: f64,
    height: f64,
    rotate: u16,
    selected: bool,
}
struct Rendered {
    page: PageModel,
    masks: Vec<Mask>,
}
struct ProposalView {
    text: String,
    fingerprint: String,
    /// Pages with found text, as ranges.
    affected: String,
    affected_count: usize,
    /// Pages without it (an offered median box), as ranges.
    absent: String,
    /// Every box on the pages of the window, for the combined preview.
    window_boxes: BTreeMap<u32, Vec<[f64; 4]>>,
    found: std::collections::BTreeSet<u32>,
    missing: std::collections::BTreeSet<u32>,
}
impl ProposalView {
    fn masks(&self, n: u32) -> Vec<Mask> {
        self.window_boxes
            .get(&n)
            .into_iter()
            .flatten()
            .map(|b| Mask::of(*b))
            .collect()
    }
    /// One view of a fingerprint: proposals with the same text (at different places) share it,
    /// so the form has one confirm, skip and absent control for it and shows every box.
    fn merge(views: &mut Vec<Self>, p: Proposal, window: &Window) {
        let index = views.iter().position(|v| v.fingerprint == p.fingerprint);
        let view = if let Some(index) = index {
            &mut views[index]
        } else {
            views.push(Self {
                text: p.text.clone(),
                fingerprint: p.fingerprint.clone(),
                affected: String::new(),
                affected_count: 0,
                absent: String::new(),
                window_boxes: BTreeMap::new(),
                found: std::collections::BTreeSet::new(),
                missing: std::collections::BTreeSet::new(),
            });
            let last = views.len() - 1;
            &mut views[last]
        };
        view.found.extend(p.boxes.keys());
        view.missing.extend(p.absent.keys());
        for (n, b) in p.boxes {
            if window.contains(n) {
                view.window_boxes.entry(n).or_default().push(b);
            }
        }
    }
    /// The page lists, once every proposal is merged: a page without a box from any of them.
    fn finish(&mut self) {
        let found: Vec<u32> = self.found.iter().copied().collect();
        let absent: Vec<u32> = self.missing.difference(&self.found).copied().collect();
        self.affected = format_page_ranges(&found);
        self.affected_count = found.len();
        self.absent = format_page_ranges(&absent);
    }
}
struct EditorPage {
    page: PageModel,
    /// What the box inputs may reach: the page size rounded up like the stored boxes.
    max_x: f64,
    max_y: f64,
    /// The first box, edited with the drawing widget or its numeric inputs.
    bounds: [f64; 4],
    selected: bool,
    /// Further saved boxes (at most seven), one `left top right bottom` per line.
    more: String,
}
struct Mask {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}
impl Mask {
    fn of(b: [f64; 4]) -> Self {
        Self {
            x: b[0],
            y: b[1],
            width: b[2] - b[0],
            height: b[3] - b[1],
        }
    }
}
impl Screen {
    fn build(
        nav: super::nav::Nav,
        upload: Upload,
        csrf: String,
        refresh: bool,
        proposals: Vec<Proposal>,
        requested: u32,
    ) -> Self {
        let selected: HashSet<u32> = upload.selected_pages.iter().copied().collect();
        let last_page = upload
            .inspection
            .as_ref()
            .and_then(|i| i.pages.iter().map(|p| p.n).max())
            .unwrap_or(0);
        let window = Window::new(requested, last_page);
        let listed = upload
            .inspection
            .iter()
            .flat_map(|i| &i.pages)
            .filter(|p| window.contains(p.n))
            .map(|p| Listed {
                n: p.n,
                width: p.width,
                height: p.height,
                rotate: p.rotate,
                selected: selected.contains(&p.n),
            })
            .collect();
        let saved = upload.exclusion.as_ref().map(|e| &e.boxes);
        let rendered = upload
            .pages
            .iter()
            .filter(|p| window.contains(p.pdf_page))
            .map(|p| Rendered {
                page: p.clone(),
                masks: saved
                    .and_then(|boxes| boxes.get(&p.pdf_page))
                    .into_iter()
                    .flatten()
                    .map(|b| Mask::of(*b))
                    .collect(),
            })
            .collect();
        let manual: Vec<EditorPage> = upload
            .pages
            .iter()
            .filter(|p| window.contains(p.pdf_page) && selected.contains(&p.pdf_page))
            .map(|page| {
                let mut boxes = saved
                    .and_then(|boxes| boxes.get(&page.pdf_page))
                    .map(|b| b.iter());
                let first = boxes.as_mut().and_then(Iterator::next).copied();
                let (max_x, max_y) = box_extent(page.width_points, page.height_points);
                EditorPage {
                    page: page.clone(),
                    max_x,
                    max_y,
                    bounds: first.unwrap_or([
                        page.width_points / 4.,
                        page.height_points / 4.,
                        page.width_points / 2.,
                        page.height_points / 2.,
                    ]),
                    selected: first.is_some(),
                    more: boxes
                        .into_iter()
                        .flatten()
                        .map(|b| format!("{} {} {} {}", b[0], b[1], b[2], b[3]))
                        .collect::<Vec<_>>()
                        .join("\n"),
                }
            })
            .collect();
        let range = RangeView {
            mode: "add".into(),
            source_page: manual.first().map_or(1, |e| e.page.pdf_page).to_string(),
            ..RangeView::default()
        };
        let mut views = Vec::new();
        for p in proposals {
            ProposalView::merge(&mut views, p, &window);
        }
        views.iter_mut().for_each(ProposalView::finish);
        let proposals = views;
        Self {
            nav,
            selection: format_page_ranges(&upload.selected_pages),
            selected_count: upload.selected_pages.len(),
            listed,
            rendered,
            proposals,
            manual,
            range,
            window,
            upload,
            csrf,
            refresh,
        }
    }
    fn busy(&self) -> bool {
        matches!(self.upload.job.state.as_str(), "queued" | "running")
    }
    fn hash(&self) -> &str {
        self.upload.pdf_sha256.get(..12).unwrap_or("")
    }
}
#[derive(serde::Deserialize)]
struct ScreenQuery {
    p: Option<u32>,
    /// What a range edit changed (numbers only: no text travels in the address).
    range_changed: Option<String>,
    range_sizes: Option<String>,
    range_unmatched: Option<String>,
}
async fn model(
    state: &AuthState,
    session: &Session,
    id: String,
    requested: u32,
    proposals: Vec<Proposal>,
) -> Result<Screen, StatusCode> {
    let auth = owner_auth(state, session).await?;
    let m = manager(state)?;
    let upload = blocking(move || m.read_for_owner(&auth, &id)).await?;
    let refresh = proposals.is_empty() && matches!(upload.job.state.as_str(), "queued" | "running");
    Ok(Screen::build(
        upload_nav(),
        upload,
        super::token(session).await?.ok_or(StatusCode::FORBIDDEN)?,
        refresh,
        proposals,
        requested,
    ))
}
async fn page(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<String>,
    Query(query): Query<ScreenQuery>,
) -> AuthResult {
    let mut screen = model(&state, &session, id, query.p.unwrap_or(1), Vec::new()).await?;
    let count = |v: &Option<String>| v.as_deref().and_then(|v| v.parse::<usize>().ok());
    if let (Some(changed), Some(sizes)) = (count(&query.range_changed), count(&query.range_sizes)) {
        let mut notice = format!(
            "{} changed, {}.",
            plural(changed, "page", "pages"),
            plural(sizes, "page size", "page sizes")
        );
        if let Some(unmatched) = count(&query.range_unmatched) {
            notice = format!(
                "{notice} {} had no matching box.",
                plural(unmatched, "page", "pages")
            );
        }
        screen.range.notice = Some(notice);
    }
    materials::html(&screen)
}
#[derive(Template)]
#[template(path = "upload_progress.html")]
struct Progress {
    upload: UploadProgress,
    csrf: String,
}
async fn progress(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<String>,
) -> AuthResult {
    // A poll reads the job only: not the inspection, the page rows or the exclusion.
    let auth = owner_auth(&state, &session).await?;
    let m = manager(&state)?;
    let upload = blocking(move || m.progress_for_owner(&auth, &id)).await?;
    materials::html(&Progress {
        upload,
        csrf: super::token(&session).await?.ok_or(StatusCode::FORBIDDEN)?,
    })
}
async fn action(
    State(state): State<AuthState>,
    session: Session,
    Path((id, action)): Path<(String, String)>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> AuthResult {
    let mut form = form_map(pairs).map_err(|e| materials::error(&e))?;
    let auth = owner_auth(&state, &session).await?;
    let csrf = csrf_admission(&session, &form.remove("csrf").ok_or(StatusCode::FORBIDDEN)?).await?;
    let owner = OwnerWrite::new(auth.clone(), csrf);
    let m = manager(&state)?;
    if action == "propose" {
        let revision = form
            .get("revision")
            .and_then(|v| v.parse().ok())
            .ok_or(StatusCode::UNPROCESSABLE_ENTITY)?;
        let query = m.clone();
        let query_id = id.clone();
        let proposals = match tokio::task::spawn_blocking(move || {
            query.propose_exclusions(&auth, &query_id, revision)
        })
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        {
            Ok(proposals) => proposals,
            Err(AppError::Conflict) => {
                return proposal_error(
                    StatusCode::CONFLICT,
                    "Wait for the upload job to finish and reload the current selection before proposing exclusions.",
                );
            }
            Err(AppError::Invalid(COMPARISON_LIMIT)) => {
                return proposal_error(StatusCode::UNPROCESSABLE_ENTITY, COMPARISON_LIMIT);
            }
            Err(error) => return Err(materials::error(&error)),
        };
        let mut screen = model(&state, &session, id, 1, proposals).await?;
        if screen.upload.revision != revision {
            return Err(StatusCode::CONFLICT);
        }
        screen.refresh = false;
        return materials::html(&screen);
    }
    if action == "exclude-range" {
        return exclude_range(&state, &session, m, owner, id, form).await;
    }
    let target = if action == "discard" {
        "/admin/materials/uploads".into()
    } else {
        format!("/admin/materials/uploads/{id}")
    };
    blocking(move || {
        let revision = || {
            form.get("revision")
                .and_then(|v| v.parse().ok())
                .ok_or(AppError::Invalid("Invalid revision."))
        };
        match action.as_str() {
            "cancel" => m.cancel(&owner, &id),
            "discard" => m.discard(&owner, &id),
            "inspect" | "render" => m.start(&owner, &id, revision()?, action == "inspect"),
            "exclude" | "remove-exclusion" => {
                let revision = revision()?;
                let excluded = form
                    .get("exclusion_revision")
                    .and_then(|v| v.parse().ok())
                    .ok_or(AppError::Invalid("Invalid exclusion revision."))?;
                let rationale = form.get("rationale").cloned().unwrap_or_default();
                if action == "remove-exclusion" {
                    m.remove_exclusion(&owner, &id, revision, excluded, &rationale)
                } else {
                    let request = exclusion_form(&form, rationale)?;
                    m.set_exclusion(&owner, &id, revision, excluded, &request)
                }
            }
            "set" => {
                let revision = revision()?;
                let metadata = Metadata {
                    system_id: form.remove("system").unwrap_or_default(),
                    edition: form.remove("edition").unwrap_or_default(),
                    language: form.remove("language").unwrap_or_default(),
                };
                // Any ascending, distinct subset of the inspected pages: a range list
                // (`1-500, 520`) or all of them.
                let pages = if form.get("select_all").is_some_and(|v| v == "1") {
                    let mut all: Vec<u32> = m
                        .read_for_owner(&auth, &id)?
                        .inspection
                        .iter()
                        .flat_map(|i| i.pages.iter().map(|p| p.n))
                        .collect();
                    all.sort_unstable();
                    all
                } else {
                    parse_page_ranges(form.get("pages").map_or("", String::as_str))?
                };
                m.set(&owner, &id, revision, &metadata, &pages)
            }
            _ => Err(AppError::SourceNotFound),
        }
    })
    .await?;
    Ok(Redirect::to(&target).into_response())
}
/// The `exclude-range` action: boxes over a range of pages. A refusal that changes nothing
/// renders the screen again at 422 with the posted values and the reason.
async fn exclude_range(
    state: &AuthState,
    session: &Session,
    m: Uploads,
    owner: OwnerWrite,
    id: String,
    form: BTreeMap<String, String>,
) -> AuthResult {
    let number = |key: &str| form.get(key).and_then(|v| v.parse::<i64>().ok());
    let (Some(revision), Some(excluded)) = (number("revision"), number("exclusion_revision"))
    else {
        return Err(materials::error(&AppError::Invalid("Invalid revision.")));
    };
    let screen = form.get("p").and_then(|v| v.parse().ok()).unwrap_or(1);
    let outcome = match range_form(&form) {
        Ok(request) => {
            let mode = request.mode;
            let target = id.clone();
            tokio::task::spawn_blocking(move || {
                m.set_exclusion_range(&owner, &target, revision, excluded, &request)
            })
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .map(|done| done.map(|counts| (mode, counts)))
        }
        Err(error) => Err(error),
    };
    let reason = match outcome {
        Ok(Ok((mode, counts))) => {
            let mut target = format!(
                "/admin/materials/uploads/{id}?p={screen}&range_changed={}&range_sizes={}",
                counts.changed, counts.sizes
            );
            if mode == RangeMode::Remove {
                target = format!("{target}&range_unmatched={}", counts.unmatched);
            }
            return Ok(Redirect::to(&target).into_response());
        }
        Ok(Err(refusal)) => refusal_text(refusal),
        Err(AppError::Invalid(reason)) => reason.to_owned(),
        Err(error) => return Err(materials::error(&error)),
    };
    let mut page = model(state, session, id, screen, Vec::new()).await?;
    page.range = RangeView {
        refusal: Some(reason),
        ..RangeView::posted(&form)
    };
    let mut response = materials::html(&page)?;
    *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
    response.extensions_mut().insert(materials::TrustedDraft);
    Ok(response)
}
/// The request of the range form; its mode is a strict enum and `clear` needs its
/// acknowledgement.
fn range_form(form: &BTreeMap<String, String>) -> Result<RangeRequest, AppError> {
    let text = |key: &str| form.get(key).map_or("", String::as_str);
    let mode =
        RangeMode::parse(text("mode")).ok_or(AppError::Invalid("Choose add, remove or clear."))?;
    let pages = if text("all") == "1" {
        None
    } else {
        let pages = parse_page_ranges(text("pages"))?;
        if pages.is_empty() {
            return Err(AppError::Invalid(
                "Give the pages, or tick all selected pages.",
            ));
        }
        Some(pages)
    };
    let rationale = text("rationale").to_owned();
    if mode == RangeMode::Clear {
        if text("clear_ack") != "1" {
            return Err(AppError::Invalid(
                "Tick the acknowledgement that every box on these pages is removed.",
            ));
        }
        return Ok(RangeRequest {
            pages,
            mode,
            source_page: 0,
            boxes: Vec::new(),
            rationale,
        });
    }
    let source_page = text("source_page")
        .parse()
        .map_err(|_| AppError::Invalid("Give the page the boxes were drawn on."))?;
    Ok(RangeRequest {
        pages,
        mode,
        source_page,
        boxes: box_lines(text("boxes"))?,
        rationale,
    })
}
/// One box per line: four numbers separated by white space or commas.
fn box_lines(text: &str) -> Result<Vec<BoxPoints>, AppError> {
    let invalid = || AppError::Invalid("Invalid displayed exclusion box.");
    let mut boxes = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let numbers: Vec<f64> = line
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .map(|s| s.parse().map_err(|_| invalid()))
            .collect::<Result<_, _>>()?;
        boxes.push(<BoxPoints>::try_from(numbers).map_err(|_| invalid())?);
    }
    Ok(boxes)
}
/// The posted fields as a map; a repeated key is refused instead of letting the last one win.
fn form_map(pairs: Vec<(String, String)>) -> Result<BTreeMap<String, String>, AppError> {
    let mut map = BTreeMap::new();
    for (key, value) in pairs {
        if map.insert(key, value).is_some() {
            return Err(AppError::Invalid("A form field was sent twice."));
        }
    }
    Ok(map)
}
/// The request of the `exclude` form: confirmed proposals (by fingerprint, with the pages to
/// leave out and to extend to) and the manual boxes of the pages on the screen. The boxes of
/// proposals are never posted: the server rebuilds them.
fn exclusion_form(
    form: &BTreeMap<String, String>,
    rationale: String,
) -> Result<ExclusionRequest, AppError> {
    let ranges = |key: String| parse_page_ranges(form.get(&key).map_or("", String::as_str));
    let mut confirmed = Vec::new();
    for (key, value) in form {
        if let Some(fingerprint) = key.strip_prefix("confirm_").filter(|_| value == "1") {
            confirmed.push(Confirmation {
                fingerprint: fingerprint.to_owned(),
                skip: ranges(format!("skip_{fingerprint}"))?,
                absent: ranges(format!("absent_{fingerprint}"))?,
            });
        }
    }
    let mut manual = BTreeMap::new();
    for key in form.keys().filter(|k| k.starts_with("exclude_")) {
        let page: u32 = key[8..]
            .parse()
            .map_err(|_| AppError::Invalid("Invalid exclusion page."))?;
        let mut bounds = [0.; 4];
        for (i, value) in bounds.iter_mut().enumerate() {
            *value = form
                .get(&format!("box_{page}_{i}"))
                .and_then(|s| s.parse().ok())
                .ok_or(AppError::Invalid("Invalid displayed exclusion box."))?;
        }
        let mut boxes = vec![bounds];
        boxes.extend(box_lines(
            form.get(&format!("more_{page}")).map_or("", String::as_str),
        )?);
        manual.insert(page, boxes);
    }
    // Further boxes of a page whose region is not ticked are the prefilled saved ones when the
    // Owner only unticks it. They are never dropped silently: the server compares them with the
    // saved boxes and refuses a difference.
    let mut unticked_extra = BTreeMap::new();
    for (key, value) in form {
        if let Some(page) = key.strip_prefix("more_")
            && !value.trim().is_empty()
        {
            let page: u32 = page
                .parse()
                .map_err(|_| AppError::Invalid("Invalid exclusion page."))?;
            if !manual.contains_key(&page) {
                unticked_extra.insert(page, box_lines(value)?);
            }
        }
    }
    let window = match (form.get("window_from"), form.get("window_to")) {
        (Some(from), Some(to)) => Some((
            from.parse()
                .map_err(|_| AppError::Invalid("Invalid exclusion window."))?,
            to.parse()
                .map_err(|_| AppError::Invalid("Invalid exclusion window."))?,
        )),
        _ => None,
    };
    Ok(ExclusionRequest {
        confirmed,
        manual,
        unticked_extra,
        window,
        rationale,
        extend_absent: form.get("extend_absent").is_some_and(|v| v == "1"),
    })
}
async fn image(
    State(state): State<AuthState>,
    session: Session,
    Path((id, page)): Path<(String, u32)>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let m = manager(&state)?;
    let bytes = blocking(move || m.image_for_owner(&auth, &id, page)).await?;
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

/// The page screen of `upload`, rendered (the large-input gate times it).
#[cfg(test)]
pub(crate) fn screen_html_for_test(upload: Upload, requested: u32) -> String {
    Screen::build(
        super::nav::Nav::logo_only(),
        upload,
        "t".repeat(64),
        false,
        Vec::new(),
        requested,
    )
    .render()
    .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upload_of(pages: u32, selected: Vec<u32>) -> Upload {
        Upload {
            exclusion_revision: 0,
            exclusion: None,
            already_document: None,
            id: "0".repeat(32),
            pdf_sha256: "a".repeat(64),
            pdf_bytes: 1,
            revision: 2,
            state: "inspected".into(),
            document_changed: false,
            metadata: Metadata {
                system_id: "mork-borg".into(),
                edition: "Synthetic".into(),
                language: "en".into(),
            },
            inspection: Some(crate::sources::uploads::InspectionData {
                document: worker::records::Document {
                    pages,
                    encrypted: false,
                    mupdf: "1.27.2".into(),
                },
                pages: (1..=pages)
                    .map(|n| worker::Page {
                        n,
                        width: 595.2756,
                        height: 841.89,
                        rotate: 0,
                        user_unit: 1.,
                        transform: [1., 0., 0., -1., 0., 841.89],
                        visible: [0., 0., 595.2756, 841.89],
                        tagged: false,
                    })
                    .collect(),
            }),
            selected_pages: selected,
            pages: Vec::new(),
            job: crate::sources::uploads::Job {
                revision: 2,
                kind: "inspect".into(),
                state: "succeeded".into(),
                done: 0,
                total: 0,
                reason: None,
            },
        }
    }

    #[test]
    fn the_page_screen_lists_a_hundred_pages_and_not_a_checkbox_per_page() {
        let upload = upload_of(20_000, (1..=20_000).collect());
        let nav = super::super::nav::Nav::logo_only;
        let screen = Screen::build(nav(), upload.clone(), "t".repeat(64), false, Vec::new(), 3);
        assert_eq!(
            (screen.window.from, screen.window.to, screen.window.count),
            (201, 300, 200)
        );
        assert_eq!(screen.listed.len(), 100);
        assert_eq!(screen.selection, "1-20000");
        let html = screen.render().unwrap();
        assert_eq!(html.matches("<li ").count(), 100);
        // The `select all` choice is the only selection checkbox; no per-page input exists (the
        // range form adds its own two: all pages and the clear acknowledgement).
        assert_eq!(html.matches("type=\"checkbox\"").count(), 3);
        assert_eq!(html.matches("name=\"select_all\"").count(), 1);
        assert!(!html.contains("name=\"page_"));
        assert!(
            html.contains("?p=2\">Previous 100 pages") && html.contains("?p=4\">Next 100 pages")
        );
        assert!(html.len() < 100_000, "{} bytes", html.len());
        // Out-of-range screens clamp; the first screen has no previous link.
        let first = Screen::build(nav(), upload.clone(), "t".repeat(64), false, Vec::new(), 0);
        assert_eq!(first.window.number, 1);
        assert!(first.window.previous().is_none());
        let last = Screen::build(nav(), upload, "t".repeat(64), false, Vec::new(), 9999);
        assert_eq!((last.window.number, last.listed.len()), (200, 100));
        assert!(last.window.next().is_none());
    }

    fn page_model(n: u32, width: f64, height: f64) -> PageModel {
        PageModel {
            renderer_crate_version: worker::CRATE_VERSION.into(),
            mupdf_version: worker::MUPDF_VERSION.into(),
            pdf_page: n,
            rendered_revision: 2,
            png_sha256: "b".repeat(64),
            png_bytes: 1,
            width_px: 1,
            height_px: 1,
            dpi: 150,
            width_points: width,
            height_points: height,
            transform: [1., 0., 0., -1., 0., height],
            stats: worker::records::Stats {
                n,
                chars: 0,
                unmapped: 0,
                image_area: 0.,
                shading_area: 0.0,
                fonts_substituted: Vec::new(),
                fonts_skipped: 0,
                incomplete: false,
                dropped_lines: 0,
            },
            settings: worker::RenderSettings::default(),
            text_sha256: "c".repeat(64),
            text_bytes: 2,
        }
    }

    #[test]
    fn the_box_inputs_allow_the_rounded_edge_a_stored_box_may_reach() {
        use crate::sources::uploads::exclusions::Exclusion;
        // A box ending at the edge of a 595.2756 x 841.8898 pt page is stored as 595.28 x 841.89.
        let mut upload = upload_of(1, vec![1]);
        upload.pages = vec![page_model(1, 595.2756, 841.8898)];
        upload.exclusion = Some(Exclusion {
            pages: vec![1],
            boxes: [(1, vec![[500.12, 800., 595.28, 841.89]])].into(),
            proposal_fingerprints: Vec::new(),
            rationale: "edge".into(),
        });
        let html = Screen::build(
            super::super::nav::Nav::logo_only(),
            upload,
            "t".repeat(64),
            false,
            Vec::new(),
            1,
        )
        .render()
        .unwrap();
        let number = |text: &str, after: &str| -> f64 {
            text.split(after)
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap()
                .parse()
                .unwrap()
        };
        for k in 0..4 {
            let at = html.find(&format!("name=\"box_1_{k}\"")).unwrap();
            let max: f64 = html[..at]
                .rsplit_once("max=\"")
                .unwrap()
                .1
                .split('"')
                .next()
                .unwrap()
                .parse()
                .unwrap();
            let value = number(&html[at..], "value=\"");
            assert!(value <= max, "box_1_{k}: value {value} over max {max}");
        }
    }

    #[test]
    fn extra_boxes_are_refused_unless_their_page_is_ticked_and_repeated_keys_are_refused() {
        let base: Vec<(String, String)> = [("exclude_5", "1"), ("box_5_0", "1"), ("box_5_1", "2")]
            .into_iter()
            .chain([("box_5_2", "3"), ("box_5_3", "4")])
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        // Further boxes of a page whose tick is off are handed to the server (which compares
        // them with the saved ones): parsed, not part of the manual boxes, and a malformed
        // line is still refused here.
        let mut orphan: Vec<(String, String)> = base[1..].to_vec();
        orphan.push(("more_5".into(), "5 6 7 8".into()));
        let form = form_map(orphan).unwrap();
        let request = exclusion_form(&form, "why".into()).unwrap();
        assert!(request.manual.is_empty());
        assert_eq!(request.unticked_extra[&5], [[5., 6., 7., 8.]]);
        let mut broken: Vec<(String, String)> = base[1..].to_vec();
        broken.push(("more_5".into(), "5 6 7".into()));
        assert!(exclusion_form(&form_map(broken).unwrap(), "why".into()).is_err());
        let mut fine = base.clone();
        fine.push(("more_5".into(), "5 6 7 8".into()));
        assert_eq!(
            exclusion_form(&form_map(fine).unwrap(), "why".into())
                .unwrap()
                .manual[&5]
                .len(),
            2
        );
        // A repeated key (two `confirm_<fp>` boxes, say) is refused: the last one must not win.
        let mut twice = base;
        twice.push(("exclude_5".into(), "1".into()));
        assert!(form_map(twice).is_err());
        let one = [("a".to_owned(), "1".to_owned())];
        assert_eq!(form_map(one.to_vec()).unwrap().len(), 1);
    }

    #[test]
    fn exclusion_form_posts_fingerprints_and_ranges_never_proposal_boxes() {
        let fingerprint = "ab".repeat(32);
        let form: BTreeMap<String, String> = [
            (format!("confirm_{fingerprint}"), "1".to_owned()),
            (format!("skip_{fingerprint}"), "3, 10-12".into()),
            (format!("absent_{fingerprint}"), "4".into()),
            ("extend_absent".into(), "1".into()),
            ("window_from".into(), "1".into()),
            ("window_to".into(), "100".into()),
            ("exclude_5".into(), "1".into()),
            ("box_5_0".into(), "1".into()),
            ("box_5_1".into(), "2".into()),
            ("box_5_2".into(), "3".into()),
            ("box_5_3".into(), "4".into()),
            ("more_5".into(), "5 6 7 8\n9,10,11,12\n\n".into()),
        ]
        .into();
        let request = exclusion_form(&form, "why".into()).unwrap();
        assert_eq!(request.confirmed.len(), 1);
        assert_eq!(request.confirmed[0].fingerprint, fingerprint);
        assert_eq!(request.confirmed[0].skip, [3, 10, 11, 12]);
        assert_eq!(request.confirmed[0].absent, [4]);
        assert!(request.extend_absent);
        assert_eq!(request.window, Some((1, 100)));
        assert_eq!(request.manual[&5].len(), 3);
        assert_eq!(request.manual[&5][2], [9., 10., 11., 12.]);
        // Unchecked confirmations are not confirmations; a malformed box line is refused.
        let mut form = form;
        form.insert(format!("confirm_{fingerprint}"), "0".into());
        form.insert("more_5".into(), "1 2 3".into());
        assert!(exclusion_form(&form, "why".into()).is_err());
        form.insert("more_5".into(), String::new());
        assert!(
            exclusion_form(&form, "why".into())
                .unwrap()
                .confirmed
                .is_empty()
        );
    }

    fn counter(body: Body, admitted: bool) -> Counting {
        Counting {
            body: body.into_data_stream(),
            pending: Bytes::new(),
            count: 0,
            file_bytes: Arc::new(AtomicU64::new(0)),
            admitted: Arc::new(AtomicBool::new(admitted)),
            status: Arc::new(AtomicU16::new(0)),
            idle: Duration::from_millis(10),
            since_yield: 0,
        }
    }
    #[tokio::test(start_paused = true)]
    async fn upload_idle_excludes_server_work() {
        let mut c = counter(Body::from("ready"), true);
        // Simulate time spent validating metadata or writing a previous chunk.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(c.read().await.unwrap().unwrap(), "ready");
    }
    #[tokio::test]
    async fn upload_parser_read_ahead_is_bounded_after_admission() {
        let supplied = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = supplied.clone();
        let prefix = b"--test\r\nContent-Disposition: form-data; name=\"file\"\r\n\r\n";
        let body = Body::from_stream(
            futures_util::stream::once(async {
                Ok::<_, std::io::Error>(Bytes::from_static(prefix))
            })
            .chain(futures_util::stream::iter((0..1024).map(move |_| {
                seen.fetch_add(16384, Ordering::Relaxed);
                Ok::<_, std::io::Error>(Bytes::from(vec![b'x'; 16384]))
            }))),
        );
        let c = counter(body, true);
        // The route credits each file chunk it takes; so does this test.
        let credit = c.file_bytes.clone();
        let stream =
            futures_util::stream::unfold(c, |mut c| async move { c.next().await.map(|b| (b, c)) });
        let mut parser = multer::Multipart::new(stream, "test");
        let mut field = parser.next_field().await.unwrap().unwrap();
        assert!(supplied.load(Ordering::Relaxed) <= 65536);
        let mut consumed = 0;
        for _ in 0..16 {
            let chunk = field.chunk().await.unwrap().unwrap();
            assert!(chunk.len() <= 2 * 65536 + 8, "{}", chunk.len());
            consumed += chunk.len();
            credit.fetch_add(chunk.len() as u64, Ordering::Release);
            assert!(supplied.load(Ordering::Relaxed) - consumed <= 2 * 65536 + 8);
        }
    }
    #[tokio::test(start_paused = true)]
    async fn upload_counting_has_an_idle_deadline_and_no_total_deadline() {
        let mut idle = counter(
            Body::from_stream(futures_util::stream::pending::<Result<Bytes, std::io::Error>>()),
            true,
        );
        assert_eq!(idle.read().await.unwrap_err(), StatusCode::REQUEST_TIMEOUT);
        // A client that always delivers within the 30 s idle window runs on for far longer than
        // the former 15-minute total deadline: 60 frames 20 s apart take 20 minutes.
        let start = tokio::time::Instant::now();
        let stream = futures_util::stream::unfold((), |()| async {
            tokio::time::sleep(Duration::from_secs(20)).await;
            Some((Ok::<_, std::io::Error>(Bytes::from_static(b"x")), ()))
        });
        let mut slow = counter(Body::from_stream(stream), true);
        slow.idle = IDLE;
        for _ in 0..60 {
            assert!(slow.read().await.unwrap().is_some());
        }
        assert!(tokio::time::Instant::now() - start >= Duration::from_mins(20));
        // The same client stalling for 31 s is cut off.
        let stalled = futures_util::stream::unfold((), |()| async {
            tokio::time::sleep(Duration::from_secs(31)).await;
            Some((Ok::<_, std::io::Error>(Bytes::from_static(b"x")), ()))
        });
        let mut stalled = counter(Body::from_stream(stalled), true);
        stalled.idle = IDLE;
        assert_eq!(
            stalled.read().await.unwrap_err(),
            StatusCode::REQUEST_TIMEOUT
        );
    }
    #[tokio::test]
    async fn upload_counting_limits_the_pre_admission_read_only() {
        let mut c = counter(Body::from(vec![b'x'; 8192]), false);
        for _ in 0..4096 {
            assert_eq!(c.read().await.unwrap().unwrap().len(), 1);
        }
        assert_eq!(c.read().await.unwrap_err(), StatusCode::PAYLOAD_TOO_LARGE);
        let mut c = counter(Body::from(vec![b'x'; 200_000]), true);
        let mut total = 0;
        while let Some(bytes) = c.read().await.unwrap() {
            total += bytes.len();
        }
        assert_eq!(total, 200_000);
    }
    #[tokio::test]
    async fn upload_counting_bounds_only_the_bytes_that_are_not_file_data() {
        // Uncredited bytes stop at the non-file budget.
        let mut c = counter(Body::from(vec![b'x'; 2 * 1024 * 1024]), true);
        let mut total = 0;
        let refused = loop {
            match c.read().await {
                Ok(Some(bytes)) => total += bytes.len(),
                Ok(None) => panic!("not refused"),
                Err(status) => break status,
            }
        };
        assert_eq!(refused, StatusCode::PAYLOAD_TOO_LARGE);
        assert!(u64::try_from(total).unwrap() <= NON_FILE + 16384);
        // The same bytes credited as file data pass.
        let mut c = counter(Body::from(vec![b'x'; 2 * 1024 * 1024]), true);
        let credit = c.file_bytes.clone();
        let mut total = 0;
        while let Some(bytes) = c.read().await.unwrap() {
            credit.fetch_add(bytes.len() as u64, Ordering::Release);
            total += bytes.len();
        }
        assert_eq!(total, 2 * 1024 * 1024);
    }
}
