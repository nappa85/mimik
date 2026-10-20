//! rig-core 0.44.0 probe: C1 schema pass-through, C2 client policy, C3 body bound, C4 cancellation.
use std::net::SocketAddr;

use bytes::Bytes;
use futures_util::StreamExt;
use rig_core::completion::CompletionRequest;
use rig_core::http_client::{self, HttpClientExt, LazyBody, MultipartForm, StreamingResponse};
use rig_core::providers::llamacpp::extension::LlamaCppOptions;
use rig_core::providers::openai::wire::{LLAMACPP, OpenAIConfig};
use rig_core::providers::openai::OpenAI;
use serde_json::{Value, json};
use stub::*;

/// A transport that enforces a body bound on plain and streaming responses.
#[derive(Clone, Debug)]
struct Bounded(reqwest::Client);

#[derive(Debug)]
struct Over;
impl std::fmt::Display for Over {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "response body over the bound")
    }
}
impl std::error::Error for Over {}

fn to_reqwest<T: Into<Bytes>>(c: &reqwest::Client, req: http::Request<T>) -> reqwest::RequestBuilder {
    let (parts, body) = req.into_parts();
    c.request(parts.method, parts.uri.to_string()).headers(parts.headers).body(body.into())
}

impl HttpClientExt for Bounded {
    fn send<T, U>(
        &self,
        req: http::Request<T>,
    ) -> impl Future<Output = http_client::Result<http::Response<LazyBody<U>>>> + Send + 'static
    where
        T: Into<Bytes> + Send,
        U: From<Bytes> + Send + 'static,
    {
        let builder = to_reqwest(&self.0, req);
        async move {
            let response = builder.send().await.map_err(http_client::Error::instance)?;
            let status = response.status();
            let headers = response.headers().clone();
            let body = bounded_body(response, BODY_LIMIT).await;
            if !status.is_success() {
                let text = body.map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
                return Err(http_client::Error::non_success_with_details(status, headers, text));
            }
            let body = body.map_err(|_| http_client::Error::instance(Over))?;
            let lazy: LazyBody<U> = Box::pin(async move { Ok(U::from(body)) });
            let mut out = http::Response::builder().status(status);
            *out.headers_mut().unwrap() = headers;
            out.body(lazy).map_err(http_client::Error::Protocol)
        }
    }

    fn send_multipart<U>(
        &self,
        _req: http::Request<MultipartForm>,
    ) -> impl Future<Output = http_client::Result<http::Response<LazyBody<U>>>> + Send + 'static
    where
        U: From<Bytes> + Send + 'static,
    {
        async { Err(http_client::Error::instance(Over)) }
    }

    fn send_streaming<T>(
        &self,
        req: http::Request<T>,
    ) -> impl Future<Output = http_client::Result<StreamingResponse>> + Send
    where
        T: Into<Bytes> + Send,
    {
        let builder = to_reqwest(&self.0, req);
        async move {
            let response = builder.send().await.map_err(http_client::Error::instance)?;
            let status = response.status();
            let headers = response.headers().clone();
            if !status.is_success() {
                return Err(http_client::Error::non_success_with_details(status, headers, String::new()));
            }
            let mut seen = 0usize;
            let stream = response.bytes_stream().map(move |chunk| {
                let chunk = chunk.map_err(http_client::Error::instance)?;
                seen += chunk.len();
                if seen > BODY_LIMIT {
                    return Err(http_client::Error::instance(Over));
                }
                Ok(chunk)
            });
            let boxed: http_client::BoxedStream = Box::pin(stream);
            let mut out = http::Response::builder().status(status);
            *out.headers_mut().unwrap() = headers;
            out.body(boxed).map_err(http_client::Error::Protocol)
        }
    }
}

fn config(addr: SocketAddr) -> OpenAIConfig {
    OpenAIConfig::with_key(&LLAMACPP, "dummy-not-a-secret").with_base_url(format!("http://{addr}/v1"))
}

fn native() -> CompletionRequest {
    let schema: rig_core::schemars::Schema = serde_json::from_value(schema()).unwrap();
    CompletionRequest::new("<data>{}</data>")
        .preamble("fixed prefix")
        .temperature(0.0)
        .max_tokens(128)
        .output_schema(schema)
        .provider_option(LlamaCppOptions::new().top_k(1).chat_template_kwarg("enable_thinking", json!(false)))
}

fn raw() -> CompletionRequest {
    CompletionRequest::new("<data>{}</data>")
        .preamble("fixed prefix")
        .temperature(0.0)
        .max_tokens(128)
        .additional_params(json!({
            "response_format": {"type": "json_schema",
                "json_schema": {"name": "mimik_interpretation_v1", "strict": true, "schema": schema()}},
            "top_k": 1, "seed": 0, "cache_prompt": true}))
}

fn client(addr: SocketAddr, mode: &str) -> OpenAI {
    match mode {
        "default" => config(addr).client(),
        "policy" => config(addr).connect(rig_reqwest::ReqwestClient::from(policy_client())),
        _ => config(addr).connect(Bounded(policy_client())),
    }
}

fn sent_schema(stub: &Stub) -> Value {
    stub.last().map_or(Value::Null, |r| r.body["response_format"]["json_schema"]["schema"].clone())
}

async fn call(c: &OpenAI, req: CompletionRequest) -> Result<String, String> {
    c.chat("probe").call(req).await.map(|r| r.text()).map_err(|e| e.to_string())
}

async fn stream_text(c: &OpenAI, req: CompletionRequest) -> Result<usize, String> {
    let mut s = c.chat("probe").stream(req).map_err(|e| e.to_string())?;
    let mut n = 0;
    while let Some(item) = s.next().await {
        item.map_err(|e| e.to_string().chars().take(160).collect::<String>())?;
        n += 1;
    }
    Ok(n)
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let (trap, trap_n) = start_trap().await;
    set_proxy_env(trap);
    let (addr, stub) = start_stub().await;
    let (trap2, trap2_n) = start_trap().await;
    let scenario = std::env::args().nth(1).unwrap_or_default();
    let mut out = serde_json::Map::new();
    if scenario == "live" {
        let raw_url = std::env::var("LIVE_URL").unwrap();
        let url = endpoint_allowed(&raw_url).expect("guard");
        let req: Value = serde_json::from_str(&std::fs::read_to_string("/out/live-request.json").unwrap()).unwrap();
        let c = OpenAIConfig::with_key(&LLAMACPP, "").with_base_url(url.as_str().trim_end_matches('/'))
            .connect(Bounded(policy_client()));
        let request = CompletionRequest::new(req["user"].as_str().unwrap())
            .preamble(req["system"].as_str().unwrap())
            .temperature(0.0)
            .max_tokens(384)
            .additional_params(json!({
                "response_format": {"type": "json_schema",
                    "json_schema": {"name": "mimik_interpretation_v1", "strict": true, "schema": schema()}},
                "top_k": 1, "seed": 0, "cache_prompt": true}));
        let started = std::time::Instant::now();
        let r = call(&c, request).await;
        println!("{}", json!({"crate": "rig-core", "ok": r.is_ok(), "seconds": started.elapsed().as_secs_f64(),
            "content": r.as_ref().ok(), "error": r.as_ref().err()}));
        return;
    }
    let p = client(addr, "policy");
    if scenario.is_empty() {
        let r = call(&p, native()).await;
        let sent = sent_schema(&stub);
        let last = stub.last();
        out.insert("c1_native".into(), json!({
            "ok_response": r.as_deref() == Ok(REPLY), "error": r.err(),
            "schema_equal": sent == schema(), "path": last.as_ref().map(|x| x.path.clone()),
            "top_k": last.as_ref().map(|x| x.body["top_k"].clone()),
            "chat_template_kwargs": last.as_ref().map(|x| x.body["chat_template_kwargs"].clone()),
            "body_keys": last.as_ref().map(|x| x.body.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>())),
        }));
        if sent != schema() {
            std::fs::write("/out/rig-native-schema.json", serde_json::to_vec_pretty(&sent).unwrap()).ok();
        }
        let r = call(&p, raw()).await;
        out.insert("c1_raw".into(), json!({"ok_response": r.as_deref() == Ok(REPLY), "error": r.err(),
            "schema_equal": sent_schema(&stub) == schema(),
            "authorization_header": stub.last().unwrap().headers.iter().any(|(k, _)| k == "authorization")}));
        stub.set(Mode::Sse);
        let r = stream_text(&p, raw()).await;
        out.insert("c1_stream".into(), json!({"items": r, "stream_flag": stub.last().unwrap().body["stream"]}));
        stub.set(Mode::Json);

        let before = trap_count(&trap_n);
        let d = call(&client(addr, "default"), raw()).await;
        let default_trap = trap_count(&trap_n) - before;
        let before = trap_count(&trap_n);
        let q = call(&p, raw()).await;
        let b = call(&client(addr, "bounded"), raw()).await;
        out.insert("c2_proxy".into(), json!({
            "default_client_trap_connections": default_trap, "default_client_ok": d.is_ok(),
            "default_error": d.err(),
            "policy_and_bounded_trap_connections": trap_count(&trap_n) - before,
            "policy_ok": q.is_ok(), "bounded_ok": b.is_ok()}));
        stub.set(Mode::Redirect(trap2.port()));
        let before2 = trap_count(&trap2_n);
        let q = call(&p, raw()).await;
        let b = call(&client(addr, "bounded"), raw()).await;
        let policy_trap2 = trap_count(&trap2_n) - before2;
        let before2 = trap_count(&trap2_n);
        let d = call(&client(addr, "default"), raw()).await;
        out.insert("c2_redirect".into(), json!({
            "policy_error": q.err(), "bounded_error": b.err(), "policy_trap2_connections": policy_trap2,
            "default_error": d.err(), "default_trap2_connections": trap_count(&trap2_n) - before2}));
        stub.set(Mode::Json);

        for (name, mode) in [("c4_plain_policy", "policy"), ("c4_plain_bounded", "bounded")] {
            let (h, closed) = start_hang(false).await;
            let c = client(h, mode);
            out.insert(name.into(), cancellation(closed, call(&c, raw())).await);
        }
        for (name, mode) in [("c4_stream_policy", "policy"), ("c4_stream_bounded", "bounded")] {
            let (h, closed) = start_hang(true).await;
            let c = client(h, mode);
            out.insert(name.into(), cancellation(closed, stream_text(&c, raw())).await);
        }
        out.insert("c2_retry_500_policy".into(), attempts_on_status(&stub, 500, || call(&p, raw())).await);
        out.insert("c2_retry_429_policy".into(), attempts_on_status(&stub, 429, || call(&p, raw())).await);
    } else {
        // C3: one scenario per process (peak RSS is monotonic); the stub is another process.
        let (mode, big) = scenario.split_once('-').unwrap();
        let c = client(c3_addr(&scenario), mode);
        let hwm0 = vm_hwm_kib();
        let r = if big == "stream" { stream_text(&c, raw()).await.map(|n| n.to_string()) } else { call(&c, raw()).await.map(|t| t.len().to_string()) };
        out.insert(format!("c3_{scenario}"), json!({"ok": r.is_ok(),
            "error": r.err().map(|e| e.chars().take(160).collect::<String>()),
            "body_bytes": BIG, "hwm_growth_kib": vm_hwm_kib().saturating_sub(hwm0)}));
    }
    println!("{}", serde_json::to_string_pretty(&Value::Object(out)).unwrap());
}
