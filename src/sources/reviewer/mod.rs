//! The in-app API reviewer (docs/api-reviewer-plan.md, slice (a) steps 1-3): one page is sent
//! to the owner-chosen provider through rig-core's own provider wires, over Mimik's bounded
//! HTTPS transport (`crate::assistant::download::https`), and the reply text is parsed as the
//! typed findings value. The page content is untrusted data, never instructions: only the
//! fixed [`prompt::INSTRUCTION`] is. Provider clients are built only from an explicit key and
//! base URL, never `from_env*`, and the key never reaches an error, a log line or a body this
//! module writes.
//!
//! This slice covers steps 1-3: the protocol check, the transport, and the settings and key
//! page. The send path (`Reviewer`, `build`) is wired into the routes by step 4; the items only
//! step 4 calls carry an `expect(dead_code)` for the non-test build.
mod findings;
pub(crate) mod key;
#[cfg(test)]
#[path = "key_tests.rs"]
mod key_tests;
#[cfg(test)]
#[path = "log_tests.rs"]
mod log_tests;
mod prompt;
mod providers;
#[cfg(test)]
#[path = "sandbox_tests.rs"]
mod sandbox_tests;
pub(crate) mod settings;
#[cfg(test)]
#[path = "settings_tests.rs"]
mod settings_tests;
#[cfg(test)]
#[path = "tests.rs"]
mod tests;
#[cfg(test)]
#[path = "transport_tests.rs"]
mod transport_tests;

use crate::assistant::download::https::{self, Https};
pub use findings::Findings;
use rig_core::error::ProviderError;
use url::Url;

/// The providers this slice enables (the plan's list; each costs its protocol check).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    /// Mistral, on the chat wire.
    Mistral,
    /// `OpenAI`, on its default Responses route.
    OpenAi,
    /// Anthropic, on its Messages API.
    Anthropic,
}

/// One reviewer: the provider, base URL, key and model, over the shared transport. The base URL
/// is validated at build time and again before every send (in the transport).
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
pub struct Reviewer {
    provider: Provider,
    base_url: Url,
    key: String,
    model: String,
    transport: Https,
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
impl Reviewer {
    /// Builds the reviewer from an explicit base URL, key and model.
    ///
    /// # Errors
    ///
    /// The base URL is refused (not HTTPS, not a domain name, or carrying credentials).
    pub fn new(
        provider: Provider,
        base_url: &str,
        key: &str,
        model: &str,
        transport: Https,
    ) -> Result<Self, ReviewError> {
        let base_url = https::validate_base_url(base_url).map_err(ReviewError::Base)?;
        Ok(Self {
            provider,
            base_url,
            key: key.to_owned(),
            model: model.to_owned(),
            transport,
        })
    }

    /// Reviews one page: `image_png` is the page's review PNG, `units_json` its units as JSON
    /// text. The whole call, any retry included, runs under the per-page budget
    /// [`https::PAGE_BUDGET`].
    ///
    /// # Errors
    ///
    /// A refused base URL, a fixed status message, a transport failure, an expired budget, or
    /// a reply that is not the findings JSON (a refusal included).
    pub async fn review_page(
        &self,
        image_png: &[u8],
        units_json: &str,
    ) -> Result<Findings, ReviewError> {
        self.review_page_within(https::PAGE_BUDGET, image_png, units_json)
            .await
    }

    /// [`Self::review_page`] under an explicit total budget; the job applies
    /// [`https::PAGE_BUDGET`], tests a shorter one.
    ///
    /// # Errors
    ///
    /// As [`Self::review_page`], or the budget expiring.
    pub async fn review_page_within(
        &self,
        budget: std::time::Duration,
        image_png: &[u8],
        units_json: &str,
    ) -> Result<Findings, ReviewError> {
        let request = providers::page_request(self, image_png, units_json);
        let call = providers::call(self, request);
        let response = tokio::time::timeout(budget, call)
            .await
            .map_err(|_| ReviewError::Budget)?
            .map_err(|error| ReviewError::from_provider(&error))?;
        // The answer text: `text` chunks joined, `thinking` dropped; rig folds a `refusal`
        // into the text, so it fails the parse below like any non-JSON reply.
        findings::parse(&response.text())
    }
}

/// Builds the reviewer from the stored settings and the key file: the shipped transport
/// ([`https::Transport::reviewer`]: the platform trust store, public addresses only, no proxy,
/// no redirects) and a provider client from the explicit key and the stored base URL, which is
/// validated again here. Step 4's preview and send build it this way.
///
/// `Ok(None)` when no key is set or no model name is stored yet: there is no reviewer then.
///
/// # Errors
/// The stored base URL is refused (a fixed reason), the stored key file does not itself
/// satisfy the save rule (a fixed message, never the key), or the transport cannot be set up.
/// The key is read into memory for the build and dropped with the reviewer; it is never
/// stored, shown or logged.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
pub async fn build(store: &crate::Store) -> Result<Option<Reviewer>, crate::AppError> {
    use crate::AppError;
    let settings = store.reviewer_settings().await?;
    let Some(key) = store.secrets().read()? else {
        return Ok(None);
    };
    // One key rule for readiness and sending: the stored bytes themselves must satisfy the
    // save rule, with no trimming at read time. The refusal is a fixed message; the key is
    // never in it.
    if !key::valid_stored_key(&key) {
        return Err(AppError::Invalid(key::INVALID_STORED_KEY));
    }
    if settings.model.is_empty() {
        return Ok(None);
    }
    let key_text = std::str::from_utf8(&key)
        .map_err(|_| AppError::Invalid("The reviewer key file is not valid UTF-8."))?;
    let transport = https::Transport::reviewer();
    let client = https::Https::new(&transport).map_err(|error| match error {
        https::Error::NoTrustStore => AppError::Invalid(
            "No trusted root certificates were found on this system: install the system CA \
             certificates (for example the ca-certificates package).",
        ),
        _ => AppError::Invalid("The reviewer's HTTPS transport cannot be set up."),
    })?;
    match Reviewer::new(
        settings.provider,
        &settings.base_url,
        key_text,
        &settings.model,
        client,
    ) {
        Ok(reviewer) => Ok(Some(reviewer)),
        Err(ReviewError::Base(reason)) => Err(AppError::Invalid(reason)),
        Err(_) => Err(AppError::Invalid(
            "The stored reviewer settings are refused.",
        )),
    }
}

/// Why one page's review failed. The messages are fixed: no key, no `Authorization` header, no
/// URL, no provider body ever reaches them.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
pub enum ReviewError {
    Base(&'static str),
    Status(u16),
    NoTrustStore,
    Redirect,
    Address,
    Tls,
    Timeout,
    Connect,
    TooLarge,
    Transfer,
    Budget,
    InvalidReply,
    Build,
}

impl std::fmt::Display for ReviewError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Base(reason) => write!(formatter, "the provider base URL is refused: {reason}"),
            Self::Status(status) => formatter.write_str(&https::status_message(*status)),
            Self::NoTrustStore => formatter.write_str(
                "no trusted root certificates were found on this system: install the system CA \
                 certificates (for example the ca-certificates package)",
            ),
            Self::Redirect => {
                formatter.write_str("the provider answered with a redirect, which is refused")
            }
            Self::Address => formatter
                .write_str("the provider's address is not allowed (local, private or reserved)"),
            Self::Tls => formatter.write_str("the provider's certificate is not trusted"),
            Self::Timeout => formatter.write_str("the connection timed out"),
            Self::Connect => formatter.write_str("the connection failed"),
            Self::TooLarge => formatter.write_str("the reply is longer than the 4 MiB bound"),
            Self::Transfer => formatter.write_str("the transfer failed"),
            Self::Budget => formatter.write_str("the page exceeded its 300 s budget"),
            Self::InvalidReply => formatter.write_str(
                "the model's reply was not valid findings JSON (a refusal lands here too)",
            ),
            Self::Build => formatter.write_str("the review request could not be built"),
        }
    }
}

impl std::error::Error for ReviewError {}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
impl ReviewError {
    /// Maps a rig error to the fixed message: the preserved reply's status decides; the
    /// transport's own error is found in the chain; anything else is a fixed failure.
    fn from_provider(error: &ProviderError) -> Self {
        if let Some(status) = status_of(error) {
            return Self::Status(status);
        }
        match &error {
            ProviderError::Http(inner) => {
                let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&**inner);
                while let Some(link) = source {
                    if let Some(ours) = link.downcast_ref::<https::Error>() {
                        return Self::from_transport(ours);
                    }
                    source = link.source();
                }
                Self::Transfer
            }
            ProviderError::Json(_) | ProviderError::Response(_) => Self::InvalidReply,
            ProviderError::Request(_) | ProviderError::UnsupportedOption(_) => Self::Build,
            _ => Self::Transfer,
        }
    }

    fn from_transport(error: &https::Error) -> Self {
        match error {
            https::Error::Base(reason) => Self::Base(reason),
            https::Error::Status(status) => Self::Status(*status),
            https::Error::NoTrustStore => Self::NoTrustStore,
            https::Error::Redirect => Self::Redirect,
            https::Error::Address => Self::Address,
            https::Error::Tls => Self::Tls,
            https::Error::Timeout => Self::Timeout,
            https::Error::Connect => Self::Connect,
            https::Error::TooLarge => Self::TooLarge,
            https::Error::Transfer | https::Error::Setup(_) => Self::Transfer,
        }
    }
}

/// The HTTP status a rig error carries, when it has one.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
fn status_of(error: &ProviderError) -> Option<u16> {
    match error {
        ProviderError::ProviderResponse(reply) | ProviderError::InvalidAuthentication(reply) => {
            reply.status.map(|status| status.as_u16())
        }
        ProviderError::Http(inner) => inner.non_success_status().map(|status| status.as_u16()),
        _ => None,
    }
}
