//! What the runtime asks the model server besides interpretations (assisted sheet plan §1.4a,
//! "Priming and calibration"): `/health` (the only request without the token), `/tokenize`,
//! and plain completions with the fixed prefix (priming, calibration samples), all over the
//! same checked transport as the jobs.
use crate::assistant::contract::FIXED_PREFIX;
use crate::assistant::interpreter::{Failure, Interpreter, RigInterpreter};
use crate::assistant::transport::Bounded;
use async_trait::async_trait;
use rig_core::http_client::{self, HttpClientExt};
use serde_json::{Value, json};
use std::sync::Arc;

/// `/health`'s answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Health {
    /// 200.
    Ready,
    /// 503: still loading the model.
    Loading,
    /// Nothing listens yet (connection refused, no socket, no listening entry).
    NotListening,
    /// The endpoint check refused the connection: the attempt fails at once.
    Refused(String),
    /// Any other answer.
    Failed,
}

/// The server's `timings` of one completion; absent members are `None`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Timings {
    pub prompt_n: Option<f64>,
    pub prompt_ms: Option<f64>,
    pub cache_n: Option<f64>,
    pub predicted_n: Option<f64>,
    pub predicted_ms: Option<f64>,
}

impl Timings {
    pub(crate) fn from_body(body: &Value) -> Self {
        let timings = &body["timings"];
        let number = |name: &str| timings.get(name).and_then(Value::as_f64);
        Self {
            prompt_n: number("prompt_n"),
            prompt_ms: number("prompt_ms"),
            cache_n: number("cache_n"),
            predicted_n: number("predicted_n"),
            predicted_ms: number("predicted_ms"),
        }
    }
}

/// A schema-constrained completion, as the model trial reads it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Structured {
    pub text: String,
    /// The reply stopped at `max_tokens` (`finish_reason` is `length`), or did not say why it
    /// stopped.
    pub truncated: bool,
    pub predicted_n: Option<f64>,
}

#[async_trait]
pub(crate) trait Server: Send + Sync {
    async fn health(&self) -> Health;
    /// The token count of `text` without special tokens.
    async fn tokens(&self, text: &str) -> Result<usize, Failure>;
    /// One non-streamed completion: the fixed prefix as the system message, `user` as the
    /// user message, greedy, `max_tokens`, `ignore_eos` when asked, no schema.
    async fn plain(
        &self,
        user: &str,
        max_tokens: u32,
        ignore_eos: bool,
    ) -> Result<Timings, Failure>;
    /// One non-streamed interpretation request as a job sends it (the fixed prefix, the
    /// pinned schema as `response_format`, greedy), for the model trial's phrases.
    async fn structured(
        &self,
        user: &str,
        ref_ids: &[String],
        max_tokens: u32,
    ) -> Result<Structured, Failure>;
    /// The interpretation client over the same transport.
    fn interpreter(&self) -> Arc<dyn Interpreter>;
}

/// The real server over a checked transport.
pub(crate) struct Llama {
    transport: Bounded,
    token: String,
    model: String,
    interpreter: Arc<RigInterpreter>,
}

impl Llama {
    pub(crate) fn new(transport: Bounded, token: &str, model: &str) -> Self {
        let interpreter = Arc::new(RigInterpreter::new(
            transport.clone(),
            token.to_owned(),
            model.to_owned(),
        ));
        Self {
            transport,
            token: token.to_owned(),
            model: model.to_owned(),
            interpreter,
        }
    }

    async fn post(&self, path: &str, body: &Value) -> Result<Value, Failure> {
        let bytes = serde_json::to_vec(body).map_err(|_| Failure::Transport)?;
        let request = http::Request::post(format!("{}{path}", self.transport.base()))
            .header(http::header::CONTENT_TYPE, "application/json")
            .header(
                http::header::AUTHORIZATION,
                format!("Bearer {}", self.token),
            )
            .body(bytes::Bytes::from(bytes))
            .map_err(|_| Failure::Transport)?;
        let response = self
            .transport
            .send::<_, bytes::Bytes>(request)
            .await
            .map_err(|error| failure(&error))?;
        let body = response.into_body().await.map_err(|_| Failure::Transport)?;
        serde_json::from_slice(&body).map_err(|_| Failure::Transport)
    }
}

/// The body of a trial phrase: the job's request, non-streamed.
fn structured_body(model: &str, user: &str, ref_ids: &[String], max_tokens: u32) -> Value {
    let mut body = json!({
        "model": model,
        "messages": [
            {"role": "system", "content": [{"type": "text", "text": FIXED_PREFIX.as_str()}]},
            {"role": "user", "content": [{"type": "text", "text": user}]},
        ],
        "max_tokens": max_tokens,
        "temperature": 0.0,
        "stream": false,
    });
    if let (Some(object), Value::Object(extra)) = (
        body.as_object_mut(),
        crate::assistant::interpreter::additional_params(
            &crate::assistant::interpreter::RequestKind::Interpretation,
            ref_ids,
        ),
    ) {
        object.extend(extra);
    }
    body
}

/// The reply's text, whether it was cut off, and its generated tokens.
fn read_structured(value: &Value) -> Result<Structured, Failure> {
    let choice = &value["choices"][0];
    Ok(Structured {
        text: choice["message"]["content"]
            .as_str()
            .ok_or(Failure::Transport)?
            .to_owned(),
        truncated: choice["finish_reason"].as_str() != Some("stop"),
        predicted_n: Timings::from_body(value).predicted_n,
    })
}

fn failure(error: &http_client::Error) -> Failure {
    match error {
        http_client::Error::InvalidStatusCodeWithDetails { status, .. } => {
            Failure::Status(status.as_u16())
        }
        _ => Failure::Transport,
    }
}

/// The I/O error under a transport error, if there is one.
fn io_kind(error: &http_client::Error) -> Option<(std::io::ErrorKind, String)> {
    match error {
        http_client::Error::Instance(inner) => {
            let mut source: Option<&(dyn std::error::Error + 'static)> = Some(inner.as_ref());
            while let Some(current) = source {
                if let Some(io) = current.downcast_ref::<std::io::Error>() {
                    return Some((io.kind(), io.to_string()));
                }
                source = current.source();
            }
            None
        }
        _ => None,
    }
}

#[async_trait]
impl Server for Llama {
    async fn health(&self) -> Health {
        let Ok(request) = http::Request::get(format!("{}/health", self.transport.base()))
            .body(bytes::Bytes::new())
        else {
            return Health::Failed;
        };
        match self.transport.send::<_, bytes::Bytes>(request).await {
            Ok(_) => Health::Ready,
            Err(http_client::Error::InvalidStatusCodeWithDetails { status, .. })
                if status.as_u16() == 503 =>
            {
                Health::Loading
            }
            Err(error) => match io_kind(&error) {
                Some((std::io::ErrorKind::PermissionDenied, why)) => Health::Refused(why),
                Some((
                    std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::ConnectionReset,
                    _,
                )) => Health::NotListening,
                _ => Health::Failed,
            },
        }
    }

    async fn tokens(&self, text: &str) -> Result<usize, Failure> {
        let value = self
            .post("/tokenize", &json!({"content": text, "add_special": false}))
            .await?;
        value["tokens"]
            .as_array()
            .map(Vec::len)
            .ok_or(Failure::Transport)
    }

    async fn plain(
        &self,
        user: &str,
        max_tokens: u32,
        ignore_eos: bool,
    ) -> Result<Timings, Failure> {
        let mut body = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": [{"type": "text", "text": FIXED_PREFIX.as_str()}]},
                {"role": "user", "content": [{"type": "text", "text": user}]},
            ],
            "max_tokens": max_tokens,
            "temperature": 0.0,
            "top_k": 1,
            "seed": 0,
            "cache_prompt": true,
            "stream": false,
        });
        if ignore_eos {
            body["ignore_eos"] = json!(true);
        }
        let value = self.post("/v1/chat/completions", &body).await?;
        Ok(Timings::from_body(&value))
    }

    async fn structured(
        &self,
        user: &str,
        ref_ids: &[String],
        max_tokens: u32,
    ) -> Result<Structured, Failure> {
        let body = structured_body(&self.model, user, ref_ids, max_tokens);
        let value = self.post("/v1/chat/completions", &body).await?;
        read_structured(&value)
    }

    fn interpreter(&self) -> Arc<dyn Interpreter> {
        Arc::clone(&self.interpreter) as Arc<dyn Interpreter>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trial_phrase_is_a_job_request_without_streaming() {
        let ids = vec!["max_hp".to_owned(), "silver".to_owned()];
        let body = structured_body("granite", "<data>x</data>", &ids, 384);
        assert_eq!(body["model"], "granite");
        assert_eq!(body["max_tokens"], 384);
        assert_eq!(body["stream"], false);
        assert_eq!(body["top_k"], 1);
        assert_eq!(body["seed"], 0);
        assert_eq!(body["temperature"], 0.0);
        assert_eq!(
            body["messages"][0]["content"][0]["text"],
            FIXED_PREFIX.as_str()
        );
        assert_eq!(body["messages"][1]["content"][0]["text"], "<data>x</data>");
        // The schema of the phrase's IDs, as every job sends its own.
        let format = &body["response_format"]["json_schema"];
        assert_eq!(format["strict"], true);
        assert_eq!(
            format["schema"],
            crate::assistant::contract::schema_for(&ids)
        );
        assert_eq!(
            format["schema"]["$defs"]["ref"]["properties"]["ref"],
            serde_json::json!({"enum": ["t", "max_hp", "silver"]})
        );
    }

    #[test]
    fn a_reply_is_truncated_unless_the_server_says_it_stopped() {
        let reply = |finish: Value| {
            json!({
                "choices": [{"message": {"content": "{}"}, "finish_reason": finish}],
                "timings": {"predicted_n": 12.0},
            })
        };
        let stopped = read_structured(&reply(json!("stop"))).unwrap();
        assert_eq!(stopped.text, "{}");
        assert!(!stopped.truncated);
        assert_eq!(stopped.predicted_n, Some(12.0));
        for cut in [json!("length"), json!(null), json!("tool_calls")] {
            assert!(read_structured(&reply(cut)).unwrap().truncated);
        }
        assert_eq!(read_structured(&json!({})), Err(Failure::Transport));
    }
}
