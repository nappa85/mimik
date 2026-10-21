//! async-openai 0.42.2 probe (comparison; not allowed by D-H): C1 to C4.
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};

use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_openai::error::OpenAIError;
use async_openai::middleware::{HttpRequestFactory, ReqwestService};
use async_openai::types::chat::{
    ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs,
    CreateChatCompletionRequestArgs, CreateChatCompletionResponse, ResponseFormat,
    ResponseFormatJsonSchema,
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use stub::*;

/// A tower service that bounds plain bodies and streams.
#[derive(Clone)]
struct Bounded(reqwest::Client);

impl tower::Service<HttpRequestFactory> for Bounded {
    type Response = reqwest::Response;
    type Error = OpenAIError;
    type Future = Pin<Box<dyn Future<Output = Result<reqwest::Response, OpenAIError>> + Send>>;
    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), OpenAIError>> {
        Poll::Ready(Ok(()))
    }
    fn call(&mut self, request: HttpRequestFactory) -> Self::Future {
        let client = self.0.clone();
        Box::pin(async move {
            let request = request.build().await?;
            let response = client.execute(request).await?;
            let status = response.status();
            let headers = response.headers().clone();
            let sse = headers
                .get("content-type")
                .is_some_and(|v| v.to_str().unwrap_or("").starts_with("text/event-stream"));
            let body = if sse {
                let mut seen = 0usize;
                let stream = response.bytes_stream().map(move |chunk| {
                    let chunk = chunk.map_err(std::io::Error::other)?;
                    seen += chunk.len();
                    if seen > BODY_LIMIT {
                        return Err(std::io::Error::other("response body over the bound"));
                    }
                    Ok(chunk)
                });
                reqwest::Body::wrap_stream(stream)
            } else {
                let bytes = bounded_body(response, BODY_LIMIT)
                    .await
                    .map_err(OpenAIError::InvalidArgument)?;
                reqwest::Body::from(bytes)
            };
            let mut out = http::Response::builder().status(status);
            *out.headers_mut().unwrap() = headers;
            Ok(reqwest::Response::from(out.body(body).unwrap()))
        })
    }
}

fn config(addr: SocketAddr) -> OpenAIConfig {
    OpenAIConfig::new().with_api_base(format!("http://{addr}/v1")).with_api_key("dummy-not-a-secret")
}

fn client(addr: SocketAddr, mode: &str) -> Client<OpenAIConfig> {
    match mode {
        "default" => Client::with_config(config(addr)),
        "policy" => Client::with_config(config(addr)).with_http_client(policy_client()),
        "service" => Client::with_config(config(addr)).with_http_service(ReqwestService::new(policy_client())),
        _ => Client::with_config(config(addr)).with_http_service(Bounded(policy_client())),
    }
}

fn native_request() -> async_openai::types::chat::CreateChatCompletionRequest {
    CreateChatCompletionRequestArgs::default()
        .model("probe")
        .messages([
            ChatCompletionRequestSystemMessageArgs::default().content("fixed prefix").build().unwrap().into(),
            ChatCompletionRequestUserMessageArgs::default().content("<data>{}</data>").build().unwrap().into(),
        ])
        .temperature(0.0)
        .max_completion_tokens(128u32)
        .response_format(ResponseFormat::JsonSchema {
            json_schema: ResponseFormatJsonSchema {
                description: None,
                name: "mimik_interpretation_v1".into(),
                schema: schema(),
                strict: Some(true),
            },
        })
        .build()
        .unwrap()
}

fn raw(stream: bool) -> Value {
    json!({"model": "probe", "stream": stream, "temperature": 0, "max_tokens": 128, "top_k": 1,
        "seed": 0, "cache_prompt": true, "chat_template_kwargs": {"enable_thinking": false},
        "messages": [{"role": "system", "content": "fixed prefix"}, {"role": "user", "content": "<data>{}</data>"}],
        "response_format": {"type": "json_schema", "json_schema": {"name": "mimik_interpretation_v1", "strict": true, "schema": schema()}}})
}

async fn call_raw(c: &Client<OpenAIConfig>) -> Result<String, String> {
    let r: Result<Value, _> = c.chat().create_byot(raw(false)).await;
    r.map(|v| v["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string())
        .map_err(|e| e.to_string().chars().take(200).collect())
}

async fn stream_raw(c: &Client<OpenAIConfig>) -> Result<usize, String> {
    let mut s = c
        .chat()
        .create_stream_byot::<Value, Value>(raw(true))
        .await
        .map_err(|e| e.to_string().chars().take(200).collect::<String>())?;
    let mut n = 0;
    while let Some(item) = s.next().await {
        item.map_err(|e| e.to_string().chars().take(200).collect::<String>())?;
        n += 1;
    }
    Ok(n)
}

fn sent_schema(stub: &Stub) -> Value {
    stub.last().map_or(Value::Null, |r| r.body["response_format"]["json_schema"]["schema"].clone())
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let (trap, trap_n) = start_trap().await;
    set_proxy_env(trap);
    let (addr, stub) = start_stub().await;
    let (trap2, trap2_n) = start_trap().await;
    let scenario = std::env::args().nth(1).unwrap_or_default();
    let mut out = serde_json::Map::new();
    let b = client(addr, "bounded");
    if scenario.is_empty() {
        let r: Result<CreateChatCompletionResponse, _> = b.chat().create(native_request()).await;
        out.insert("c1_native".into(), json!({
            "ok_response": r.as_ref().map(|r| r.choices[0].message.content.as_deref() == Some(REPLY)).unwrap_or(false),
            "error": r.err().map(|e| e.to_string()), "schema_equal": sent_schema(&stub) == schema(),
            "body_keys": stub.last().map(|x| x.body.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()))}));
        let r = call_raw(&b).await;
        let last = stub.last().unwrap();
        out.insert("c1_raw".into(), json!({"ok_response": r.as_deref() == Ok(REPLY), "error": r.err(),
            "schema_equal": sent_schema(&stub) == schema(), "top_k": last.body["top_k"],
            "chat_template_kwargs": last.body["chat_template_kwargs"]}));
        stub.set(Mode::Sse);
        out.insert("c1_stream".into(), json!({"items": stream_raw(&b).await}));
        stub.set(Mode::Json);

        let mut proxy = serde_json::Map::new();
        for mode in ["default", "policy", "service", "bounded"] {
            let before = trap_count(&trap_n);
            let r = call_raw(&client(addr, mode)).await;
            proxy.insert(mode.into(), json!({"trap_connections": trap_count(&trap_n) - before, "ok": r.is_ok(), "error": r.err()}));
        }
        out.insert("c2_proxy".into(), Value::Object(proxy));
        stub.set(Mode::Redirect(trap2.port()));
        let mut redirect = serde_json::Map::new();
        for mode in ["default", "policy", "bounded"] {
            let before = trap_count(&trap2_n);
            let r = call_raw(&client(addr, mode)).await;
            redirect.insert(mode.into(), json!({"trap2_connections": trap_count(&trap2_n) - before, "error": r.err()}));
        }
        out.insert("c2_redirect".into(), Value::Object(redirect));
        stub.set(Mode::Json);
        for mode in ["policy", "bounded"] {
            let c = client(addr, mode);
            out.insert(format!("c2_retry_500_{mode}"), attempts_on_status(&stub, 500, || call_raw(&c)).await);
            out.insert(format!("c2_retry_429_{mode}"), attempts_on_status(&stub, 429, || call_raw(&c)).await);
        }
        for mode in ["policy", "bounded"] {
            let (h, closed) = start_hang(false).await;
            let c = client(h, mode);
            out.insert(format!("c4_plain_{mode}"), cancellation(closed, call_raw(&c)).await);
            let (h, closed) = start_hang(true).await;
            let c = client(h, mode);
            out.insert(format!("c4_stream_{mode}"), cancellation(closed, stream_raw(&c)).await);
        }
    } else {
        let (mode, big) = scenario.split_once('-').unwrap();
        let c = client(c3_addr(&scenario), mode);
        let hwm0 = vm_hwm_kib();
        let r = if big == "stream" { stream_raw(&c).await.map(|n| n.to_string()) } else { call_raw(&c).await.map(|t| t.len().to_string()) };
        out.insert(format!("c3_{scenario}"), json!({"ok": r.is_ok(), "error": r.err(),
            "body_bytes": BIG, "hwm_growth_kib": vm_hwm_kib().saturating_sub(hwm0)}));
    }
    println!("{}", serde_json::to_string_pretty(&Value::Object(out)).unwrap());
}
