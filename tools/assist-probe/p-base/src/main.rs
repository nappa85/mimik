//! Baseline: the same stub, axum, tokio and TLS-free reqwest, without any provider crate.
use serde_json::json;
use stub::*;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("serve") {
        serve_c3().await;
    }
    let (addr, stub) = start_stub().await;
    let c = policy_client();
    let r = c
        .post(format!("http://{addr}/v1/chat/completions"))
        .json(&json!({"model": "probe", "messages": [], "response_format": {"type": "json_schema",
            "json_schema": {"name": "x", "schema": schema()}}}))
        .send()
        .await
        .unwrap();
    let body = bounded_body(r, BODY_LIMIT).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    println!("{}", json!({"ok": v["choices"][0]["message"]["content"] == REPLY,
        "schema_equal": stub.last().unwrap().body["response_format"]["json_schema"]["schema"] == schema(),
        "guard": guard_report()}));
}
