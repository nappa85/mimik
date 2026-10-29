//! Proxy and trust-store tests of the download client. The process-wide environment (proxy
//! variables, the system trust store) is read once per process, so those cases run in a child
//! process with exactly the environment they test.
use super::client::*;
use super::fake_hub::{FakeHub, Reply, certificates};
use super::fake_proxy::FakeProxy;
use crate::assistant::model::{HfSpec, Spec, parse_spec};
use rustls::pki_types::CertificateDer;
use std::net::SocketAddr;

const REV: &str = "c40945d71cd90f249a56985e8155551a9188dc30";

fn spec() -> HfSpec {
    let Ok(Spec::Hf(spec)) = parse_spec(&format!(
        "hf:ibm-granite/granite-4.2-3b-GGUF/granite-4.2-3b-Q4_K_M.gguf@{REV}"
    )) else {
        panic!("fixture spec")
    };
    spec
}

fn names() -> Vec<&'static str> {
    let mut names = SHIPPED_HOSTS.to_vec();
    names.push("evil.example");
    names
}

#[tokio::test]
async fn a_configured_proxy_gets_credentials_on_connect_only_and_never_a_rejected_host() {
    let hub = FakeHub::start(&names(), |_| Reply::status(500)).await;
    let port = hub.addr.port();
    hub.respond(move |seen| {
        if seen.path.starts_with("/api/") {
            Reply::json("{}")
        } else {
            Reply::redirect(&format!("https://evil.example:{port}/x"))
        }
    });
    let proxy = FakeProxy::start(hub.addr).await;
    let mut endpoints = hub.endpoints();
    endpoints.overrides.clear();
    endpoints.proxy = ProxyMode::Explicit(Box::new(
        reqwest::Proxy::all(format!("http://user:secret@{}", proxy.addr)).unwrap(),
    ));
    let client = Client::new(&endpoints).unwrap();
    assert!(
        client
            .api_json(client.metadata_url(&spec()), 100)
            .await
            .is_ok()
    );
    // The credentials went to the proxy, in the CONNECT, and not to the origin.
    let connects = proxy.connects.lock().unwrap().clone();
    assert_eq!(connects.len(), 1);
    assert_eq!(connects[0].0, format!("huggingface.co:{port}"));
    assert!(
        connects[0]
            .1
            .to_ascii_lowercase()
            .contains("proxy-authorization: basic")
    );
    assert!(
        hub.requests()
            .iter()
            .all(|seen| seen.headers.get("proxy-authorization").is_none())
    );
    // A rejected redirect target is never even tunnelled to.
    let result = client
        .open(client.resolve_url(&spec(), "granite-4.2-3b-Q4_K_M.gguf"), 0)
        .await;
    assert!(matches!(result, Err(Error::Redirect(host)) if host == "evil.example"));
    let connects = proxy.connects.lock().unwrap().clone();
    assert!(
        connects
            .iter()
            .all(|(target, _)| target.starts_with("huggingface.co")),
        "{connects:?}"
    );
}

fn pem(der: &CertificateDer<'_>) -> String {
    use base64::Engine;
    let body = base64::engine::general_purpose::STANDARD.encode(der.as_ref());
    let lines: Vec<&str> = body
        .as_bytes()
        .chunks(64)
        .map(|line| std::str::from_utf8(line).unwrap())
        .collect();
    format!(
        "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
        lines.join("\n")
    )
}

/// Runs the `#[ignore]`d child test `name` of this binary with `env`, returning whether it
/// passed.
async fn child(name: &str, env: Vec<(&'static str, String)>) -> bool {
    let name = format!("assistant::download::net_tests::{name}");
    tokio::task::spawn_blocking(move || {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                &name,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env_remove("HTTPS_PROXY")
            .env_remove("https_proxy")
            .env_remove("ALL_PROXY")
            .env_remove("all_proxy")
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .envs(env);
        let output = command.output().unwrap();
        if !output.status.success() {
            eprintln!("{}", String::from_utf8_lossy(&output.stdout));
            eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        }
        output.status.success()
    })
    .await
    .unwrap()
}

fn env_of(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

/// The child's client: the shipped proxy and trust settings over the parent's fake server.
fn child_client(roots: Roots, proxy: ProxyMode) -> Client {
    let addr: SocketAddr = env_of("MIMIK_CHILD_ADDR").parse().unwrap();
    let mut endpoints = Endpoints::shipped();
    endpoints.api_base = format!("https://huggingface.co:{}", addr.port());
    endpoints.port = addr.port();
    endpoints.roots = roots;
    endpoints.proxy = proxy;
    endpoints.addresses = AddressPolicy::Any;
    endpoints.overrides.insert("huggingface.co".into(), addr);
    Client::new(&endpoints).unwrap()
}

#[tokio::test]
#[ignore = "run in a child process by the parent tests"]
async fn child_system_proxy() {
    let client = child_client(
        Roots::Only(vec![CertificateDer::from(
            std::fs::read(env_of("MIMIK_CHILD_CA")).unwrap(),
        )]),
        ProxyMode::System,
    );
    client
        .api_json(client.metadata_url(&spec()), 100)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "run in a child process by the parent tests"]
async fn child_platform_trust() {
    let client = child_client(Roots::Platform, ProxyMode::Direct);
    let result = client.api_json(client.metadata_url(&spec()), 100).await;
    if env_of("MIMIK_CHILD_EXPECT") == "trusted" {
        result.unwrap();
    } else {
        assert_eq!(result, Err(Error::Tls));
    }
}

#[tokio::test]
async fn the_system_proxy_is_used_from_the_environment_and_no_proxy_is_honoured() {
    let hub = FakeHub::start(&names(), |_| Reply::json("{}")).await;
    let proxy = FakeProxy::start(hub.addr).await;
    let dir = tempfile::tempdir().unwrap();
    let ca = dir.path().join("ca.der");
    std::fs::write(&ca, hub.ca.as_ref()).unwrap();
    let env = |extra: Vec<(&'static str, String)>| {
        let mut env = vec![
            ("MIMIK_CHILD_ADDR", hub.addr.to_string()),
            ("MIMIK_CHILD_CA", ca.display().to_string()),
            ("HTTPS_PROXY", format!("http://{}", proxy.addr)),
        ];
        env.extend(extra);
        env
    };
    assert!(child("child_system_proxy", env(vec![])).await);
    assert_eq!(
        proxy.connects.lock().unwrap().len(),
        1,
        "the proxy was used"
    );
    assert!(
        child(
            "child_system_proxy",
            env(vec![("NO_PROXY", "huggingface.co".into())])
        )
        .await
    );
    assert_eq!(
        proxy.connects.lock().unwrap().len(),
        1,
        "NO_PROXY bypassed it"
    );
    assert_eq!(hub.requests().len(), 2);
}

#[tokio::test]
async fn the_shipped_trust_store_accepts_what_it_holds_and_refuses_everything_else() {
    // An isolated trust store (SSL_CERT_FILE) read by the production verifier in a child: the
    // fake server's CA is trusted when it is in the store and refused when it is not.
    let hub = FakeHub::start(&names(), |_| Reply::json("{}")).await;
    let dir = tempfile::tempdir().unwrap();
    let ours = dir.path().join("ours.pem");
    std::fs::write(&ours, pem(&hub.ca)).unwrap();
    let (other, _, _) = certificates(&["huggingface.co"]);
    let theirs = dir.path().join("theirs.pem");
    std::fs::write(&theirs, pem(&other)).unwrap();
    let env = |file: &std::path::Path, expect: &str| {
        vec![
            ("MIMIK_CHILD_ADDR", hub.addr.to_string()),
            ("MIMIK_CHILD_EXPECT", expect.to_owned()),
            ("SSL_CERT_FILE", file.display().to_string()),
            (
                "SSL_CERT_DIR",
                dir.path().join("none").display().to_string(),
            ),
        ]
    };
    assert!(child("child_platform_trust", env(&ours, "trusted")).await);
    assert!(child("child_platform_trust", env(&theirs, "refused")).await);
}
