//! Full-document account administration and manual credential redemption.
use super::{AuthResult, AuthState, check_csrf, csrf_admission, form_token, owner_auth, throttle};
use crate::{
    AppError,
    accounts::{Summary, browser::Issued, digest_bytes},
    pagination::Listing,
};
use askama::Template;
use axum::{
    Router,
    extract::{Form, Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect},
    routing::{get, post},
};
use serde::Deserialize;
use tower_sessions::Session;
use zeroize::Zeroizing;

pub(super) fn routes() -> Router<AuthState> {
    Router::new()
        .route("/admin", get(directory))
        .route("/admin/invite", post(invite))
        .route("/admin/accounts/{id}", post(change))
        .route("/auth/enroll", get(enroll_form).post(enroll))
        .route("/auth/reset", get(reset_form).post(reset))
        .route(
            "/assets/account-sharing.js",
            get(crate::presentation::script_asset),
        )
}
#[derive(Clone)]
pub(super) struct CredentialRetry;
fn retry_form(
    csrf: &str,
    purpose: &str,
    token: &str,
    status: StatusCode,
    message: &str,
) -> AuthResult {
    let body = Redemption {
        csrf,
        purpose,
        token,
        message,
    }
    .render()
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut response = (status, Html(body)).into_response();
    response.extensions_mut().insert(CredentialRetry);
    if status == StatusCode::TOO_MANY_REQUESTS {
        response.headers_mut().insert(
            axum::http::header::RETRY_AFTER,
            "6".parse().expect("static retry"),
        );
    }
    if status == StatusCode::SERVICE_UNAVAILABLE {
        response.headers_mut().insert(
            axum::http::header::RETRY_AFTER,
            "1".parse().expect("static retry"),
        );
    }
    Ok(response)
}
fn error(error: &AppError) -> StatusCode {
    match error {
        AppError::NotFound => StatusCode::FORBIDDEN,
        AppError::Conflict => StatusCode::CONFLICT,
        AppError::Invalid(_) => StatusCode::UNPROCESSABLE_ENTITY,
        AppError::PasswordBusy | AppError::AuthStorageBusy => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct Cursor {
    after: i64,
}
#[derive(Template)]
#[template(path = "admin.html")]
struct Directory<'a> {
    nav: super::nav::Nav,
    rows: &'a [Summary],
    next: Option<i64>,
    csrf: &'a str,
}
async fn directory(
    State(state): State<AuthState>,
    session: Session,
    Query(cursor): Query<Cursor>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let Listing { rows, next } = state
        .accounts
        .browser_list(auth, cursor.after)
        .await
        .map_err(|err| error(&err))?;
    let csrf = super::token(&session).await?.ok_or(StatusCode::FORBIDDEN)?;
    Ok(Html(
        Directory {
            nav: super::nav::Nav::here(super::nav::Section::Accounts),
            rows: &rows,
            next,
            csrf: &csrf,
        }
        .render()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    )
    .into_response())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Invite {
    csrf: String,
    login: String,
    display_name: String,
    epoch: i64,
}
async fn invite(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<Invite>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    check_csrf(&session, &form.csrf).await?;
    issuance(
        &state.options.origin(),
        &state
            .accounts
            .browser_invite(auth, form.login, form.display_name, form.epoch)
            .await
            .map_err(|err| error(&err))?,
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    csrf: String,
    epoch: i64,
    action: Action,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Reset,
    Disable,
}
async fn change(
    State(state): State<AuthState>,
    session: Session,
    Path(id): Path<i64>,
    Form(form): Form<Change>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    check_csrf(&session, &form.csrf).await?;
    match form.action {
        Action::Reset => issuance(
            &state.options.origin(),
            &state
                .accounts
                .browser_reset(auth, id, form.epoch)
                .await
                .map_err(|err| error(&err))?,
        ),
        Action::Disable => {
            state
                .accounts
                .browser_disable(auth, id, form.epoch)
                .await
                .map_err(|err| error(&err))?;
            Ok(Redirect::to("/admin").into_response())
        }
    }
}
#[derive(Template)]
#[template(path = "account_issued.html")]
struct Issuance<'a> {
    nav: super::nav::Nav,
    account: &'a Summary,
    token: &'a str,
    link: &'a str,
    qr: &'a str,
    purpose: &'a str,
    expires: &'a crate::presentation::DisplayTime,
}
fn issuance(origin: &str, issued: &Issued) -> AuthResult {
    let token =
        std::str::from_utf8(&issued.token).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let link = Zeroizing::new(format!("{}/auth/{}#token={token}", origin, issued.purpose));
    let code = qrcode::QrCode::with_error_correction_level(link.as_bytes(), qrcode::EcLevel::M)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let qr = Zeroizing::new(
        code.render::<qrcode::render::svg::Color>()
            .quiet_zone(true)
            .min_dimensions(256, 256)
            .build(),
    );
    let expires = crate::presentation::datetime(issued.expires);
    Ok(Html(
        Issuance {
            nav: super::nav::Nav::here(super::nav::Section::Accounts),
            account: &issued.account,
            token,
            link: link.as_str(),
            qr: qr.as_str(),
            purpose: issued.purpose,
            expires: &expires,
        }
        .render()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    )
    .into_response())
}
#[derive(Template)]
#[template(path = "account_redeem.html")]
struct Redemption<'a> {
    csrf: &'a str,
    purpose: &'a str,
    token: &'a str,
    message: &'a str,
}
async fn redemption_form(state: AuthState, session: Session, purpose: &'static str) -> AuthResult {
    if session
        .get::<super::Principal>("principal")
        .await
        .map_err(super::session_error)?
        .is_some()
        && super::principal(&state, &session).await?.is_none()
    {
        return Ok(Redirect::to(&format!("/auth/{purpose}")).into_response());
    }
    let csrf = form_token(&state, &session).await?;
    Ok(Html(
        Redemption {
            csrf: &csrf,
            purpose,
            token: "",
            message: "",
        }
        .render()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    )
    .into_response())
}
async fn enroll_form(State(state): State<AuthState>, session: Session) -> AuthResult {
    redemption_form(state, session, "enroll").await
}
async fn reset_form(State(state): State<AuthState>, session: Session) -> AuthResult {
    redemption_form(state, session, "reset").await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Redeem {
    csrf: String,
    token: String,
    password: String,
    confirmation: String,
}
async fn redeem(
    state: AuthState,
    session: Session,
    form: Redeem,
    purpose: &'static str,
) -> AuthResult {
    let quota = throttle(&state);
    let token = Zeroizing::new(form.token.into_bytes());
    let password = Zeroizing::new(form.password.into_bytes());
    let confirmation = Zeroizing::new(form.confirmation.into_bytes());
    let digest = digest_bytes(&token).map_err(|err| error(&err))?;
    let token_text = std::str::from_utf8(&token).map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    let admission = match csrf_admission(&session, &form.csrf).await {
        Ok(admission) => admission,
        Err(StatusCode::FORBIDDEN) => {
            return retry_form(
                "",
                purpose,
                token_text,
                StatusCode::FORBIDDEN,
                "This form session expired or was revoked. Open a fresh form below to continue.",
            );
        }
        Err(StatusCode::SERVICE_UNAVAILABLE) => {
            return retry_form(
                "",
                purpose,
                token_text,
                StatusCode::SERVICE_UNAVAILABLE,
                "The server is busy. Wait a moment, then open a fresh form below.",
            );
        }
        Err(status) => return Err(status),
    };
    if let Err(status) = quota {
        return retry_form(
            &form.csrf,
            purpose,
            token_text,
            status,
            "Please wait six seconds and try again. Your link remains in this form.",
        );
    }
    if *password != *confirmation {
        return retry_form(
            &form.csrf,
            purpose,
            token_text,
            StatusCode::UNPROCESSABLE_ENTITY,
            "The passwords do not match. Enter and confirm your password again.",
        );
    }
    let result = state
        .accounts
        .redeem_bytes(purpose, digest, password, Some(admission))
        .await
        .map_err(|err| error(&err));
    if let Err(status) = result {
        let message = match status {
            StatusCode::FORBIDDEN => {
                "This form session expired or was revoked. Open a fresh form below to continue."
            }
            StatusCode::SERVICE_UNAVAILABLE => {
                "The server is busy. Please wait a moment and try again."
            }
            StatusCode::CONFLICT | StatusCode::UNPROCESSABLE_ENTITY => {
                "Check the password requirements below and try again. If the link expired or was replaced, request a new one."
            }
            _ => return Err(status),
        };
        return retry_form(&form.csrf, purpose, token_text, status, message);
    }
    Ok(Redirect::to("/auth/login").into_response())
}
async fn enroll(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<Redeem>,
) -> AuthResult {
    redeem(state, session, form, "enroll").await
}
async fn reset(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<Redeem>,
) -> AuthResult {
    redeem(state, session, form, "reset").await
}
