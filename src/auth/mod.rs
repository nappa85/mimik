//! Trusted-LAN/proxy authentication boundary and request-scoped campaign admissions.
mod administration;
mod errors;
mod materials;
mod persistence;
mod rulesets;
pub use persistence::Principal;
pub use persistence::SqliteSessions;

use crate::{
    AppError, Store,
    accounts::{Accounts, Summary},
};
use askama::Template;
use axum::{
    Router,
    extract::{DefaultBodyLimit, Form, Request, State},
    http::{Method, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use governor::{DefaultDirectRateLimiter, Quota, RateLimiter};
use serde::Deserialize;
use std::{
    num::NonZeroU32,
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use subtle::ConstantTimeEq;
use tower_sessions::{Expiry, Session, SessionManagerLayer, cookie::SameSite};
use zeroize::Zeroizing;

static REQUESTS: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(8)));
#[cfg(not(test))]
static QUOTA: LazyLock<Arc<DefaultDirectRateLimiter>> = LazyLock::new(new_quota);
fn new_quota() -> Arc<DefaultDirectRateLimiter> {
    Arc::new(RateLimiter::direct(
        Quota::with_period(Duration::from_secs(6))
            .expect("nonzero quota period")
            .allow_burst(NonZeroU32::new(10).expect("nonzero burst")),
    ))
}
tokio::task_local! { static STORAGE_BUSY: Arc<AtomicBool>; }
tokio::task_local! {static DOCUMENT_PRESENTATION:bool;}
fn document_presentation() -> bool {
    DOCUMENT_PRESENTATION
        .try_with(|value| *value)
        .unwrap_or(false)
}
tokio::task_local! {
    static READ_ADMISSION: std::cell::Cell<Option<(tower_sessions::session::Id, i64)>>;
}

thread_local! { static DOMAIN_REQUEST: std::cell::RefCell<Option<RequestAuth>> = const {std::cell::RefCell::new(None)}; }

pub(crate) fn register_domain_guard(db: &rusqlite::Connection) -> Result<(), AppError> {
    db.create_scalar_function("mimik_request_auth",0,rusqlite::functions::FunctionFlags::SQLITE_UTF8,|_|{
        Ok(DOMAIN_REQUEST.with(|ctx|ctx.borrow().as_ref().map(|auth|serde_json::json!({"id":auth.original_id.to_string(),"account":auth.account.id,"epoch":auth.account.epoch,"deadline":auth.deadline,"development_owner_play":auth.development_owner_play,"digest":auth.digest()}).to_string())))
    })?;
    Ok(())
}

pub(crate) fn with_domain_request<T>(auth: RequestAuth, operation: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            DOMAIN_REQUEST.with(|ctx| *ctx.borrow_mut() = None);
        }
    }
    DOMAIN_REQUEST.with(|ctx| *ctx.borrow_mut() = Some(auth));
    let _reset = Reset;
    operation()
}

fn remember_session_read(record: &tower_sessions::session::Record) {
    let _ = READ_ADMISSION.try_with(|captured| {
        if captured.get().is_none() {
            captured.set(Some((record.id, record.expiry_date.unix_timestamp())));
        }
    });
}

pub(crate) fn mark_storage_busy() {
    let _ = STORAGE_BUSY.try_with(|busy| busy.store(true, Ordering::Release));
}

#[derive(Clone)]
pub(crate) struct AuthState {
    accounts: Accounts,
    sessions: SqliteSessions,
    quota: Arc<DefaultDirectRateLimiter>,
    options: AuthOptions,
    #[cfg(test)]
    before_current: Option<Arc<dyn Fn() + Send + Sync>>,
}

#[derive(Clone, Default)]
pub struct AuthOptions {
    pub development_owner_play: bool,
    pub network: crate::network::Network,
    pub external_url: Option<url::Url>,
}
impl AuthOptions {
    #[must_use]
    pub fn origin(&self) -> String {
        self.external_url.as_ref().map_or_else(
            || self.network.origin(),
            |url| url.origin().ascii_serialization(),
        )
    }
}

/// Construct the distinct auth router without starting background tasks.
/// # Errors
/// Requires a durable Owner binding and working bounded storage.
pub async fn app(store: Store) -> Result<Router, AppError> {
    app_with_options(store, AuthOptions::default()).await
}

/// Construct auth mode with an explicit trusted development option.
/// # Errors
/// Requires the same durable Owner binding and bounded storage as normal auth mode.
pub async fn app_with_options(store: Store, options: AuthOptions) -> Result<Router, AppError> {
    Ok(router(app_state(store, options).await?))
}

async fn app_state(store: Store, options: AuthOptions) -> Result<AuthState, AppError> {
    crate::network::Access::new(options.network, options.external_url.clone())?;
    let accounts = Accounts::new(store.clone());
    accounts.require_owner_binding().await?;
    Ok(AuthState {
        accounts,
        sessions: SqliteSessions::new(store),
        #[cfg(not(test))]
        quota: QUOTA.clone(),
        #[cfg(test)]
        quota: new_quota(),
        options,
        #[cfg(test)]
        before_current: None,
    })
}

/// Supply deterministic store rendezvous/faults to the real router in integration tests.
/// This constructor is excluded from shipping defaults and starts no background task.
/// # Errors
/// Requires the same durable Owner binding and bounded storage as the shipping router.
#[cfg(feature = "auth-test-support")]
pub async fn app_with_test_sessions(
    store: Store,
    sessions: impl tower_sessions::SessionStore + Clone,
) -> Result<Router, AppError> {
    let mut state = app_state(store, AuthOptions::default()).await?;
    // Synthetic routers get an independent instance of the same guarded quota.
    state.quota = new_quota();
    Ok(router_with_sessions(state, sessions))
}

fn router(state: AuthState) -> Router {
    let sessions = state.sessions.clone();
    router_with_sessions(state, sessions)
}

fn router_with_sessions(
    state: AuthState,
    sessions: impl tower_sessions::SessionStore + Clone,
) -> Router {
    let access =
        crate::network::Access::new(state.options.network, state.options.external_url.clone())
            .expect("validated auth network");
    let layer = SessionManagerLayer::new(sessions)
        .with_name("mimik_session")
        .with_secure(false)
        .with_http_only(true)
        .with_same_site(SameSite::Strict)
        .with_path("/");
    Router::new()
        .route("/", get(landing))
        .route("/auth/login", get(login_form).post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/session", get(status))
        .merge(administration::routes())
        .merge(materials::routes())
        .merge(rulesets::routes())
        .merge(crate::campaigns::web::routes())
        .route("/assets/app.css", get(crate::web::css_asset))
        .route("/assets/logo.png", get(crate::web::logo_asset))
        .fallback(|| async { StatusCode::FORBIDDEN })
        .with_state(state)
        .layer(DefaultBodyLimit::max(8192))
        .layer(layer)
        .layer(middleware::from_fn(post_origin))
        .layer(middleware::from_fn_with_state(
            access,
            crate::network::admission,
        ))
        .layer(middleware::from_fn(boundary))
}

async fn boundary(request: Request, next: Next) -> Response {
    let browser_navigation = browser_auth_navigation(&request);
    let document = browser_document(&request);
    let response = if let Ok(_permit) = REQUESTS.clone().try_acquire_owned() {
        let busy = Arc::new(AtomicBool::new(false));
        let result = STORAGE_BUSY
            .scope(
                busy.clone(),
                READ_ADMISSION.scope(
                    std::cell::Cell::new(None),
                    DOCUMENT_PRESENTATION.scope(
                        document,
                        tokio::time::timeout(Duration::from_secs(5), next.run(request)),
                    ),
                ),
            )
            .await;
        match result {
            Ok(response)
                if !busy.load(Ordering::Acquire)
                    || (response.status() == StatusCode::SERVICE_UNAVAILABLE
                        && (response
                            .extensions()
                            .get::<administration::CredentialRetry>()
                            .is_some()
                            || response.extensions().get::<errors::LoginRetry>().is_some())) =>
            {
                response
            }
            Ok(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
            Err(_) => StatusCode::REQUEST_TIMEOUT.into_response(),
        }
    } else {
        StatusCode::SERVICE_UNAVAILABLE.into_response()
    };
    if browser_navigation && response.status() == StatusCode::UNAUTHORIZED {
        let mut redirect = Redirect::to("/auth/login").into_response();
        for cookie in response.headers().get_all(header::SET_COOKIE) {
            redirect
                .headers_mut()
                .append(header::SET_COOKIE, cookie.clone());
        }
        normalize(redirect)
    } else {
        let trusted_editor = response
            .extensions()
            .get::<crate::campaigns::web::PrivateTrustedEditor>()
            .is_some();
        let trusted_credential = response
            .extensions()
            .get::<administration::CredentialRetry>()
            .is_some();
        let response = if document
            && (response.status().is_client_error() || response.status().is_server_error())
            && !trusted_editor
            && !trusted_credential
            && !crate::campaigns::web::catalogue_notice(&response)
            && !crate::campaigns::web::sheet_busy(&response)
            && !materials::trusted(&response)
            && !errors::trusted(&response)
        {
            errors::document(response)
        } else {
            response
        };
        normalize(response)
    }
}

async fn landing(State(state): State<AuthState>, session: Session) -> AuthResult {
    let target = if principal(&state, &session).await?.is_some() {
        "/auth/session"
    } else {
        "/auth/login"
    };
    Ok(Redirect::to(target).into_response())
}

fn positive_quality(value: &str) -> Option<bool> {
    match value {
        "0" => Some(false),
        "1" => Some(true),
        _ => {
            let (whole, fraction) = value.split_once('.')?;
            if fraction.is_empty()
                || fraction.len() > 3
                || !fraction.bytes().all(|b| b.is_ascii_digit())
            {
                return None;
            }
            match whole {
                "0" => Some(fraction.bytes().any(|b| b != b'0')),
                "1" if fraction.bytes().all(|b| b == b'0') => Some(true),
                _ => None,
            }
        }
    }
}

fn single_header<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    let mut values = request.headers().get_all(name).iter();
    let first = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    Some(first)
}

fn browser_auth_navigation(request: &Request) -> bool {
    let path = request.uri().path();
    request.method() == Method::GET
        && (path == "/auth/session"
            || path == "/admin"
            || path.starts_with("/admin/")
            || path == "/campaigns"
            || path.starts_with("/campaigns/")
            || path == "/private-sheets"
            || path.starts_with("/private-sheets/"))
        && browser_document(request)
}
fn browser_document(request: &Request) -> bool {
    static QUOTED_QUALITY: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"(?i);[ \t]*q[ \t]*=[ \t]*"[0-9.]+""#)
            .expect("static quality-syntax pattern")
    });
    let has_fetch_metadata = request.headers().contains_key("sec-fetch-mode")
        || request.headers().contains_key("sec-fetch-dest");
    if has_fetch_metadata
        && (single_header(request, "sec-fetch-mode") != Some("navigate")
            || single_header(request, "sec-fetch-dest") != Some("document"))
    {
        return false;
    }
    let mut html = None;
    for value in request.headers().get_all(header::ACCEPT) {
        let Ok(value) = value.to_str() else {
            return false;
        };
        for range in value.split(',') {
            let Ok(media) = range.trim().parse::<mime::Mime>() else {
                return false;
            };
            // MIME supports quoted parameter values, but HTTP weights do not.
            // Preserve that raw syntax rather than trusting the decoded value.
            if QUOTED_QUALITY.is_match(media.as_ref()) {
                return false;
            }
            let mut quality = None;
            for (name, value) in media.params() {
                if name == "q" {
                    if quality.is_some() {
                        return false;
                    }
                    let Some(positive) = positive_quality(value.as_str()) else {
                        return false;
                    };
                    quality = Some(positive);
                }
            }
            if media.essence_str() == mime::TEXT_HTML.essence_str() {
                if html.is_some() {
                    return false;
                }
                html = Some(quality.unwrap_or(true));
            }
        }
    }
    html == Some(true)
}

fn normalize(mut response: Response) -> Response {
    let status = response.status();
    if (status.is_client_error() || status.is_server_error())
        && (response
            .extensions()
            .get::<administration::CredentialRetry>()
            .is_some()
            || errors::trusted(&response)
            || materials::trusted(&response))
    {
        response.headers_mut().remove(header::SET_COOKIE);
    }
    // Pages that explain their own error keep their body at any status they are marked for.
    let explained = errors::trusted(&response)
        || materials::trusted(&response)
        || crate::campaigns::web::catalogue_notice(&response)
        || crate::campaigns::web::sheet_busy(&response);
    if (status.is_client_error() || status.is_server_error())
        && !explained
        && !(matches!(
            status,
            StatusCode::FORBIDDEN
                | StatusCode::CONFLICT
                | StatusCode::UNPROCESSABLE_ENTITY
                | StatusCode::TOO_MANY_REQUESTS
                | StatusCode::SERVICE_UNAVAILABLE
        ) && response
            .extensions()
            .get::<administration::CredentialRetry>()
            .is_some())
        && !(matches!(
            status,
            StatusCode::CONFLICT | StatusCode::UNPROCESSABLE_ENTITY
        ) && response
            .extensions()
            .get::<crate::campaigns::web::PrivateTrustedEditor>()
            .is_some())
    {
        let text = errors::plain(status);
        let retry = response.headers().get(header::RETRY_AFTER).cloned();
        response = (status, text).into_response();
        if let Some(retry) = retry {
            response.headers_mut().insert(header::RETRY_AFTER, retry);
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, "6".parse().expect("static retry"));
        }
        if status == StatusCode::SERVICE_UNAVAILABLE
            && !response.headers().contains_key(header::RETRY_AFTER)
        {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, "1".parse().expect("static retry"));
        }
    }
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store".parse().expect("static header"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        "nosniff".parse().expect("static header"),
    );
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        "frame-ancestors 'none'".parse().expect("static header"),
    );
    response.headers_mut().insert(
        header::X_FRAME_OPTIONS,
        "DENY".parse().expect("static header"),
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        "same-origin".parse().expect("static header"),
    );
    response
}

async fn post_origin(request: Request, next: Next) -> Response {
    if request.method() == Method::POST {
        let Some(expected) = request
            .extensions()
            .get::<crate::network::ExpectedOrigin>()
            .map(|origin| origin.value.as_str())
        else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        let mut origins = request.headers().get_all(header::ORIGIN).iter();
        if origins.next().and_then(|v| v.to_str().ok()) != Some(expected)
            || origins.next().is_some()
        {
            return StatusCode::FORBIDDEN.into_response();
        }
        let mut content = request.headers().get_all(header::CONTENT_TYPE).iter();
        let valid = content
            .next()
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.split(';').next() == Some("application/x-www-form-urlencoded"));
        if !valid || content.next().is_some() {
            return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
        }
    }
    next.run(request).await
}

type AuthResult = Result<Response, StatusCode>;
fn storage(error: &AppError) -> StatusCode {
    match error {
        AppError::PasswordBusy | AppError::AuthStorageBusy => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
fn session_error(_: tower_sessions::session::Error) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}
fn throttle(state: &AuthState) -> Result<(), StatusCode> {
    state
        .quota
        .check()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)
}

fn csrf() -> Result<String, StatusCode> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(hex::encode(bytes))
}
pub(crate) async fn token(session: &Session) -> Result<Option<String>, StatusCode> {
    session.get("csrf").await.map_err(session_error)
}
pub(crate) async fn check_csrf(session: &Session, submitted: &str) -> Result<(), StatusCode> {
    let expected = token(session).await?.ok_or(StatusCode::FORBIDDEN)?;
    if submitted.len() != 64 || !bool::from(expected.as_bytes().ct_eq(submitted.as_bytes())) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

/// A domain admission captured from the library session, never from request fields.
#[derive(Clone)]
pub(crate) struct RequestAuth {
    account: Principal,
    original_id: tower_sessions::session::Id,
    deadline: i64,
    development_owner_play: bool,
}

impl RequestAuth {
    #[cfg(test)]
    pub(crate) fn fixture_cookie(&self) -> String {
        format!("mimik_session={}", self.original_id)
    }
    #[cfg(test)]
    pub(crate) fn csrf_fixture(&self) -> CsrfAdmission {
        CsrfAdmission {
            id: self.original_id,
            deadline: self.deadline,
            token: "a".repeat(64),
            principal: Some(self.account),
        }
    }
    /// A durable CSRF admission for this session whose token no longer matches.
    #[cfg(test)]
    pub(crate) fn stale_csrf_fixture(&self) -> CsrfAdmission {
        CsrfAdmission {
            token: "b".repeat(64),
            ..self.csrf_fixture()
        }
    }
    #[cfg(test)]
    pub(crate) fn fixture(account: i64, id: tower_sessions::session::Id, deadline: i64) -> Self {
        Self {
            account: Principal {
                id: account,
                epoch: 1,
            },
            original_id: id,
            deadline,
            development_owner_play: false,
        }
    }
    pub(crate) fn account_id(&self) -> i64 {
        self.account.id
    }
    pub(crate) fn epoch(&self) -> i64 {
        self.account.epoch
    }
    pub(crate) fn development_owner_play(&self) -> bool {
        self.development_owner_play
    }
    #[cfg(test)]
    pub(crate) fn development_fixture(mut self) -> Self {
        self.development_owner_play = true;
        self
    }
    pub(crate) fn digest(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        hash.update(b"mimik-history-session-v1\0");
        hash.update(self.original_id.to_string().as_bytes());
        hex::encode(hash.finalize())
    }
    pub(crate) fn recheck(&self, db: &rusqlite::Connection) -> Result<i64, AppError> {
        self.recheck_binding(db, self.development_owner_play)
    }
    pub(crate) fn recheck_owner(&self, db: &rusqlite::Connection) -> Result<i64, AppError> {
        self.recheck_binding(db, true)?;
        if !db.query_row(
            "SELECT EXISTS(SELECT 1 FROM account_owner WHERE account_id=?1)",
            [self.account.id],
            |row| row.get::<_, bool>(0),
        )? {
            return Err(AppError::NotFound);
        }
        let now = persistence::now(db)?;
        if now >= self.deadline {
            return Err(AppError::NotFound);
        }
        Ok(now)
    }
    fn recheck_binding(
        &self,
        db: &rusqlite::Connection,
        allow_owner: bool,
    ) -> Result<i64, AppError> {
        let valid: bool=db.query_row("SELECT EXISTS(SELECT 1 FROM auth_sessions s JOIN accounts a ON a.id=s.account_id WHERE s.id=?1 AND s.state='active' AND s.expires_at=?2 AND s.account_id=?3 AND s.epoch=?4 AND a.epoch=?4 AND a.state='enabled' AND json_extract(s.payload,'$.principal.id')=?3 AND json_extract(s.payload,'$.principal.epoch')=?4 AND (?5 OR NOT EXISTS(SELECT 1 FROM account_owner o WHERE o.account_id=a.id)))", rusqlite::params![self.original_id.to_string(),self.deadline,self.account.id,self.account.epoch,allow_owner], |r|r.get(0))?;
        // Sample after potentially blocking row retrieval; equality is expired.
        let now = persistence::now(db)?;
        if !valid || now >= self.deadline {
            return Err(AppError::NotFound);
        }
        Ok(now)
    }
}

pub(crate) async fn request_auth(
    state: &AuthState,
    session: &Session,
) -> Result<RequestAuth, StatusCode> {
    let account = principal(state, session)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if account.owner && !state.options.development_owner_play {
        return Err(StatusCode::FORBIDDEN);
    }
    let (original_id, deadline) = READ_ADMISSION
        .try_with(std::cell::Cell::get)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    Ok(RequestAuth {
        account: Principal {
            id: account.id,
            epoch: account.epoch,
        },
        original_id,
        deadline,
        development_owner_play: state.options.development_owner_play,
    })
}

impl AuthState {
    pub(crate) fn store(&self) -> Store {
        self.sessions.store.clone()
    }
}
/// Whether the session carries a principal binding at all (cookie-backed session data only;
/// no account lookup and no request body). Callers still authenticate fully afterwards.
pub(crate) async fn has_principal(session: &Session) -> Result<bool, StatusCode> {
    let binding: Option<Principal> = session.get("principal").await.map_err(session_error)?;
    Ok(binding.is_some())
}

async fn principal(state: &AuthState, session: &Session) -> Result<Option<Summary>, StatusCode> {
    let binding: Option<Principal> = session.get("principal").await.map_err(session_error)?;
    let Some(binding) = binding else {
        return Ok(None);
    };
    #[cfg(test)]
    if let Some(hook) = &state.before_current {
        hook();
    }
    let current = state
        .accounts
        .current(binding.id, binding.epoch)
        .await
        .map_err(|error| storage(&error))?;
    if current.is_none() {
        session.flush().await.map_err(session_error)?;
        return Ok(None);
    }
    // Read admission is after both potentially blocking reads: the account
    // snapshot above and this final session row/deadline check. Later account
    // transitions cannot retract an admitted response (there are no writes here).
    let Some(id) = session.id() else {
        return Ok(None);
    };
    // Session does not inherit a loaded Record's expiry. Preserve the original
    // adapter admission before the final reload, including across ID recreation.
    let captured = READ_ADMISSION
        .try_with(std::cell::Cell::get)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let admitted = tower_sessions::SessionStore::load(&state.sessions, &id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let Some(admitted) = admitted else {
        return Ok(None);
    };
    if captured != (admitted.id, admitted.expiry_date.unix_timestamp())
        || admitted.data.get("principal")
            != Some(&serde_json::to_value(binding).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?)
    {
        return Ok(None);
    }
    Ok(current)
}

#[derive(Template)]
#[template(path = "auth_login.html")]
struct LoginTemplate<'a> {
    csrf: &'a str,
    login: &'a str,
    message: &'a str,
}
#[derive(Template)]
#[template(path = "auth_session.html")]
struct StatusTemplate<'a> {
    account: &'a Summary,
    csrf: &'a str,
    development_owner_play: bool,
}

async fn owner_auth(state: &AuthState, session: &Session) -> Result<RequestAuth, StatusCode> {
    let account = principal(state, session)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !account.owner {
        return Err(StatusCode::FORBIDDEN);
    }
    let (original_id, deadline) = READ_ADMISSION
        .try_with(std::cell::Cell::get)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    Ok(RequestAuth {
        account: Principal {
            id: account.id,
            epoch: account.epoch,
        },
        original_id,
        deadline,
        development_owner_play: false,
    })
}

async fn login_form(State(state): State<AuthState>, session: Session) -> AuthResult {
    let binding: Option<Principal> = session.get("principal").await.map_err(session_error)?;
    if binding.is_some() && principal(&state, &session).await?.is_none() {
        return Ok(Redirect::to("/auth/login").into_response());
    }
    let token = form_token(&state, &session).await?;
    Ok(Html(
        LoginTemplate {
            csrf: &token,
            login: "",
            message: "",
        }
        .render()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    )
    .into_response())
}

async fn form_token(state: &AuthState, session: &Session) -> Result<String, StatusCode> {
    let token = if let Some(token) = token(session).await? {
        token
    } else {
        throttle(state)?;
        let token = csrf()?;
        let now = state
            .sessions
            .store
            .run_bounded_auth(|db| persistence::now(db))
            .await
            .map_err(|error| storage(&error))?;
        session.set_expiry(Some(Expiry::AtDateTime(
            time::OffsetDateTime::from_unix_timestamp(
                now.checked_add(600)
                    .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?,
            )
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        )));
        session
            .insert("csrf", &token)
            .await
            .map_err(session_error)?;
        session.save().await.map_err(session_error)?;
        token
    };
    Ok(token)
}

#[derive(Clone)]
pub(crate) struct CsrfAdmission {
    id: tower_sessions::session::Id,
    deadline: i64,
    token: String,
    principal: Option<Principal>,
}
impl CsrfAdmission {
    pub(crate) fn recheck(&self, db: &rusqlite::Connection) -> Result<i64, AppError> {
        let valid:bool = db.query_row("SELECT EXISTS(SELECT 1 FROM auth_sessions s WHERE s.id=?1 AND s.state='active' AND s.expires_at=?2 AND json_extract(s.payload,'$.csrf')=?3 AND ((?4 IS NULL AND s.account_id IS NULL AND json_type(s.payload,'$.principal') IS NULL) OR (s.account_id=?4 AND s.epoch=?5 AND json_extract(s.payload,'$.principal.id')=?4 AND json_extract(s.payload,'$.principal.epoch')=?5 AND EXISTS(SELECT 1 FROM accounts a WHERE a.id=?4 AND a.epoch=?5 AND a.state='enabled'))))", rusqlite::params![self.id.to_string(),self.deadline,self.token,self.principal.map(|p|p.id),self.principal.map(|p|p.epoch)], |r|r.get(0))?;
        let now = persistence::now(db)?;
        if !valid || now >= self.deadline {
            return Err(AppError::NotFound);
        }
        Ok(now)
    }
}
async fn csrf_admission(session: &Session, submitted: &str) -> Result<CsrfAdmission, StatusCode> {
    check_csrf(session, submitted).await?;
    let (id, deadline) = READ_ADMISSION
        .try_with(std::cell::Cell::get)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::FORBIDDEN)?;
    Ok(CsrfAdmission {
        id,
        deadline,
        token: submitted.into(),
        principal: session.get("principal").await.map_err(session_error)?,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginForm {
    login: String,
    password: String,
    csrf: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LogoutForm {
    csrf: String,
}

async fn login(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<LoginForm>,
) -> AuthResult {
    let candidate = form.login.trim().to_ascii_lowercase();
    let retained = candidate.chars().take(64).collect::<String>();
    let password = Zeroizing::new(form.password.into_bytes());
    let quota = throttle(&state);
    let admission = match csrf_admission(&session, &form.csrf).await {
        Ok(admission) => admission,
        Err(status) => {
            return if status == StatusCode::FORBIDDEN {
                errors::login(
                    status,
                    &retained,
                    "",
                    "Your sign-in form expired or could not be verified. Open a fresh sign-in form below.",
                )
            } else {
                Err(status)
            };
        }
    };
    if let Err(status) = quota {
        return errors::login(
            status,
            &retained,
            &form.csrf,
            "Too many sign-in attempts. Wait six seconds, then try again.",
        );
    }
    if let Err(status) = recheck_login_form(&state, admission.clone()).await {
        return login_form_problem(status, &retained, &form.csrf);
    }
    let result = state
        .accounts
        .authenticate(candidate, password)
        .await
        .map_err(|error| storage(&error));
    if result.is_ok()
        && let Err(status) = recheck_login_form(&state, admission).await
    {
        return login_form_problem(status, &retained, &form.csrf);
    }
    let account = match result {
        Ok(Some(account)) => account,
        Ok(None) => {
            return errors::login(
                StatusCode::UNAUTHORIZED,
                &retained,
                &form.csrf,
                "Sign-in failed. Check your login name and password.",
            );
        }
        Err(StatusCode::SERVICE_UNAVAILABLE) => {
            return errors::login(
                StatusCode::SERVICE_UNAVAILABLE,
                &retained,
                &form.csrf,
                "The server is busy. Wait a moment, then try again.",
            );
        }
        Err(status) => return Err(status),
    };
    session.cycle_id().await.map_err(session_error)?;
    session.clear().await;
    session
        .insert(
            "principal",
            Principal {
                id: account.id,
                epoch: account.epoch,
            },
        )
        .await
        .map_err(session_error)?;
    session
        .insert("csrf", csrf()?)
        .await
        .map_err(session_error)?;
    let now = state
        .sessions
        .store
        .run_bounded_auth(|db| persistence::now(db))
        .await
        .map_err(|error| storage(&error))?;
    session.set_expiry(Some(Expiry::AtDateTime(
        time::OffsetDateTime::from_unix_timestamp(
            now.checked_add(28800)
                .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?,
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    )));
    session.save().await.map_err(session_error)?;
    Ok(Redirect::to("/auth/session").into_response())
}
async fn recheck_login_form(state: &AuthState, admission: CsrfAdmission) -> Result<(), StatusCode> {
    state
        .store()
        .run_bounded_auth(move |db| admission.recheck(db).map(|_| ()))
        .await
        .map_err(|error| match error {
            AppError::NotFound => StatusCode::FORBIDDEN,
            other => storage(&other),
        })
}
fn login_form_problem(status: StatusCode, login: &str, csrf: &str) -> AuthResult {
    match status {
        StatusCode::FORBIDDEN => errors::login(
            status,
            login,
            "",
            "Your sign-in form expired or was revoked. Open a fresh sign-in form below.",
        ),
        StatusCode::SERVICE_UNAVAILABLE => errors::login(
            status,
            login,
            csrf,
            "The server is busy. Wait a moment, then try again.",
        ),
        _ => Err(status),
    }
}
async fn status(State(state): State<AuthState>, session: Session) -> AuthResult {
    let account = principal(&state, &session)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let token = token(&session)
        .await?
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Html(
        StatusTemplate {
            account: &account,
            csrf: &token,
            development_owner_play: state.options.development_owner_play,
        }
        .render()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    )
    .into_response())
}
async fn logout(session: Session, Form(form): Form<LogoutForm>) -> AuthResult {
    check_csrf(&session, &form.csrf).await?;
    session.flush().await.map_err(session_error)?;
    Ok(Redirect::to("/auth/login").into_response())
}

/// Serve with observed cleanup and signal outcomes. Dropping the service cancels
/// the cleanup future; already admitted atomic SQL work may finish detached.
/// # Errors
/// Cleanup, listener and signal errors end the service.
pub async fn serve(
    listener: tokio::net::TcpListener,
    store: Store,
) -> Result<(), Box<dyn std::error::Error>> {
    serve_with_options(listener, store, AuthOptions::default()).await
}

/// Serve with an explicit development option; normal callers use safe defaults.
/// # Errors
/// Initialization, cleanup, listener and signal errors end the service.
pub async fn serve_with_options(
    listener: tokio::net::TcpListener,
    store: Store,
    options: AuthOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let router = app_with_options(store.clone(), options.clone()).await?;
    if options.development_owner_play {
        println!("DEVELOPMENT ONLY: Owner gameplay override enabled.");
    }
    println!(
        "Mimik auth mode: {} — private campaign workspace",
        options.origin()
    );
    let sessions = SqliteSessions::new(store);
    tokio::select! {
        result=axum::serve(listener,router.into_make_service_with_connect_info::<std::net::SocketAddr>())=>{result?;},
        result=cleanup_loop(sessions)=>{result?;},
        result=tokio::signal::ctrl_c()=>{result?;},
    }
    Ok(())
}

async fn cleanup_loop(sessions: SqliteSessions) -> Result<(), AppError> {
    let mut interval = tokio::time::interval(Duration::from_mins(1));
    interval.tick().await;
    loop {
        interval.tick().await;
        sessions.cleanup().await?;
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) async fn test_resource_lock() -> tokio::sync::OwnedMutexGuard<()> {
    tests::AUTH_TEST_LOCK.clone().lock_owned().await
}
