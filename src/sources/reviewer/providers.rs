//! The per-provider request builders over rig-core's own provider clients and wires. Each
//! client is built only from an explicit key and base URL (never `from_env*`) and sends through
//! the shared transport: Mistral on the chat wire, `OpenAI` on its default Responses route,
//! Anthropic on its Messages API.
use super::{Provider, Reviewer, findings, prompt};
use base64::Engine;
use rig_core::completion::message::{DocumentSourceKind, Image, ImageMediaType, Text, UserContent};
use rig_core::completion::{CompletionRequest, CompletionResponse, Message};
use rig_core::error::ProviderError;
use rig_core::providers::anthropic::AnthropicConfig;
use rig_core::providers::openai::OpenAIConfig;
use rig_core::providers::openai::wire::MISTRAL;
use serde_json::json;

/// Anthropic's Messages API requires `max_tokens` on every request; the model is free text, so
/// rig's per-model default is not always known and the request sets one.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "set by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
const ANTHROPIC_MAX_TOKENS: u64 = 32_768;

/// The one page payload as a rig request: the fixed instruction as the preamble, the page's
/// units as JSON text and the page image as base64 PNG in one user message, and the findings
/// schema as the structured output. `OpenAI`'s Responses route always carries `store: false`.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
pub(super) fn page_request(
    reviewer: &Reviewer,
    image_png: &[u8],
    units_json: &str,
) -> CompletionRequest {
    let encoded = base64::engine::general_purpose::STANDARD.encode(image_png);
    let user = Message::User {
        content: vec![
            UserContent::Text(Text::new(units_json.to_owned())),
            UserContent::Image(Image {
                data: DocumentSourceKind::Base64(encoded),
                media_type: Some(ImageMediaType::PNG),
                detail: None,
                native: None,
            }),
        ],
    };
    let mut request = CompletionRequest::new(user)
        .preamble(prompt::INSTRUCTION.to_owned())
        .output_schema(findings::schema());
    match reviewer.provider {
        Provider::OpenAi => {
            request = request.additional_params(json!({ "store": false }));
        }
        Provider::Anthropic => {
            request = request.max_tokens(Some(ANTHROPIC_MAX_TOKENS));
        }
        Provider::Mistral => {}
    }
    request
}

/// Sends one request through the provider's own client and wire, over the shared transport.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
#[expect(
    clippy::result_large_err,
    reason = "the error type is rig's `ProviderError`"
)]
pub(super) async fn call(
    reviewer: &Reviewer,
    request: CompletionRequest,
) -> Result<CompletionResponse, ProviderError> {
    let base = reviewer.base_url.as_str().to_owned();
    let transport = reviewer.transport.clone();
    match reviewer.provider {
        Provider::Mistral => {
            OpenAIConfig::with_key(&MISTRAL, reviewer.key.clone())
                .with_base_url(base)
                .connect(transport)
                .chat(reviewer.model.clone())
                .call(request)
                .await
        }
        Provider::OpenAi => {
            OpenAIConfig::new(reviewer.key.clone())
                .with_base_url(base)
                .connect(transport)
                .responses(reviewer.model.clone())
                .call(request)
                .await
        }
        Provider::Anthropic => {
            AnthropicConfig::new(reviewer.key.clone())
                .with_base_url(base)
                .connect(transport)
                .completion(reviewer.model.clone())
                .call(request)
                .await
        }
    }
}
