//! Regression tests of the 2d-2 review batch for the download client.
use super::*;
use crate::assistant::download::fake_hub::{FakeHub, Reply};
use crate::assistant::model::{Spec, parse_spec};

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

#[test]
fn transition_and_embedded_ipv4_addresses_are_judged_by_the_address_inside() {
    for bad in [
        // NAT64 and 6to4 carrying a private or loopback IPv4.
        "64:ff9b::7f00:1",
        "64:ff9b::a00:1",
        "64:ff9b::c0a8:1",
        "2002:7f00:1::1",
        "2002:a00:1::1",
        "2002:c0a8:101::1",
        // Site-local, IPv4-compatible, Teredo.
        "fec0::1",
        "::127.0.0.1",
        "::10.0.0.1",
        "2001:0:4136:e378:8000:63bf:3fff:fdd2",
    ] {
        assert!(!public(bad.parse().unwrap()), "{bad}");
    }
    // The same transition forms around a public IPv4 are fine (NAT64, 6to4).
    for good in ["64:ff9b::808:808", "2002:808:808::1"] {
        assert!(public(good.parse().unwrap()), "{good}");
    }
}

#[tokio::test]
async fn a_redirect_status_that_is_not_followed_is_reported_as_the_status() {
    let hub = FakeHub::start(&names(), |_| Reply::status(500)).await;
    let client = Client::new(&hub.endpoints()).unwrap();
    let url = client.resolve_url(&spec(), "granite-4.2-3b-Q4_K_M.gguf");
    for status in [300_u16, 304, 305] {
        hub.respond(move |_| Reply::Full {
            status,
            headers: vec![("location", "https://cdn-lfs.hf.co/x".into())],
            body: Vec::new(),
        });
        let Err(error) = client.open(url.clone(), 0).await else {
            panic!("{status} must be refused")
        };
        assert_eq!(error, Error::Status(status), "{status}");
    }
    // A redirect without a Location is a status too.
    hub.respond(|_| Reply::status(302));
    let Err(error) = client.open(url, 0).await else {
        panic!("must be refused")
    };
    assert_eq!(error, Error::Status(302));
}

#[tokio::test]
async fn concurrent_requests_each_report_their_own_rejected_redirect() {
    let hub = FakeHub::start(&names(), |_| Reply::status(500)).await;
    let port = hub.addr.port();
    hub.respond(move |seen| {
        if seen.path.contains("granite") {
            Reply::redirect(&format!("https://evil.example:{port}/x"))
        } else {
            Reply::Delayed(Duration::from_millis(20), Box::new(Reply::text("fine")))
        }
    });
    let client = Client::new(&hub.endpoints()).unwrap();
    let bad = client.resolve_url(&spec(), "granite-4.2-3b-Q4_K_M.gguf");
    let good = client.resolve_url(&spec(), "LICENSE");
    let mut tasks = Vec::new();
    for _ in 0..40 {
        let (first, bad) = (client.clone(), bad.clone());
        tasks.push(tokio::spawn(async move { first.open(bad, 0).await.err() }));
        let (second, good) = (client.clone(), good.clone());
        tokio::spawn(async move {
            let _ = second.small_file(good, 100).await;
        });
    }
    for task in tasks {
        assert_eq!(
            task.await.unwrap(),
            Some(Error::Redirect("evil.example".into()))
        );
    }
}

#[tokio::test]
async fn every_value_of_content_encoding_must_be_identity() {
    let hub = FakeHub::start(&names(), |_| Reply::status(500)).await;
    let client = Client::new(&hub.endpoints()).unwrap();
    let url = client.metadata_url(&spec());
    for headers in [
        vec![("content-encoding", "identity, gzip".to_owned())],
        vec![
            ("content-encoding", "identity".to_owned()),
            ("content-encoding", "gzip".to_owned()),
        ],
        vec![("content-encoding", "gzip, identity".to_owned())],
    ] {
        hub.respond(move |_| Reply::Full {
            status: 200,
            headers: headers.clone(),
            body: b"ok".to_vec(),
        });
        assert_eq!(
            client.api_json(url.clone(), 100).await,
            Err(Error::Encoding)
        );
    }
    hub.respond(|_| Reply::Full {
        status: 200,
        headers: vec![
            ("content-encoding", "identity".to_owned()),
            ("content-encoding", "Identity".to_owned()),
        ],
        body: b"ok".to_vec(),
    });
    assert!(client.api_json(url, 100).await.is_ok());
}
