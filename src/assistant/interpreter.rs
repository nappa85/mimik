//! The interpretation client (assisted sheet plan §1.4, "Shape" and "Crate"): an
//! [`Interpreter`] trait, the rig-core adapter over Mimik's bounded transport, and a stub
//! for tests. One request per call: nothing retries.
use super::contract::{FIXED_PREFIX, SCHEMA, SCHEMA_NAME};
use super::transport::Bounded;
use async_trait::async_trait;
use rig_core::completion::CompletionRequest;
use rig_core::http_client::HttpClientExt;
use rig_core::providers::openai::OpenAI;
use rig_core::providers::openai::wire::{LLAMACPP, OpenAIConfig};
use serde_json::{Value, json};

/// One interpretation request: the dynamic part (the data envelope) and the model's
/// `max_tokens`. The fixed prefix and the schema are the same for every request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Completion {
    pub dynamic: String,
    pub max_tokens: u32,
}

/// A completed, bounded 2xx exchange, whatever its content.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Completed {
    pub text: String,
    /// Prompt tokens the server read from its cache, when it reports them.
    pub cached_tokens: Option<u64>,
    pub prompt_tokens: Option<u64>,
}

/// A failed exchange: what the breaker counts (plan §1.4, "Circuit breaker"): transport
/// errors, HTTP 4xx and 5xx, a body over the bound, a stream that is not a well-formed
/// completion. Cancellation is not a failure: the future is dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    /// The server answered with this status.
    Status(u16),
    /// No well-formed exchange: connection, body bound, stream shape.
    Transport,
}

#[async_trait]
pub(crate) trait Interpreter: Send + Sync {
    /// The token count of `text` as the server's tokenizer counts it (`/tokenize`).
    async fn tokens(&self, text: &str) -> Result<usize, Failure>;
    /// One streamed completion with the fixed prefix, the schema and `completion`.
    async fn complete(&self, completion: &Completion) -> Result<Completed, Failure>;
}

/// The `rig-core` adapter: the `OpenAI` chat route with the llama.cpp dialect, over
/// [`Bounded`]. The token is rig's API key (`Authorization: Bearer`); the model name is
/// configuration, never model output.
pub(crate) struct RigInterpreter {
    client: OpenAI,
    transport: Bounded,
    token: String,
    model: String,
}

impl RigInterpreter {
    pub(crate) fn new(transport: Bounded, token: String, model: String) -> Self {
        let client = OpenAIConfig::with_key(&LLAMACPP, token.as_str())
            .with_base_url(format!("{}/v1", transport.base()))
            .connect(transport.clone());
        Self {
            client,
            transport,
            token,
            model,
        }
    }
}

/// The body members beside rig's own (plan §1.4): the schema as `response_format`, the
/// greedy sampler and `cache_prompt`. rig sets `stream: true` for a streamed call.
pub(crate) fn additional_params() -> Value {
    json!({
        "response_format": {
            "type": "json_schema",
            "json_schema": {"name": SCHEMA_NAME, "strict": true, "schema": &*SCHEMA},
        },
        "top_k": 1,
        "seed": 0,
        "cache_prompt": true,
    })
}

fn failure(error: &rig_core::error::ProviderError) -> Failure {
    use rig_core::error::ProviderError as E;
    match error {
        E::ProviderResponse(response) | E::InvalidAuthentication(response) => {
            response.status.map_or(Failure::Transport, |status| {
                Failure::Status(status.as_u16())
            })
        }
        _ => Failure::Transport,
    }
}

#[async_trait]
impl Interpreter for RigInterpreter {
    async fn tokens(&self, text: &str) -> Result<usize, Failure> {
        let body = serde_json::to_vec(&json!({"content": text, "add_special": false}))
            .map_err(|_| Failure::Transport)?;
        let request = http::Request::post(format!("{}/tokenize", self.transport.base()))
            .header(http::header::CONTENT_TYPE, "application/json")
            .header(
                http::header::AUTHORIZATION,
                format!("Bearer {}", self.token),
            )
            .body(bytes::Bytes::from(body))
            .map_err(|_| Failure::Transport)?;
        let response = self
            .transport
            .send::<_, bytes::Bytes>(request)
            .await
            .map_err(|error| match error {
                rig_core::http_client::Error::InvalidStatusCodeWithDetails { status, .. } => {
                    Failure::Status(status.as_u16())
                }
                _ => Failure::Transport,
            })?;
        let body = response.into_body().await.map_err(|_| Failure::Transport)?;
        let value: Value = serde_json::from_slice(&body).map_err(|_| Failure::Transport)?;
        value["tokens"]
            .as_array()
            .map(Vec::len)
            .ok_or(Failure::Transport)
    }

    async fn complete(&self, completion: &Completion) -> Result<Completed, Failure> {
        let request = CompletionRequest::new(completion.dynamic.as_str())
            .preamble(FIXED_PREFIX.as_str())
            .temperature(0.0)
            .max_tokens(u64::from(completion.max_tokens))
            .additional_params(additional_params());
        let stream = self
            .client
            .chat(self.model.as_str())
            .stream(request)
            .map_err(|error| failure(&error))?;
        let response = stream.finish().await.map_err(|error| failure(&error))?;
        Ok(Completed {
            text: response.text(),
            cached_tokens: response.usage.cached_input_tokens,
            prompt_tokens: response.usage.input_tokens,
        })
    }
}

/// A test double: scripted answers, a token count of one per 3 bytes unless set, and a
/// record of every request.
#[cfg(test)]
pub(crate) mod stub {
    use super::{Completed, Completion, Failure, Interpreter, async_trait};
    use std::collections::VecDeque;
    use std::sync::Mutex;

    #[derive(Default)]
    pub(crate) struct StubInterpreter {
        pub answers: Mutex<VecDeque<Result<String, Failure>>>,
        pub token_ratio: Mutex<Option<usize>>,
        pub requests: Mutex<Vec<Completion>>,
        pub tokenized: Mutex<Vec<String>>,
        /// When set, each completion waits for one permit before answering.
        pub gate: Mutex<Option<std::sync::Arc<tokio::sync::Semaphore>>>,
        /// When set, `/tokenize` fails with this.
        pub tokenize_fails: Mutex<Option<Failure>>,
    }

    impl StubInterpreter {
        pub(crate) fn answering(
            answers: impl IntoIterator<Item = Result<String, Failure>>,
        ) -> Self {
            Self {
                answers: Mutex::new(answers.into_iter().collect()),
                ..Self::default()
            }
        }
    }

    #[async_trait]
    impl Interpreter for StubInterpreter {
        async fn tokens(&self, text: &str) -> Result<usize, Failure> {
            self.tokenized.lock().unwrap().push(text.to_owned());
            if let Some(failure) = *self.tokenize_fails.lock().unwrap() {
                return Err(failure);
            }
            let ratio = self.token_ratio.lock().unwrap().unwrap_or(3);
            Ok(text.len().div_ceil(ratio.max(1)))
        }

        async fn complete(&self, completion: &Completion) -> Result<Completed, Failure> {
            self.requests.lock().unwrap().push(completion.clone());
            let gate = self.gate.lock().unwrap().clone();
            if let Some(gate) = gate {
                gate.acquire().await.unwrap().forget();
            }
            let answer = self
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Err(Failure::Transport));
            answer.map(|text| Completed {
                text,
                ..Completed::default()
            })
        }
    }
}
