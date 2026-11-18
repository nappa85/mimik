//! The interpretation client (assisted sheet plan §1.4, "Shape" and "Crate"): an
//! [`Interpreter`] trait, the rig-core adapter over Mimik's bounded transport, and a stub
//! for tests. One request per call: nothing retries.
use super::contract::{
    FIXED_PREFIX, REMINDER_PREFIX, REMINDER_SCHEMA_NAME, SCHEMA_NAME, reminder_schema_for,
    schema_for,
};
use super::transport::Bounded;
use async_trait::async_trait;
use rig_core::completion::CompletionRequest;
use rig_core::http_client::HttpClientExt;
use rig_core::providers::openai::OpenAI;
use rig_core::providers::openai::wire::{LLAMACPP, OpenAIConfig};
use serde_json::{Value, json};

/// What a request is for: it picks the system message and the reply schema (plan 3c-1).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum RequestKind {
    /// A scribble's reading: the fixed prefix and the interpretation schema.
    #[default]
    Interpretation,
    /// A reminder after a decrease: the reminder system message and schema, with the row IDs
    /// and the change targets of this request (the expression references are `ref_ids`).
    Reminder {
        rows: Vec<String>,
        targets: Vec<String>,
    },
}

/// One model request: the dynamic part (the data envelope), the IDs of its context items
/// (the reply schema's `ref` enum), the model's `max_tokens` and the kind. The system
/// message is the same for every request of a kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Completion {
    pub dynamic: String,
    pub ref_ids: Vec<String>,
    pub max_tokens: u32,
    pub kind: RequestKind,
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

/// The body members beside rig's own (plan §1.4): the schema of a request of `kind` that supplies
/// `ref_ids` as `response_format`, the greedy sampler and `cache_prompt`. rig sets
/// `stream: true` for a streamed call.
pub(crate) fn additional_params(kind: &RequestKind, ref_ids: &[String]) -> Value {
    let (name, schema) = match kind {
        RequestKind::Interpretation => (SCHEMA_NAME, schema_for(ref_ids)),
        RequestKind::Reminder { rows, targets } => (
            REMINDER_SCHEMA_NAME,
            reminder_schema_for(rows, targets, ref_ids),
        ),
    };
    json!({
        "response_format": {
            "type": "json_schema",
            "json_schema": {"name": name, "strict": true, "schema": schema},
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
            .preamble(match completion.kind {
                RequestKind::Interpretation => FIXED_PREFIX.as_str(),
                RequestKind::Reminder { .. } => REMINDER_PREFIX.as_str(),
            })
            .temperature(0.0)
            .max_tokens(u64::from(completion.max_tokens))
            .additional_params(additional_params(&completion.kind, &completion.ref_ids));
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
        /// The prompt tokens each completion reports as read from the cache.
        pub cached_tokens: Mutex<Option<u64>>,
        /// How long each completion takes (tokio time), in order; later ones take none.
        pub delays: Mutex<VecDeque<std::time::Duration>>,
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
            let delay = self.delays.lock().unwrap().pop_front();
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            let answer = self
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Err(Failure::Transport));
            let cached_tokens = *self.cached_tokens.lock().unwrap();
            answer.map(|text| Completed {
                text,
                cached_tokens,
                ..Completed::default()
            })
        }
    }
}
