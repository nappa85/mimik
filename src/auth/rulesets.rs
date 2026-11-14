//! Owner-only ruleset registry: bounded listing, exact registration and legacy aliases.
//! Registration identifies materials; it never approves evidence or enables a sheet.
use super::{
    AuthResult, AuthState, RequestAuth, csrf_admission,
    materials::{TrustedDraft, error, html},
    owner_auth,
};
use crate::{
    AppError,
    registry::{RegisterAlias, RegisterRuleset, RulesetEntry, RulesetKey},
};
use askama::Template;
use axum::{
    Router,
    extract::{Form, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect},
    routing::{get, post},
};
use serde::Deserialize;
use tower_sessions::Session;

pub(super) fn routes() -> Router<AuthState> {
    Router::new()
        .route("/admin/rulesets", get(directory).post(register))
        .route("/admin/rulesets/aliases", post(register_alias))
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Notice {
    Registered,
    Unchanged,
    AliasRegistered,
    AliasUnchanged,
}
impl Notice {
    fn text(self) -> &'static str {
        match self {
            Self::Registered => "Registered the new game/edition identity.",
            Self::Unchanged => "This exact game/edition was already registered; nothing changed.",
            Self::AliasRegistered => "Registered the exact legacy alias.",
            Self::AliasUnchanged => "This exact legacy alias already existed; nothing changed.",
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Cursor {
    after_game: Option<String>,
    after_edition: Option<String>,
    notice: Option<Notice>,
}
impl Cursor {
    /// Both composite parts or neither; the shared listing validates each part.
    fn key(self) -> Result<Option<RulesetKey>, StatusCode> {
        match (self.after_game, self.after_edition) {
            (None, None) => Ok(None),
            (Some(game_id), Some(edition_id)) => Ok(Some(RulesetKey {
                game_id,
                edition_id,
            })),
            _ => Err(StatusCode::UNPROCESSABLE_ENTITY),
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RulesetForm {
    csrf: String,
    game_id: String,
    game_label: String,
    edition_id: String,
    edition_label: String,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct AliasForm {
    csrf: String,
    game_id: String,
    edition_id: String,
    legacy_edition: String,
}

#[derive(Template)]
#[template(path = "admin_rulesets.html")]
struct Directory {
    nav: super::nav::Nav,
    rows: Vec<RulesetEntry>,
    next: Option<RulesetKey>,
    csrf: String,
    notice: &'static str,
    message: String,
    ruleset: Option<RulesetForm>,
    alias: Option<AliasForm>,
}

async fn listing(
    state: &AuthState,
    auth: RequestAuth,
    after: Option<RulesetKey>,
    csrf: String,
) -> Result<Directory, StatusCode> {
    let page = state
        .store()
        .owner_rulesets(auth, after)
        .await
        .map_err(|e| error(&e))?;
    Ok(Directory {
        nav: super::nav::Nav::here(super::nav::Section::Rulesets),
        rows: page.rows,
        next: page.next,
        csrf,
        notice: "",
        message: String::new(),
        ruleset: None,
        alias: None,
    })
}

async fn directory(
    State(state): State<AuthState>,
    session: Session,
    Query(cursor): Query<Cursor>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let notice = cursor.notice.map_or("", Notice::text);
    let after = cursor.key()?;
    let csrf = super::token(&session).await?.ok_or(StatusCode::FORBIDDEN)?;
    let mut page = listing(&state, auth, after, csrf).await?;
    page.notice = notice;
    html(&page)
}

/// Rejected input is re-rendered escaped beside the first page, never silently refreshed.
async fn rejected(
    state: &AuthState,
    auth: RequestAuth,
    csrf: String,
    problem: &AppError,
    retain: impl FnOnce(&mut Directory),
) -> AuthResult {
    let mut page = listing(state, auth, None, csrf).await?;
    page.message = problem.to_string();
    retain(&mut page);
    let mut response = html(&page)?;
    *response.status_mut() = error(problem);
    response.extensions_mut().insert(TrustedDraft);
    Ok(response)
}

async fn register(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<RulesetForm>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let csrf = csrf_admission(&session, &form.csrf).await?;
    let result = state
        .store()
        .owner_register_ruleset(
            auth.clone(),
            csrf,
            RegisterRuleset {
                game_id: form.game_id.clone(),
                game_label: form.game_label.clone(),
                edition_id: form.edition_id.clone(),
                edition_label: form.edition_label.clone(),
            },
        )
        .await;
    match result {
        Ok(created) => Ok(Redirect::to(if created {
            "/admin/rulesets?notice=registered"
        } else {
            "/admin/rulesets?notice=unchanged"
        })
        .into_response()),
        Err(problem @ AppError::Invalid(_)) => {
            rejected(&state, auth, form.csrf.clone(), &problem, |page| {
                page.ruleset = Some(form);
            })
            .await
        }
        Err(problem) => Err(error(&problem)),
    }
}

async fn register_alias(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<AliasForm>,
) -> AuthResult {
    let auth = owner_auth(&state, &session).await?;
    let csrf = csrf_admission(&session, &form.csrf).await?;
    let result = state
        .store()
        .owner_register_alias(
            auth.clone(),
            csrf,
            RegisterAlias {
                game_id: form.game_id.clone(),
                edition_id: form.edition_id.clone(),
                legacy_edition: form.legacy_edition.clone(),
            },
        )
        .await;
    match result {
        Ok(created) => Ok(Redirect::to(if created {
            "/admin/rulesets?notice=alias-registered"
        } else {
            "/admin/rulesets?notice=alias-unchanged"
        })
        .into_response()),
        Err(problem @ AppError::Invalid(_)) => {
            rejected(&state, auth, form.csrf.clone(), &problem, |page| {
                page.alias = Some(form);
            })
            .await
        }
        Err(problem) => Err(error(&problem)),
    }
}
