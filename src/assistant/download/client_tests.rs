use super::*;
use crate::assistant::download::fake_hub::{FakeHub, Reply, certificates};
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

async fn hub(
    reply: impl FnMut(&crate::assistant::download::fake_hub::Seen) -> Reply + Send + 'static,
) -> FakeHub {
    FakeHub::start(&names(), reply).await
}

fn client(hub: &FakeHub) -> Client {
    Client::new(&hub.endpoints()).unwrap()
}

fn hosts(list: &[&str]) -> HashSet<String> {
    list.iter().map(|host| (*host).to_owned()).collect()
}

#[test]
fn urls_are_built_by_segment_never_by_concatenation() {
    let hub_less = Client::new(&Endpoints {
        roots: Roots::Only(Vec::new()),
        proxy: ProxyMode::Direct,
        ..Endpoints::shipped()
    })
    .unwrap();
    let spec = spec();
    assert_eq!(
        hub_less.metadata_url(&spec).as_str(),
        format!(
            "https://huggingface.co/api/models/ibm-granite/granite-4.2-3b-GGUF/revision/{REV}?blobs=true"
        )
    );
    assert_eq!(
        hub_less
            .resolve_url(&spec, "granite-4.2-3b-Q4_K_M.gguf")
            .as_str(),
        format!(
            "https://huggingface.co/ibm-granite/granite-4.2-3b-GGUF/resolve/{REV}/granite-4.2-3b-Q4_K_M.gguf"
        )
    );
    // Segments are encoded: a stray character cannot add a path level or a query.
    let url = hub_less.resolve_url(&spec, "a/b c?x#y");
    assert_eq!(url.query(), None);
    assert_eq!(url.fragment(), None);
    assert!(url.path().ends_with("/a/b%20c%3Fx%23y"), "{url}");
}

#[test]
fn a_redirect_destination_is_judged_before_it_is_sent() {
    let list = hosts(&["cdn-lfs.hf.co", "huggingface.co"]);
    let judged = |text: &str, hops: usize| judge(&Url::parse(text).unwrap(), hops, &list, 443);
    assert_eq!(judged("https://cdn-lfs.hf.co/x?sig=1", 1), Verdict::Follow);
    // Case and a trailing dot are normalised, not a way round the list.
    assert_eq!(judged("https://CDN-LFS.HF.CO/x", 1), Verdict::Follow);
    assert_eq!(judged("https://cdn-lfs.hf.co./x", 1), Verdict::Follow);
    assert_eq!(judged("https://cdn-lfs.hf.co/x", 2), Verdict::Follow);
    for (url, hops, expected) in [
        (
            "https://evil.example/x",
            1,
            Rejection::Host("evil.example".into()),
        ),
        (
            "https://cdn-lfs.hf.co.evil.example/x",
            1,
            Rejection::Host("cdn-lfs.hf.co.evil.example".into()),
        ),
        (
            "https://sub.cdn-lfs.hf.co/x",
            1,
            Rejection::Host("sub.cdn-lfs.hf.co".into()),
        ),
        ("http://cdn-lfs.hf.co/x", 1, Rejection::Shape),
        ("https://cdn-lfs.hf.co:8443/x", 1, Rejection::Shape),
        ("https://user:pw@cdn-lfs.hf.co/x", 1, Rejection::Shape),
        ("https://user@cdn-lfs.hf.co/x", 1, Rejection::Shape),
        ("https://cdn-lfs.hf.co/x", 3, Rejection::Shape),
        ("https://127.0.0.1/x", 1, Rejection::Shape),
        ("https://[::1]/x", 1, Rejection::Shape),
        ("ftp://cdn-lfs.hf.co/x", 1, Rejection::Shape),
    ] {
        assert_eq!(judged(url, hops), Verdict::Refuse(expected), "{url}");
    }
}

#[test]
fn only_public_addresses_are_allowed() {
    for bad in [
        "127.0.0.1",
        "10.1.2.3",
        "172.16.0.1",
        "192.168.1.1",
        "169.254.1.1",
        "0.0.0.0",
        "224.0.0.1",
        "255.255.255.255",
        "100.64.0.1",
        "198.18.0.1",
        "192.0.2.1",
        "240.0.0.1",
        "::1",
        "::",
        "fe80::1",
        "fc00::1",
        "fd12::1",
        "ff02::1",
        "::ffff:127.0.0.1",
        "::ffff:10.0.0.1",
        "2001:db8::1",
    ] {
        assert!(!public(bad.parse().unwrap()), "{bad}");
    }
    for good in [
        "8.8.8.8",
        "1.1.1.1",
        "13.32.0.1",
        "2606:4700:4700::1111",
        "::ffff:8.8.8.8",
    ] {
        assert!(public(good.parse().unwrap()), "{good}");
    }
}

#[tokio::test]
async fn the_api_is_read_with_the_headers_the_policy_names() {
    let hub = hub(|_| Reply::json("{\"sha\":\"x\"}")).await;
    let body = client(&hub)
        .api_json(client(&hub).metadata_url(&spec()), 1 << 20)
        .await
        .unwrap();
    assert_eq!(&body[..], b"{\"sha\":\"x\"}");
    let seen = hub.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].host, "huggingface.co");
    assert_eq!(
        seen[0].path,
        format!("/api/models/ibm-granite/granite-4.2-3b-GGUF/revision/{REV}")
    );
    assert_eq!(seen[0].query, "blobs=true");
    let headers = &seen[0].headers;
    assert_eq!(headers.get("accept-encoding").unwrap(), "identity");
    assert!(
        headers
            .get("user-agent")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("mimik-model-download/")
    );
    for forbidden in ["cookie", "authorization", "referer", "proxy-authorization"] {
        assert!(headers.get(forbidden).is_none(), "{forbidden}");
    }
}

#[tokio::test]
async fn a_resolve_redirect_to_a_listed_host_is_followed_with_the_same_headers() {
    let hub = hub(|_| Reply::status(500)).await;
    // The Location needs the server's port: answer by host with the real port.
    let port = hub.addr.port();
    hub.respond(move |seen| {
        if seen.host == "huggingface.co" {
            Reply::redirect(&format!(
                "https://cdn-lfs.hf.co:{port}/blob?X-Amz-Signature=SECRET"
            ))
        } else {
            Reply::text("model")
        }
    });
    let client = client(&hub);
    let mut body = client
        .open(client.resolve_url(&spec(), "granite-4.2-3b-Q4_K_M.gguf"), 0)
        .await
        .unwrap();
    assert_eq!(body.status, 200);
    assert_eq!(&body.chunk().await.unwrap().unwrap()[..], b"model");
    let seen = hub.requests();
    assert_eq!(
        seen.iter().map(|s| s.host.as_str()).collect::<Vec<_>>(),
        ["huggingface.co", "cdn-lfs.hf.co"]
    );
    for request in &seen {
        assert_eq!(request.headers.get("accept-encoding").unwrap(), "identity");
        for forbidden in ["cookie", "authorization", "referer"] {
            assert!(request.headers.get(forbidden).is_none(), "{forbidden}");
        }
    }
}

#[tokio::test]
async fn a_redirect_off_the_list_or_off_https_reaches_nobody() {
    let port = std::sync::Arc::new(std::sync::atomic::AtomicU16::new(0));
    for (location, expected) in [
        (
            "https://evil.example:{port}/x",
            Error::Redirect("evil.example".into()),
        ),
        ("http://cdn-lfs.hf.co:{port}/x", Error::RedirectShape),
        (
            "https://user:pw@cdn-lfs.hf.co:{port}/x",
            Error::RedirectShape,
        ),
        ("https://cdn-lfs.hf.co:1/x", Error::RedirectShape),
    ] {
        let hub = hub(|_| Reply::status(500)).await;
        let target = location.replace("{port}", &hub.addr.port().to_string());
        port.store(hub.addr.port(), std::sync::atomic::Ordering::SeqCst);
        hub.respond(move |_| Reply::redirect(&target));
        let client = client(&hub);
        let Err(error) = client
            .open(client.resolve_url(&spec(), "granite-4.2-3b-Q4_K_M.gguf"), 0)
            .await
        else {
            panic!("{location} must be refused")
        };
        assert_eq!(error, expected, "{location}");
        // Exactly the first request was sent: the rejected destination received nothing.
        assert_eq!(hub.requests().len(), 1, "{location}");
        assert_eq!(hub.requests()[0].host, "huggingface.co");
    }
}

#[tokio::test]
async fn a_third_redirect_hop_stops_the_download() {
    let hub = hub(|_| Reply::status(500)).await;
    let port = hub.addr.port();
    hub.respond(move |_| Reply::redirect(&format!("https://cdn-lfs.hf.co:{port}/next")));
    let client = client(&hub);
    let result = client
        .open(client.resolve_url(&spec(), "granite-4.2-3b-Q4_K_M.gguf"), 0)
        .await;
    assert!(
        matches!(result, Err(Error::RedirectShape)),
        "hops are bounded"
    );
    assert_eq!(
        hub.requests().len(),
        3,
        "the original and two hops, no more"
    );
}

#[tokio::test]
async fn an_api_redirect_a_denial_and_a_foreign_encoding_are_refused() {
    let hub = hub(|_| Reply::redirect("https://cdn-lfs.hf.co/x")).await;
    let client = Client::new(&hub.endpoints()).unwrap();
    let url = client.metadata_url(&spec());
    assert_eq!(
        client.api_json(url.clone(), 1000).await,
        Err(Error::Status(302))
    );
    assert_eq!(hub.requests().len(), 1);
    for status in [401, 403] {
        hub.respond(move |_| Reply::status(status));
        assert_eq!(
            client.api_json(url.clone(), 1000).await,
            Err(Error::Denied(status))
        );
    }
    hub.respond(|_| Reply::Full {
        status: 200,
        headers: vec![("content-encoding", "gzip".into())],
        body: vec![0x1f, 0x8b],
    });
    assert_eq!(
        client.api_json(url.clone(), 1000).await,
        Err(Error::Encoding)
    );
    // `identity` is the one encoding that is fine.
    hub.respond(|_| Reply::Full {
        status: 200,
        headers: vec![("content-encoding", "identity".into())],
        body: b"ok".to_vec(),
    });
    assert!(client.api_json(url, 1000).await.is_ok());
}

#[tokio::test]
async fn an_answer_over_the_bound_is_refused_not_cut() {
    let hub = hub(|_| Reply::text(&"x".repeat(2000))).await;
    let client = client(&hub);
    let url = client.metadata_url(&spec());
    assert_eq!(
        client.api_json(url.clone(), 1999).await,
        Err(Error::TooLarge)
    );
    assert_eq!(
        client.api_json(url.clone(), 2000).await.unwrap().len(),
        2000
    );
    // Without a Content-Length (a stream) the bound still holds.
    hub.respond(|_| Reply::Chunked {
        status: 200,
        headers: Vec::new(),
        body: vec![b'x'; 2000],
        piece: 100,
        gap: Duration::from_millis(1),
    });
    assert_eq!(client.api_json(url, 1500).await, Err(Error::TooLarge));
}

#[tokio::test(start_paused = true)]
async fn a_slow_loris_answer_is_cut_by_the_total_deadline() {
    let hub = hub(|_| Reply::Stall {
        status: 200,
        headers: Vec::new(),
        first: b"{".to_vec(),
        declared: None,
    })
    .await;
    let client = client(&hub);
    let started = tokio::time::Instant::now();
    let result = client.api_json(client.metadata_url(&spec()), 1 << 20).await;
    assert_eq!(result, Err(Error::Timeout));
    assert!(started.elapsed() >= METADATA_DEADLINE);
    assert!(started.elapsed() < METADATA_DEADLINE + Duration::from_secs(5));
    // The licence request has its own, longer deadline.
    let started = tokio::time::Instant::now();
    let result = client
        .small_file(client.resolve_url(&spec(), "LICENSE"), 1 << 20)
        .await;
    assert_eq!(result, Err(Error::Timeout));
    assert!(started.elapsed() >= LICENCE_DEADLINE);
}

#[tokio::test]
async fn a_server_not_signed_by_the_trusted_roots_or_for_another_name_is_refused() {
    let hub = hub(|_| Reply::text("never read")).await;
    // Another CA than the server's.
    let (other_ca, _, _) = certificates(&["huggingface.co"]);
    let mut endpoints = hub.endpoints();
    endpoints.roots = Roots::Only(vec![other_ca]);
    let client = Client::new(&endpoints).unwrap();
    assert_eq!(
        client.api_json(client.metadata_url(&spec()), 100).await,
        Err(Error::Tls)
    );
    // The right CA but a certificate for another name.
    let wrong = FakeHub::start(&["other.example"], |_| Reply::text("never read")).await;
    let client = Client::new(&wrong.endpoints()).unwrap();
    assert_eq!(
        client.api_json(client.metadata_url(&spec()), 100).await,
        Err(Error::Tls)
    );
    assert!(
        wrong.requests().is_empty(),
        "no request got past the handshake"
    );
    // The production-shaped constructor (the OS store) trusts neither test CA.
    let mut endpoints = hub.endpoints();
    endpoints.roots = Roots::Platform;
    match Client::new(&endpoints) {
        Ok(client) => {
            let error = client
                .api_json(client.metadata_url(&spec()), 100)
                .await
                .unwrap_err();
            assert!(matches!(error, Error::Tls | Error::NoTrustStore), "{error}");
        }
        Err(error) => assert_eq!(error, Error::NoTrustStore),
    }
    assert!(hub.requests().is_empty());
}

#[tokio::test]
async fn a_name_that_resolves_to_a_local_address_is_refused_before_connecting() {
    // DNS rebinding: the listed name answers with the loopback address.
    let hub = hub(|_| Reply::text("never read")).await;
    let mut endpoints = hub.endpoints();
    endpoints.overrides.clear();
    endpoints.addresses = AddressPolicy::Public;
    endpoints.lookup = Some(Arc::new(|_host| {
        Box::pin(async {
            Ok(vec![
                "127.0.0.1".parse().unwrap(),
                "10.0.0.5".parse().unwrap(),
            ])
        })
    }));
    let client = Client::new(&endpoints).unwrap();
    assert_eq!(
        client.api_json(client.metadata_url(&spec()), 100).await,
        Err(Error::Address)
    );
    assert!(hub.requests().is_empty());
}

#[tokio::test]
async fn an_error_on_a_redirected_hop_carries_no_url() {
    // The second hop's host is on the list but unreachable; the presigned query must not
    // reach the error, its text or its debug form.
    let hub = hub(|_| Reply::status(500)).await;
    let port = hub.addr.port();
    hub.respond(move |_| {
        Reply::redirect(&format!(
            "https://cdn-lfs-eu-1.hf.co:{port}/blob?X-Amz-Signature=SECRET&token=SECRET"
        ))
    });
    let mut endpoints = hub.endpoints();
    endpoints.overrides.remove("cdn-lfs-eu-1.hf.co");
    endpoints.addresses = AddressPolicy::Public;
    endpoints.lookup = Some(Arc::new(|_host| Box::pin(async { Ok(Vec::new()) })));
    let client = Client::new(&endpoints).unwrap();
    let Err(error) = client
        .open(client.resolve_url(&spec(), "granite-4.2-3b-Q4_K_M.gguf"), 0)
        .await
    else {
        panic!("must fail")
    };
    for text in [error.to_string(), format!("{error:?}")] {
        for secret in ["SECRET", "X-Amz", "token", "cdn-lfs-eu-1", "http"] {
            assert!(!text.contains(secret), "{text}");
        }
    }
}

#[test]
fn the_shipped_constructor_uses_the_fixed_values() {
    let shipped = Endpoints::shipped();
    assert_eq!(shipped.api_base, "https://huggingface.co");
    assert_eq!(shipped.port, 443);
    assert!(matches!(shipped.roots, Roots::Platform));
    assert_eq!(shipped.addresses, AddressPolicy::Public);
    assert!(matches!(shipped.proxy, ProxyMode::System));
    assert!(shipped.overrides.is_empty() && shipped.lookup.is_none());
    assert_eq!(shipped.hosts, SHIPPED_HOSTS);
}

#[test]
fn the_shipped_host_list_is_the_reviewed_one() {
    // A tripwire, not coverage: changing the list means editing this expectation too, and the
    // release notes (`docs/model-downloads.md`). Candidates until the live redirect observation.
    assert_eq!(
        SHIPPED_HOSTS,
        [
            "huggingface.co",
            "cdn-lfs.huggingface.co",
            "cdn-lfs-us-1.huggingface.co",
            "cdn-lfs-eu-1.huggingface.co",
            "cdn-lfs.hf.co",
            "cdn-lfs-us-1.hf.co",
            "cdn-lfs-eu-1.hf.co",
            "cas-bridge.xethub.hf.co",
            "transfer.xethub.hf.co",
        ]
    );
    // Exact names only: no wildcard, no leading dot, no scheme, all lower case.
    for host in SHIPPED_HOSTS {
        assert!(
            !host.contains(['*', '/', ':']) && !host.starts_with('.'),
            "{host}"
        );
        assert_eq!(host, host.to_ascii_lowercase());
    }
}
