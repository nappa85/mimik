//! `/admin/reviewer` (docs/api-reviewer-plan.md step 3): the owner's settings of the in-app
//! API reviewer — provider, base URL and model name, one database row — and the API key file
//! in the data folder's `secrets/` directory. Owner only; every form is CSRF-checked and the
//! router's body bound applies. The key is never shown back: the page reports only "set" or
//! "not set" and the change time, and no response, error or log line carries it.
use super::materials::{TrustedDraft, error, html};
use super::nav::{Nav, Section};
use super::{AuthResult, AuthState, csrf_admission, owner_auth};
use crate::sources::reviewer::key::KeyPageStatus;
use crate::sources::reviewer::{Provider, key, settings};
use askama::Template;
use axum::{
    Router,
    extract::{Form, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;
use tower_sessions::Session;
use url::Url;

pub(super) fn routes() -> Router<AuthState> {
    Router::new()
        .route("/admin/reviewer", get(page))
        .route("/admin/reviewer/settings", post(save_settings))
        .route("/admin/reviewer/key", post(save_key))
        .route("/admin/reviewer/key/delete", post(delete_key))
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Notice {
    SettingsSaved,
    KeySet,
    KeyReplaced,
    KeyDeleted,
    KeyNotSet,
}

impl Notice {
    fn text(self) -> &'static str {
        match self {
            Self::SettingsSaved => "Saved the reviewer settings.",
            Self::KeySet => {
                "Set the API key. It is stored only in the secrets folder and is never shown again."
            }
            Self::KeyReplaced => "Replaced the API key.",
            Self::KeyDeleted => "Deleted the API key.",
            Self::KeyNotSet => "No API key is set; nothing was deleted.",
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Query_ {
    notice: Option<Notice>,
}

/// The form bodies are a CSRF token plus at most a 2 KiB base URL, a 91-byte model and a
/// 512-byte key: the router's 8 KiB body bound is far more than they need.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsForm {
    csrf: String,
    provider: String,
    base_url: String,
    model: String,
}

/// A CSRF token plus at most a 512-byte key: the router's 8 KiB body bound is far more than the
/// form needs.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyForm {
    csrf: String,
    key: String,
}

/// A CSRF token alone: the router's 8 KiB body bound is far more than the form needs.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteForm {
    csrf: String,
}

/// One provider of the settings form: its radio button and the path rule its base URL must
/// follow (what rig 0.47.1 appends to the base).
struct ProviderLine {
    value: &'static str,
    label: &'static str,
    help: &'static str,
    checked: bool,
}

/// The fixed page message when the key file's entry is not a private 0600 regular file owned
/// by this user (a wrong mode, a link or another user's file).
const KEY_NOT_PRIVATE_MESSAGE: &str = "The key file is not private: replace or delete it";
/// The fixed page message when the key file is private but cannot be used (a read failure, for
/// example a file over [`key::KEY_MAX_BYTES`]).
const KEY_UNUSABLE_MESSAGE: &str = "The key file cannot be used: replace or delete it";

#[derive(Template)]
#[template(path = "admin_reviewer.html")]
struct Page {
    nav: Nav,
    csrf: String,
    notice: &'static str,
    providers: Vec<ProviderLine>,
    base_url: String,
    model: String,
    /// The settings row's change time, when a row exists.
    settings_changed: Option<String>,
    key_set: bool,
    /// The key file's change time, when it is set.
    key_changed: Option<String>,
    /// The fixed message about the key file when it is neither simply set nor absent: not a
    /// private 0600 regular file owned by this user, or private but unusable (a read failure, or
    /// bytes that break the save rule).
    /// The page still renders, says so, stays not ready, and keeps the replace and delete
    /// forms.
    key_message: Option<&'static str>,
    /// Why the last form was refused (never the key).
    error: Option<String>,
    /// Whether a reviewer can be built from the stored settings and key right now, by the cheap
    /// check (a key is set and valid, a model is set, the stored base URL validates); step 4
    /// sends through `sources::reviewer::build`.
    ready: bool,
    /// The key's byte bound, for the form and the help text.
    key_max_bytes: usize,
}

/// A Unix timestamp as the page shows it.
fn timestamp(seconds: i64) -> String {
    match time::OffsetDateTime::from_unix_timestamp(seconds) {
        Ok(at) => format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
            at.year(),
            at.month() as u8,
            at.day(),
            at.hour(),
            at.minute(),
            at.second()
        ),
        Err(_) => seconds.to_string(),
    }
}

fn redirect(notice: &str) -> Response {
    Redirect::to(&format!("/admin/reviewer?notice={notice}")).into_response()
}

fn status_of(failure: &crate::AppError) -> StatusCode {
    error(failure)
}

/// The page. `echo` holds a refused settings form's submitted values (kept in the fields, the
/// base URL blanked when it carries a `@`, credentials, or does not parse at all); the key is
/// never echoed.
async fn render(
    state: &AuthState,
    session: &Session,
    notice: &'static str,
    echo: Option<(String, String, String)>,
    error: Option<String>,
) -> AuthResult {
    let csrf = super::token(session).await?.ok_or(StatusCode::FORBIDDEN)?;
    let settings = state
        .store()
        .reviewer_settings()
        .await
        .map_err(|e| status_of(&e))?;
    // The key file as the page sees it: a not-private entry (a wrong mode, a link or another
    // user's file) does not fail the page, it is reported with the replace and delete forms
    // kept.
    let (key_set, key_changed, mut key_message) = match state
        .store()
        .secrets()
        .page_status()
        .map_err(|e| status_of(&e))?
    {
        KeyPageStatus::NotSet => (false, None, None),
        KeyPageStatus::Set(status) => (true, Some(timestamp(status.changed_at)), None),
        KeyPageStatus::NotPrivate => (false, None, Some(KEY_NOT_PRIVATE_MESSAGE)),
    };
    // The readiness check is cheap: no TLS client is built and the key is not copied into a
    // String, only read (zeroized) to check the save rule. A read failure (an over-long or
    // otherwise unusable key file) does not fail the page either: it is reported with a fixed
    // message, the page stays not ready, and both forms stay. `build` stays for step 4's send.
    let mut key_ready = false;
    if key_set {
        match state.store().secrets().ready() {
            Ok(true) => key_ready = true,
            // Empty, or bytes that break the save rule (a hand-written `key\n`, say).
            Ok(false) | Err(_) => key_message = Some(KEY_UNUSABLE_MESSAGE),
        }
    }
    let ready = key_ready
        && settings::validate(
            settings.provider.as_str(),
            &settings.base_url,
            &settings.model,
        )
        .is_ok();
    let (provider, base_url, model) = match echo {
        Some((provider, base_url, model)) => (provider, base_url, model),
        None => (
            settings.provider.as_str().to_owned(),
            settings.base_url.clone(),
            settings.model.clone(),
        ),
    };
    let providers = [Provider::Mistral, Provider::OpenAi, Provider::Anthropic]
        .into_iter()
        .map(|choice| ProviderLine {
            value: choice.as_str(),
            label: choice.label(),
            help: choice.base_help(),
            checked: choice.as_str() == provider,
        })
        .collect();
    html(&Page {
        nav: Nav::here(Section::Reviewer),
        csrf,
        notice,
        providers,
        base_url,
        model,
        settings_changed: (settings.changed_at > 0).then(|| timestamp(settings.changed_at)),
        key_set,
        key_changed,
        key_message,
        error,
        ready,
        key_max_bytes: key::KEY_MAX_BYTES,
    })
}

/// A refused form: the page again, with the submitted settings kept in their fields (a base URL
/// that carries a `@`, a user name or password, or does not parse at all is re-rendered empty,
/// never echoed), the message beside them and the key never echoed.
async fn refused(
    state: &AuthState,
    session: &Session,
    form: &SettingsForm,
    message: String,
) -> AuthResult {
    let base_url = match Url::parse(&form.base_url) {
        Ok(url) if !url.username().is_empty() || url.password().is_some() => String::new(),
        Ok(_) if form.base_url.contains('@') => String::new(),
        Err(_) => String::new(),
        Ok(_) => form.base_url.clone(),
    };
    let mut response = render(
        state,
        session,
        "",
        Some((form.provider.clone(), base_url, form.model.clone())),
        Some(message),
    )
    .await?;
    *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
    response.extensions_mut().insert(TrustedDraft);
    Ok(response)
}

async fn page(
    State(state): State<AuthState>,
    session: Session,
    Query(query): Query<Query_>,
) -> AuthResult {
    owner_auth(&state, &session).await?;
    render(
        &state,
        &session,
        query.notice.map_or("", Notice::text),
        None,
        None,
    )
    .await
}

async fn save_settings(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<SettingsForm>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let csrf = csrf_admission(&session, &form.csrf).await?;
    let valid = match settings::validate(&form.provider, &form.base_url, &form.model) {
        Ok(valid) => valid,
        Err(message) => return refused(&state, &session, &form, message).await,
    };
    match state
        .store()
        .owner_save_reviewer_settings(auth, csrf, valid)
        .await
    {
        Ok(_) => Ok(redirect("settings-saved")),
        Err(error) => Err(status_of(&error)),
    }
}

/// Sets or replaces the key file. The submitted key is trimmed and checked against the save
/// rule here; it is never echoed, logged or stored anywhere but the 0600 file. Whether the
/// notice says "set" or "replaced" depends only on whether an entry exists (stat-ed without
/// following a link): the rename replaces the file anyway, even one that is not private.
async fn save_key(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<KeyForm>,
) -> AuthResult {
    let _auth = owner_auth(&state, &session).await?;
    let _csrf = csrf_admission(&session, &form.csrf).await?;
    let key = form.key.trim();
    if !key::valid_key(key.as_bytes()) {
        return render(
            &state,
            &session,
            "",
            None,
            Some(format!("{}; nothing was saved.", key::KEY_RULE)),
        )
        .await
        .map(|mut response| {
            *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
            response.extensions_mut().insert(TrustedDraft);
            response
        });
    }
    let secrets = state.store().secrets().clone();
    let existed = secrets.entry_exists().map_err(|e| status_of(&e))?;
    secrets.write(key.as_bytes()).map_err(|e| status_of(&e))?;
    Ok(redirect(if existed { "key-replaced" } else { "key-set" }))
}

async fn delete_key(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<DeleteForm>,
) -> AuthResult {
    let _auth = owner_auth(&state, &session).await?;
    let _csrf = csrf_admission(&session, &form.csrf).await?;
    let deleted = state
        .store()
        .secrets()
        .delete()
        .map_err(|e| status_of(&e))?;
    Ok(redirect(if deleted {
        "key-deleted"
    } else {
        "key-not-set"
    }))
}
