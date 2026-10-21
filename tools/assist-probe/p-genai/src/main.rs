//! genai 0.7.0 probe: C1 schema pass-through, C2 client policy, C3 body bound, C4 cancellation.
use std::net::SocketAddr;

use futures_util::StreamExt;
use genai::adapter::AdapterKind;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, ChatStreamEvent, JsonSpec};
use genai::resolver::{AuthData, Endpoint};
use genai::{Client, ModelIden, ServiceTarget};
use serde_json::{Value, json};
use stub::*;

fn target(addr: SocketAddr) -> ServiceTarget {
    ServiceTarget {
        endpoint: Endpoint::from_owned(format!("http://{addr}/v1/")),
        auth: AuthData::from_single("dummy-not-a-secret"),
        model: ModelIden::new(AdapterKind::OpenAI, "probe"),
    }
}

fn request() -> ChatRequest {
    ChatRequest::new(vec![ChatMessage::system("fixed prefix"), ChatMessage::user("<data>{}</data>")])
}

fn native_options() -> ChatOptions {
    ChatOptions::default()
        .with_response_format(JsonSpec::new("mimik_interpretation_v1", schema()))
        .with_temperature(0.0)
        .with_max_tokens(128)
        .with_seed(0)
}

fn raw_options() -> ChatOptions {
    ChatOptions::default().with_temperature(0.0).with_max_tokens(128).with_extra_body(json!({
        "response_format": {"type": "json_schema",
            "json_schema": {"name": "mimik_interpretation_v1", "strict": true, "schema": schema()}},
        "top_k": 1, "seed": 0, "cache_prompt": true,
        "chat_template_kwargs": {"enable_thinking": false}
    }))
}

fn client(policy: bool) -> Client {
    let builder = Client::builder();
    let builder = if policy { builder.with_reqwest(policy_client()) } else { builder };
    builder.build().unwrap()
}

fn c_live() -> Client {
    Client::builder().with_reqwest(policy_client()).build().unwrap()
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
    let mut out = serde_json::Map::new();
    let scenario = std::env::args().nth(1).unwrap_or_default();
    if scenario == "live" {
        // One live request through the forwarder: the endpoint passes the §1.4 guard first.
        let raw_url = std::env::var("LIVE_URL").unwrap();
        let url = endpoint_allowed(&raw_url).expect("guard");
        let req: Value = serde_json::from_str(&std::fs::read_to_string("/out/live-request.json").unwrap()).unwrap();
        let live = ServiceTarget {
            endpoint: Endpoint::from_owned(url.to_string()),
            auth: AuthData::from_single(""),
            model: ModelIden::new(AdapterKind::OpenAI, "screen"),
        };
        let chat = ChatRequest::new(vec![
            ChatMessage::system(req["system"].as_str().unwrap()),
            ChatMessage::user(req["user"].as_str().unwrap()),
        ]);
        let options = ChatOptions::default().with_temperature(0.0).with_max_tokens(384).with_extra_body(json!({
            "response_format": {"type": "json_schema",
                "json_schema": {"name": "mimik_interpretation_v1", "strict": true, "schema": schema()}},
            "top_k": 1, "seed": 0, "cache_prompt": true}));
        let started = std::time::Instant::now();
        let r = c_live().exec_chat(live, chat, Some(&options)).await;
        println!("{}", json!({"crate": "genai", "ok": r.is_ok(), "seconds": started.elapsed().as_secs_f64(),
            "content": r.as_ref().ok().and_then(|r| r.first_text().map(str::to_string)),
            "error": r.err().map(|e| e.to_string())}));
        return;
    }
    let c = client(true);
    if scenario.is_empty() {
    // C1 native structured output
    let r = c.exec_chat(target(addr), request(), Some(&native_options())).await;
    let sent = sent_schema(&stub);
    out.insert("c1_native".into(), json!({
        "ok_response": r.as_ref().map(|r| r.first_text() == Some(REPLY)).unwrap_or(false),
        "error": r.as_ref().err().map(ToString::to_string),
        "schema_equal": sent == schema(),
        "path": stub.last().map(|x| x.path),
        "body_keys": stub.last().map(|x| x.body.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>())),
    }));
    if sent != schema() {
        std::fs::write("/out/genai-native-schema.json", serde_json::to_vec_pretty(&sent).unwrap()).ok();
    }
    // C1 raw extra body
    let r = c.exec_chat(target(addr), request(), Some(&raw_options())).await;
    let last = stub.last().unwrap();
    out.insert("c1_raw".into(), json!({
        "ok_response": r.as_ref().map(|r| r.first_text() == Some(REPLY)).unwrap_or(false),
        "schema_equal": sent_schema(&stub) == schema(),
        "top_k": last.body["top_k"], "seed": last.body["seed"],
        "chat_template_kwargs": last.body["chat_template_kwargs"],
        "max_tokens": last.body.get("max_tokens").or(last.body.get("max_completion_tokens")),
        "authorization_header": last.headers.iter().any(|(k, _)| k == "authorization"),
        "user_agent": last.headers.iter().find(|(k, _)| k == "user-agent").map(|(_, v)| v.clone()),
    }));
    // C1 streaming with the raw body
    stub.set(Mode::Sse);
    let mut text = String::new();
    match c.exec_chat_stream(target(addr), request(), Some(&raw_options())).await {
        Ok(mut s) => {
            while let Some(ev) = s.stream.next().await {
                if let Ok(ChatStreamEvent::Chunk(ch)) = ev {
                    text.push_str(&ch.content);
                }
            }
        }
        Err(e) => text = format!("error {e}"),
    }
    out.insert("c1_stream".into(), json!({"ok": text == REPLY, "stream_flag": stub.last().unwrap().body["stream"]}));
    stub.set(Mode::Json);

    // C2 proxies: default client vs policy client
    let before = trap_count(&trap_n);
    let d = client(false).exec_chat(target(addr), request(), Some(&raw_options())).await;
    let default_trap = trap_count(&trap_n) - before;
    let before = trap_count(&trap_n);
    let p = client(true).exec_chat(target(addr), request(), Some(&raw_options())).await;
    out.insert("c2_proxy".into(), json!({
        "default_client_trap_connections": default_trap,
        "default_client_ok": d.is_ok(),
        "policy_client_trap_connections": trap_count(&trap_n) - before,
        "policy_client_ok": p.is_ok(),
    }));
    // C2 redirect
    stub.set(Mode::Redirect(trap2.port()));
    let before2 = trap_count(&trap2_n);
    let p = client(true).exec_chat(target(addr), request(), Some(&raw_options())).await;
    let policy_trap2 = trap_count(&trap2_n) - before2;
    let before2 = trap_count(&trap2_n);
    let d = client(false).exec_chat(target(addr), request(), Some(&raw_options())).await;
    out.insert("c2_redirect".into(), json!({
        "policy_error": p.as_ref().err().map(ToString::to_string),
        "policy_trap2_connections": policy_trap2,
        "default_error": d.as_ref().err().map(ToString::to_string),
        "default_trap2_connections": trap_count(&trap2_n) - before2,
    }));

    // C4 cancellation, plain and streaming
    let (h, closed) = start_hang(false).await;
    let raw = raw_options();
    let fut = c.exec_chat(target(h), request(), Some(&raw));
    out.insert("c4_plain".into(), cancellation(closed, fut).await);
    let (h, closed) = start_hang(true).await;
    let fut = async {
        if let Ok(mut s) = c.exec_chat_stream(target(h), request(), Some(&raw_options())).await {
            while s.stream.next().await.is_some() {}
        }
    };
    out.insert("c4_stream".into(), cancellation(closed, fut).await);

    let ro = raw_options();
    stub.set(Mode::Gzip);
    let gp = c.exec_chat(target(addr), request(), Some(&ro)).await;
    let gp_ae = stub.last().unwrap().headers.iter().find(|(k, _)| k == "accept-encoding").map(|(_, v)| v.clone());
    let gd = client(false).exec_chat(target(addr), request(), Some(&ro)).await;
    let gd_ae = stub.last().unwrap().headers.iter().find(|(k, _)| k == "accept-encoding").map(|(_, v)| v.clone());
    stub.set(Mode::Json);
    out.insert("c2_gzip".into(), json!({
        "policy_decoded": gp.as_ref().map(|r| r.first_text() == Some(REPLY)).unwrap_or(false),
        "policy_error": gp.err().map(|e| e.to_string().chars().take(120).collect::<String>()),
        "policy_accept_encoding": gp_ae,
        "default_decoded": gd.as_ref().map(|r| r.first_text() == Some(REPLY)).unwrap_or(false),
        "default_accept_encoding": gd_ae}));
    out.insert("c2_retry_500".into(), attempts_on_status(&stub, 500, || c.exec_chat(target(addr), request(), Some(&ro))).await);
    out.insert("c2_retry_429".into(), attempts_on_status(&stub, 429, || c.exec_chat(target(addr), request(), Some(&ro))).await);
    out.insert("guard".into(), guard_report());
    }
    let ext = c3_addr(&scenario);
    // C3 body bound, one scenario per process (peak RSS is monotonic).
    if scenario == "c3-stream" {
    let hwm0 = vm_hwm_kib();
    let mut seen = 0usize;
    let mut outcome = "completed".to_string();
    match c.exec_chat_stream(target(ext), request(), Some(&raw_options())).await {
        Ok(mut s) => {
            while let Some(ev) = s.stream.next().await {
                match ev {
                    Ok(ChatStreamEvent::Chunk(ch)) => {
                        seen += ch.content.len();
                        if seen > BODY_LIMIT {
                            outcome = "capped by consumer".into();
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        outcome = format!("error {e}");
                        break;
                    }
                }
            }
        }
        Err(e) => outcome = format!("error {e}"),
    }
    let hwm1 = vm_hwm_kib();
    out.insert("c3_stream_endless_line".into(), json!({
        "outcome": outcome, "body_bytes": BIG, "hwm_growth_kib": hwm1.saturating_sub(hwm0)}));
    }
    if scenario == "c3-plain" {
    let hwm1 = vm_hwm_kib();
    let r = c.exec_chat(target(ext), request(), Some(&raw_options())).await;
    let hwm2 = vm_hwm_kib();
    out.insert("c3_plain".into(), json!({
        "ok": r.is_ok(), "error": r.err().map(|e| e.to_string().chars().take(160).collect::<String>()),
        "body_bytes": BIG, "hwm_growth_kib": hwm2.saturating_sub(hwm1)}));
    }
    println!("{}", serde_json::to_string_pretty(&Value::Object(out)).unwrap());
}
