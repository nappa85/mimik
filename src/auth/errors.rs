//! Trusted browser presentation; status/authentication decisions remain separate.
use super::{AuthResult, LoginTemplate};
use askama::Template;
use axum::{
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
};

#[derive(Clone)]
pub(super) struct LoginRetry;
#[derive(Clone)]
pub(super) struct ErrorDocument;

pub(super) fn plain(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST => "Invalid request.",
        StatusCode::UNAUTHORIZED => "Invalid credentials or session.",
        StatusCode::FORBIDDEN => "Access denied.",
        StatusCode::REQUEST_TIMEOUT => "Request timed out.",
        StatusCode::PAYLOAD_TOO_LARGE => "Request body is too large.",
        StatusCode::UNSUPPORTED_MEDIA_TYPE => "Use a URL-encoded form.",
        StatusCode::TOO_MANY_REQUESTS => "Too many authentication requests.",
        StatusCode::SERVICE_UNAVAILABLE => "Authentication worker is busy.",
        _ => "Request failed.",
    }
}
pub(super) fn trusted(response: &Response) -> bool {
    let status = response.status();
    (matches!(
        status,
        StatusCode::UNAUTHORIZED
            | StatusCode::FORBIDDEN
            | StatusCode::TOO_MANY_REQUESTS
            | StatusCode::SERVICE_UNAVAILABLE
    ) && response.extensions().get::<LoginRetry>().is_some())
        || ((status.is_client_error() || status.is_server_error())
            && response.extensions().get::<ErrorDocument>().is_some())
}
pub(super) fn login(status: StatusCode, login: &str, csrf: &str, message: &str) -> AuthResult {
    if !super::document_presentation() {
        return Err(status);
    }
    let html = LoginTemplate {
        csrf,
        login,
        message,
    }
    .render()
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut response = (status, Html(html)).into_response();
    response.extensions_mut().insert(LoginRetry);
    retry(&mut response);
    Ok(response)
}
fn retry(response: &mut Response) {
    let seconds = match response.status() {
        StatusCode::TOO_MANY_REQUESTS => Some("6"),
        StatusCode::SERVICE_UNAVAILABLE => Some("1"),
        _ => None,
    };
    if let Some(seconds) = seconds {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, seconds.parse().expect("static retry"));
    }
}
#[derive(Template)]
#[template(path = "error.html")]
struct ErrorTemplate<'a> {
    code: u16,
    title: &'a str,
    message: &'a str,
}
pub(super) fn document(mut response: Response) -> Response {
    let status = response.status();
    let (title, message) = match status {
        StatusCode::UNAUTHORIZED => (
            "Sign in required",
            "Your session is no longer valid. Open sign-in to continue.",
        ),
        StatusCode::FORBIDDEN => (
            "This request could not be accepted",
            "Your form may have expired, or this account does not have access. Open a fresh page and try again.",
        ),
        StatusCode::TOO_MANY_REQUESTS => (
            "Please wait before trying again",
            "Too many sign-in attempts. Wait six seconds, then open sign-in and try again.",
        ),
        StatusCode::SERVICE_UNAVAILABLE => (
            "The server is busy",
            "Wait a moment, then try again. Your password has not been included in this page.",
        ),
        StatusCode::REQUEST_TIMEOUT => (
            "The request took too long",
            "Open a fresh page and try again. If you were saving changes, check their current state first.",
        ),
        StatusCode::PAYLOAD_TOO_LARGE => (
            "The submitted form is too large",
            "Reduce the form size and try again.",
        ),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
        | StatusCode::BAD_REQUEST
        | StatusCode::UNPROCESSABLE_ENTITY => (
            "The form could not be read",
            "Open a fresh page and submit the form again.",
        ),
        _ => (
            "The request could not be completed",
            "The server could not complete this request. Please try again; if it continues, check the server status.",
        ),
    };
    let Ok(html) = ErrorTemplate {
        code: status.as_u16(),
        title,
        message,
    }
    .render() else {
        return response;
    };
    let retry_after = response.headers().get(header::RETRY_AFTER).cloned();
    response = (status, Html(html)).into_response();
    response.extensions_mut().insert(ErrorDocument);
    if let Some(value) = retry_after {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    retry(&mut response);
    response
}
