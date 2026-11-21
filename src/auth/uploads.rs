//! Owner upload HTTP boundary. PDF bytes are never parsed in this process.
use super::{AuthResult, AuthState, csrf_admission, materials, owner_auth};
use crate::{
    AppError,
    sources::{
        uploads::{
            Metadata, OwnerWrite, PageModel, Upload, UploadPhase, UploadProgress, Uploads,
            exclusions::{COMPARISON_LIMIT, Exclusion, Proposal},
        },
        worker,
    },
};
use askama::Template;
use axum::{
    Router,
    body::Body,
    extract::{DefaultBodyLimit, Form, Path, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Redirect},
    routing::{get, post},
};
use bytes::Bytes;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::{
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
#[derive(Template)]
#[template(path = "upload_page.html")]
struct Screen {
    nav: super::nav::Nav,
    upload: Upload,
    csrf: String,
    refresh: bool,
    editors: Vec<Editor>,
}
struct Editor {
    title: String,
    text: String,
    fingerprint: String,
    pages: Vec<EditorPage>,
}
struct EditorPage {
    page: PageModel,
    bounds: [f64; 4],
    selected: bool,
    absent: bool,
}
struct Mask {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}
impl Screen {
    fn mask(&self, n: u32) -> Option<Mask> {
        self.upload.exclusion.as_ref()?.boxes.get(&n).map(|b| Mask {
            x: b[0],
            y: b[1],
            width: b[2] - b[0],
            height: b[3] - b[1],
        })
    }
    fn editors(upload: &Upload, proposals: Vec<Proposal>) -> Vec<Editor> {
        let mut editors: Vec<_> = proposals
            .into_iter()
            .map(|p| Editor {
                title: "Confirm or adjust repeated text".into(),
                text: p.text,
                fingerprint: p.fingerprint,
                pages: upload
                    .pages
                    .iter()
                    .filter(|page| upload.selected_pages.contains(&page.pdf_page))
                    .map(|page| EditorPage {
                        page: page.clone(),
                        selected: p.boxes.contains_key(&page.pdf_page),
                        absent: p.absent.contains_key(&page.pdf_page),
                        bounds: p
                            .boxes
                            .get(&page.pdf_page)
                            .or_else(|| p.absent.get(&page.pdf_page))
                            .copied()
                            .unwrap_or([0., 0., 1., 1.]),
                    })
                    .collect(),
            })
            .collect();
        editors.push(Editor {
            title: "Draw or adjust an exclusion by hand".into(),
            text: String::new(),
            fingerprint: String::new(),
            pages: upload
                .pages
                .iter()
                .filter(|p| upload.selected_pages.contains(&p.pdf_page))
                .map(|page| {
                    let saved = upload
                        .exclusion
                        .as_ref()
                        .and_then(|e| e.boxes.get(&page.pdf_page));
                    EditorPage {
                        page: page.clone(),
                        bounds: saved.copied().unwrap_or([
                            page.width_points / 4.,
                            page.height_points / 4.,
                            page.width_points / 2.,
                            page.height_points / 2.,
                        ]),
                        selected: saved.is_some(),
                        absent: false,
                    }
                })
                .collect(),
        });
        editors
    }
    fn selected(&self, n: u32) -> bool {
        self.upload.selected_pages.contains(&n)
    }
    fn busy(&self) -> bool {
        matches!(self.upload.job.state.as_str(), "queued" | "running")
    }
    fn hash(&self) -> &str {
        self.upload.pdf_sha256.get(..12).unwrap_or("")
    }
}
async fn model(state: &AuthState, session: &Session, id: String) -> Result<Screen, StatusCode> {
    let auth = owner_auth(state, session).await?;
    let m = manager(state)?;
    let upload = blocking(move || m.read_for_owner(&auth, &id)).await?;
    let refresh = matches!(upload.job.state.as_str(), "queued" | "running");
    Ok(Screen {
        nav: upload_nav(),
        editors: Screen::editors(&upload, vec![]),
        upload,
        csrf: super::token(session).await?.ok_or(StatusCode::FORBIDDEN)?,
        refresh,
    })
}
async fn page(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<String>,
) -> AuthResult {
    materials::html(&model(&state, &session, id).await?)
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
    Form(mut form): Form<std::collections::BTreeMap<String, String>>,
) -> AuthResult {
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
        let mut screen = model(&state, &session, id).await?;
        if screen.upload.revision != revision {
            return Err(StatusCode::CONFLICT);
        }
        screen.editors = Screen::editors(&screen.upload, proposals);
        screen.refresh = false;
        return materials::html(&screen);
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
                    let exclusion = exclusion_form(&form, rationale)?;
                    m.set_exclusion(
                        &owner,
                        &id,
                        revision,
                        excluded,
                        &exclusion,
                        form.get("extend_absent").is_some_and(|v| v == "1"),
                    )
                }
            }
            "set" => {
                let revision = revision()?;
                let metadata = Metadata {
                    system_id: form.remove("system").unwrap_or_default(),
                    edition: form.remove("edition").unwrap_or_default(),
                    language: form.remove("language").unwrap_or_default(),
                };
                let pages = form
                    .keys()
                    .filter(|k| k.starts_with("page_"))
                    .map(|k| {
                        k[5..]
                            .parse::<u32>()
                            .map_err(|_| AppError::Invalid("Invalid selected page."))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let mut pages = pages;
                pages.sort_unstable();
                m.set(&owner, &id, revision, &metadata, &pages)
            }
            _ => Err(AppError::SourceNotFound),
        }
    })
    .await?;
    Ok(Redirect::to(&target).into_response())
}
fn exclusion_form(
    form: &std::collections::BTreeMap<String, String>,
    rationale: String,
) -> Result<Exclusion, AppError> {
    let mut boxes = std::collections::BTreeMap::new();
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
        boxes.insert(page, bounds);
    }
    Ok(Exclusion {
        pages: boxes.keys().copied().collect(),
        boxes,
        rationale,
        proposal_fingerprint: form.get("fingerprint").filter(|s| !s.is_empty()).cloned(),
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

#[cfg(test)]
mod tests {
    use super::*;

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
