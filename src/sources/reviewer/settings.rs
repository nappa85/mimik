//! The reviewer's settings row (docs/api-reviewer-plan.md step 3, migration 041): provider,
//! base URL and model name, stored in the database (the API key is not: it is a 0600 file in
//! the data folder's `secrets/` directory, `key.rs`). The base URL is validated with
//! [`https::validate_base_url`] plus the per-provider path rule of what rig 0.47.1 appends;
//! the model name is free text, non-empty, at most [`MODEL_MAX_BYTES`] bytes and free of control
//! characters (the `Report::origin` rule).
use super::Provider;
use crate::AppError;
use crate::assistant::download::https;
use crate::auth::{CsrfAdmission, RequestAuth};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use url::Url;

/// The prefix a model name gets as a report's origin, so the model name is at most 100 bytes
/// minus this prefix.
pub(crate) const MODEL_PREFIX: &str = "reviewer:";
/// The most bytes the whole origin (prefix plus model name) may have.
pub(crate) const ORIGIN_MAX_BYTES: usize = 100;
/// The most bytes the model name may have.
pub(crate) const MODEL_MAX_BYTES: usize = ORIGIN_MAX_BYTES - MODEL_PREFIX.len();
/// The most bytes the base URL may have.
pub(crate) const BASE_URL_MAX_BYTES: usize = 2048;

impl Provider {
    /// The provider's name as stored in the settings row.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Mistral => "mistral",
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
        }
    }

    /// Parses a stored or submitted provider name.
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw {
            "mistral" => Some(Self::Mistral),
            "openai" => Some(Self::OpenAi),
            "anthropic" => Some(Self::Anthropic),
            _ => None,
        }
    }

    /// The provider's name on the page.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Mistral => "Mistral",
            Self::OpenAi => "OpenAI",
            Self::Anthropic => "Anthropic",
        }
    }

    /// The provider's public base URL, the settings form's default.
    pub(crate) fn default_base_url(self) -> &'static str {
        match self {
            Self::Mistral => "https://api.mistral.ai",
            Self::OpenAi => "https://api.openai.com/v1",
            Self::Anthropic => "https://api.anthropic.com",
        }
    }

    /// The path the base URL must have: rig appends the route to the base URL, so Mistral's
    /// and Anthropic's base is the bare host and `OpenAI`'s includes `/v1` (part 1's fixtures:
    /// `POST {base}/v1/chat/completions`, `POST {base}/v1/responses` on `{base}=…/v1`,
    /// `POST {base}/v1/messages`).
    pub(crate) fn base_path(self) -> &'static str {
        match self {
            Self::OpenAi => "/v1",
            Self::Mistral | Self::Anthropic => "/",
        }
    }

    /// The page's help text for the base URL field: the per-provider path rule.
    pub(crate) fn base_help(self) -> &'static str {
        match self {
            Self::Mistral => {
                "The bare host, for example https://api.mistral.ai: the client appends \
                 /v1/chat/completions."
            }
            Self::OpenAi => {
                "Including /v1, for example https://api.openai.com/v1: the client appends /responses."
            }
            Self::Anthropic => {
                "The bare host, for example https://api.anthropic.com: the client appends \
                 /v1/messages."
            }
        }
    }
}

/// One validated settings input, as the form submitted it.
#[derive(Clone, Debug)]
pub(crate) struct ValidSettings {
    pub(crate) provider: Provider,
    pub(crate) base_url: Url,
    pub(crate) model: String,
}

/// Validates the form's provider, base URL and model. The messages are fixed; the submitted
/// values are kept in the re-rendered form (never the key).
///
/// # Errors
/// The provider is unknown, the base URL is refused, or the model name is empty or too long.
pub(crate) fn validate(
    provider: &str,
    base_url: &str,
    model: &str,
) -> Result<ValidSettings, String> {
    let Some(provider) = Provider::parse(provider) else {
        return Err("That is not a provider this page offers.".to_owned());
    };
    let base_url = base_url.trim();
    if base_url.len() > BASE_URL_MAX_BYTES {
        return Err(format!(
            "The base URL must be at most {BASE_URL_MAX_BYTES} bytes."
        ));
    }
    let url = https::validate_base_url(base_url)
        .map_err(|reason| format!("The base URL is refused: {reason}."))?;
    if url.path() != provider.base_path() {
        return Err(format!(
            "For {} the base URL is {}.",
            provider.label(),
            if provider.base_path() == "/" {
                "the bare host, without a path"
            } else {
                "the host with the /v1 path"
            }
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("The base URL must not carry a query or a fragment.".to_owned());
    }
    let model = model.trim();
    if model.is_empty() {
        return Err("The model name must not be empty.".to_owned());
    }
    if model.len() > MODEL_MAX_BYTES {
        return Err(format!(
            "The model name must be at most {MODEL_MAX_BYTES} bytes: 100 bytes for the report's \
             origin minus the \"{MODEL_PREFIX}\" prefix."
        ));
    }
    // The origin rule of `Report::origin` (src/sources/reports.rs): no control character
    // anywhere in the model name (a NUL, a tab, a DEL in the middle included).
    if model.chars().any(char::is_control) {
        return Err("The model name must not contain a control character.".to_owned());
    }
    Ok(ValidSettings {
        provider,
        base_url: url,
        model: model.to_owned(),
    })
}

/// The stored settings, or the defaults when no row exists yet.
#[derive(Clone, Debug)]
pub(crate) struct Settings {
    pub(crate) provider: Provider,
    pub(crate) base_url: String,
    pub(crate) model: String,
    /// The row's change time, Unix seconds; 0 when there is no row yet.
    pub(crate) changed_at: i64,
}

impl Settings {
    /// Loads the settings row; the defaults when there is none.
    ///
    /// # Errors
    /// The row is malformed, or a database failure.
    pub(crate) fn load(db: &Connection) -> Result<Self, AppError> {
        let found = db
            .query_row(
                "SELECT provider, base_url, model, changed_at FROM reviewer_settings WHERE id=1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()?;
        Ok(match found {
            Some((provider, base_url, model, changed_at)) => Self {
                provider: Provider::parse(&provider).ok_or(AppError::Invalid(
                    "The stored reviewer provider is unknown.",
                ))?,
                base_url,
                model,
                changed_at,
            },
            None => Self {
                provider: Provider::Mistral,
                base_url: Provider::Mistral.default_base_url().to_owned(),
                model: String::new(),
                changed_at: 0,
            },
        })
    }

    /// Writes the single settings row, with who changed it and when (the authentication clock).
    ///
    /// # Errors
    /// A database failure.
    pub(crate) fn save(
        db: &Connection,
        by: i64,
        settings: &ValidSettings,
    ) -> Result<i64, AppError> {
        let now: i64 = db.query_row("SELECT mimik_auth_now()", [], |row| row.get(0))?;
        db.execute(
            "INSERT INTO reviewer_settings(id,provider,base_url,model,changed_by,changed_at)
             VALUES(1,?1,?2,?3,?4,?5)
             ON CONFLICT(id) DO UPDATE SET provider=?1,base_url=?2,model=?3,changed_by=?4,changed_at=?5",
            params![
                settings.provider.as_str(),
                settings.base_url.as_str(),
                settings.model,
                by,
                now
            ],
        )?;
        Ok(now)
    }
}

impl crate::Store {
    /// Loads the reviewer settings for the page.
    ///
    /// # Errors
    /// A database failure.
    pub(crate) async fn reviewer_settings(&self) -> Result<Settings, AppError> {
        self.run_bounded_auth(|db| Settings::load(db)).await
    }

    /// Saves the reviewer settings, rechecking the owner and CSRF admissions before and after
    /// the write, and returns the change time.
    ///
    /// # Errors
    /// The admissions no longer hold, or a database failure: nothing is stored.
    pub(crate) async fn owner_save_reviewer_settings(
        &self,
        auth: RequestAuth,
        csrf: CsrfAdmission,
        settings: ValidSettings,
    ) -> Result<i64, AppError> {
        self.run_bounded_auth(move |db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            auth.recheck_owner(&tx)?;
            csrf.recheck(&tx)?;
            let now = Settings::save(&tx, auth.account_id(), &settings)?;
            auth.recheck_owner(&tx)?;
            csrf.recheck(&tx)?;
            tx.commit()?;
            Ok(now)
        })
        .await
    }
}
