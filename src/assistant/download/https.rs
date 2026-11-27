//! The shared HTTPS client (docs/api-reviewer-plan.md step 2, DEP corrected: the HTTP client
//! crate is in the tree): rustls with the `ring` provider and the platform verifier (test roots
//! in test builds), a resolver that refuses local addresses, no proxy for the reviewer, no
//! redirects, identity encoding only, response bodies bounded at [`BODY_LIMIT`] plain and
//! streamed, and errors that never carry a URL, a credential header or a body. The model
//! downloads (`client.rs`) and the API reviewer (`crate::sources::reviewer`) both build on it.
//!
//! The reviewer sends through it as rig's `HttpClientExt`, so rig-core builds the requests and
//! Mimik keeps the wire. A base URL is HTTPS only, names a domain (IP literals refused, in every
//! form the URL parser accepts), and carries no userinfo: refused when the client is built and
//! again before every send.
use bytes::Bytes;
use reqwest::{
    dns::{Addrs, Name, Resolve, Resolving},
    header::{ACCEPT_ENCODING, CONTENT_TYPE, HeaderMap, HeaderValue},
    redirect::Policy,
};
use rig_core::http_client::{self, HttpClientExt, LazyBody, MultipartForm, StreamingResponse};
#[cfg(test)]
use rustls::pki_types::CertificateDer;
#[cfg(test)]
use std::collections::HashMap;
use std::{
    future::Future,
    net::{IpAddr, Ipv6Addr, SocketAddr},
    pin::Pin,
    sync::Arc,
    time::Duration,
};
use url::Url;

/// The most bytes of a response body, plain or streamed.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
pub const BODY_LIMIT: usize = 4 * 1024 * 1024;
/// The connect timeout of the reviewer's client.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// The per-page total budget, including any retry: the caller's, applied around one page's
/// whole call (the job of docs/api-reviewer-plan.md step 5 owns it).
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
pub const PAGE_BUDGET: Duration = Duration::from_secs(300);

/// An error of this module. Its text never holds a URL, a key, an `Authorization` header or a
/// body; status codes map to fixed messages.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
pub enum Error {
    Setup(&'static str),
    NoTrustStore,
    Base(&'static str),
    Redirect,
    Address,
    Tls,
    Timeout,
    Connect,
    TooLarge,
    Transfer,
    Status(u16),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Setup(reason) => write!(formatter, "the HTTP client cannot be set up: {reason}"),
            Self::NoTrustStore => formatter.write_str(
                "no trusted root certificates were found on this system: install the system CA \
                 certificates (for example the ca-certificates package)",
            ),
            Self::Base(reason) => write!(formatter, "the base URL is refused: {reason}"),
            Self::Redirect => {
                formatter.write_str("the provider answered with a redirect, which is refused")
            }
            Self::Address => formatter
                .write_str("the provider's address is not allowed (local, private or reserved)"),
            Self::Tls => formatter.write_str("the provider's certificate is not trusted"),
            Self::Timeout => formatter.write_str("the connection timed out"),
            Self::Connect => formatter.write_str("the connection failed"),
            Self::TooLarge => formatter.write_str("the answer is longer than the 4 MiB bound"),
            Self::Transfer => formatter.write_str("the transfer failed"),
            Self::Status(status) => formatter.write_str(&status_message(*status)),
        }
    }
}

impl std::error::Error for Error {}

/// The fixed message a provider status maps to. The status is a number; the provider's error
/// body is never read into it.
#[must_use]
pub fn status_message(status: u16) -> String {
    match status {
        401 | 403 => "the provider rejected the API key (HTTP 401/403)".to_owned(),
        404 => "the provider has no such endpoint or model (HTTP 404)".to_owned(),
        413 => "the provider refused the request as too large (HTTP 413)".to_owned(),
        429 => "the provider is rate-limiting this key (HTTP 429)".to_owned(),
        500..=599 => "the provider failed (HTTP 5xx)".to_owned(),
        other => format!("the provider answered HTTP {other}"),
    }
}

/// A transport failure, classified without its URL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    Timeout,
    Address,
    Tls,
    Connect,
    Redirect,
    Transfer,
}

impl From<Failure> for Error {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Timeout => Self::Timeout,
            Failure::Address => Self::Address,
            Failure::Tls => Self::Tls,
            Failure::Connect => Self::Connect,
            Failure::Redirect => Self::Redirect,
            Failure::Transfer => Self::Transfer,
        }
    }
}

fn chain_has<T: std::error::Error + 'static>(error: &(dyn std::error::Error + 'static)) -> bool {
    if error.downcast_ref::<T>().is_some() {
        return true;
    }
    // An `io::Error` wraps its cause without exposing it through `source()`.
    if let Some(io) = error.downcast_ref::<std::io::Error>()
        && io.get_ref().is_some_and(|inner| chain_has::<T>(inner))
    {
        return true;
    }
    error.source().is_some_and(chain_has::<T>)
}

/// Turns a `reqwest` error into a URL-free classification.
pub(crate) fn classify(error: reqwest::Error) -> Failure {
    let error = error.without_url();
    if error.is_timeout() {
        return Failure::Timeout;
    }
    if chain_has::<AddressRefused>(&error) {
        return Failure::Address;
    }
    if chain_has::<rustls::Error>(&error) {
        return Failure::Tls;
    }
    if error.is_connect() {
        return Failure::Connect;
    }
    if error.is_redirect() {
        return Failure::Redirect;
    }
    Failure::Transfer
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
fn classify_error(error: reqwest::Error) -> Error {
    Error::from(classify(error))
}

/// The marker error a refused address travels in through the connector.
#[derive(Debug, thiserror::Error)]
#[error("address refused")]
pub(crate) struct AddressRefused;

/// Which addresses a hostname may resolve to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressPolicy {
    /// Only public unicast addresses (the shipped policy).
    Public,
    /// Anything (the tests' loopback fake server; never constructed by the shipped builders).
    #[cfg(test)]
    Any,
}

impl AddressPolicy {
    #[must_use]
    pub fn allows(self, ip: IpAddr) -> bool {
        match self {
            Self::Public => public(ip),
            #[cfg(test)]
            Self::Any => true,
        }
    }
}

fn public_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return public(IpAddr::V4(v4));
    }
    let groups = ip.segments();
    // Transition forms carry an IPv4 address that decides: NAT64 (64:ff9b::/96) and 6to4
    // (2002::/16, the address in the next two groups).
    let embedded = |high: u16, low: u16| {
        let (high, low) = (high.to_be_bytes(), low.to_be_bytes());
        std::net::Ipv4Addr::new(high[0], high[1], low[0], low[1])
    };
    if groups[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return public(IpAddr::V4(embedded(groups[6], groups[7])));
    }
    // The local-use NAT64 prefix 64:ff9b:1::/48 (RFC 8215) with the /48 layout of RFC 6052:
    // the IPv4 address is split around the reserved octet 8, in octets 6, 7, 9 and 10.
    if groups[..3] == [0x64, 0xff9b, 0x1] {
        let octets = ip.octets();
        return public(IpAddr::V4(std::net::Ipv4Addr::new(
            octets[6], octets[7], octets[9], octets[10],
        )));
    }
    if groups[0] == 0x2002 {
        return public(IpAddr::V4(embedded(groups[1], groups[2])));
    }
    let loopback_or_reserved = ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_unique_local()
        || ip.is_unicast_link_local();
    // Site-local (fec0::/10, deprecated), IPv4-compatible (::a.b.c.d), Teredo (2001::/32),
    // documentation (2001:db8::/32) and the discard prefix (100::/64).
    let site_local = groups[0] & 0xffc0 == 0xfec0;
    let ipv4_compatible = groups[..6] == [0, 0, 0, 0, 0, 0];
    let teredo = groups[0] == 0x2001 && groups[1] == 0;
    let documentation = groups[0] == 0x2001 && groups[1] == 0x0db8;
    let discard = groups[0] == 0x0100 && groups[1..4] == [0, 0, 0];
    !(loopback_or_reserved || site_local || ipv4_compatible || teredo || documentation || discard)
}

/// Whether `ip` is a public unicast address: not loopback, private, link-local, multicast,
/// unspecified, shared (CGNAT), reserved or documentation space.
#[must_use]
pub fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                // 100.64.0.0/10 (shared), 0.0.0.0/8, 192.0.0.0/24, 198.18.0.0/15, 240.0.0.0/4.
                || (o[0] == 100 && (64..128).contains(&o[1]))
                || o[0] == 0
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
                || o[0] >= 240)
        }
        IpAddr::V6(v6) => public_v6(v6),
    }
}

/// How a host name is looked up (replaced in tests).
pub type Lookup = Arc<
    dyn Fn(String) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<IpAddr>>> + Send>>
        + Send
        + Sync,
>;

fn system_lookup() -> Lookup {
    Arc::new(|host: String| {
        Box::pin(async move {
            Ok(tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .map(|addr| addr.ip())
                .collect())
        })
    })
}

/// The resolver every client of this module uses: the policy's addresses only.
pub(crate) struct Resolver {
    policy: AddressPolicy,
    lookup: Lookup,
    /// Fixed addresses, bypassing the policy. Test-only: the field does not exist in a
    /// non-test build, so production cannot skip the policy.
    #[cfg(test)]
    overrides: HashMap<String, SocketAddr>,
}

impl Resolve for Resolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().trim_end_matches('.').to_ascii_lowercase();
        #[cfg(test)]
        if let Some(address) = self.overrides.get(&host).copied() {
            return Box::pin(async move { Ok(Box::new(std::iter::once(address)) as Addrs) });
        }
        let (policy, lookup) = (self.policy, Arc::clone(&self.lookup));
        Box::pin(async move {
            let found = lookup(host).await?;
            let allowed: Vec<SocketAddr> = found
                .into_iter()
                .filter(|ip| policy.allows(*ip))
                .map(|ip| SocketAddr::new(ip, 0))
                .collect();
            if allowed.is_empty() {
                return Err(Box::new(AddressRefused) as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(allowed.into_iter()) as Addrs)
        })
    }
}

/// Where the trusted roots come from.
#[derive(Clone)]
pub enum Roots {
    /// The operating system's trust store, through the platform verifier.
    Platform,
    /// Exactly these certificates and nothing else (tests).
    #[cfg(test)]
    Only(Vec<CertificateDer<'static>>),
}

/// How the system proxy is used.
#[derive(Clone)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
pub enum ProxyMode {
    /// reqwest's system proxy (environment variables, `NO_PROXY`, OS settings).
    System,
    /// No proxy at all: `.no_proxy()` (the reviewer's shipped mode; tests are isolated from
    /// the ambient environment).
    Direct,
    /// This proxy (tests).
    #[cfg(test)]
    Explicit(Box<reqwest::Proxy>),
}

/// Everything a client of this module needs from its configuration.
#[derive(Clone)]
pub struct Transport {
    pub roots: Roots,
    pub addresses: AddressPolicy,
    pub lookup: Option<Lookup>,
    /// Fixed addresses for the resolver, bypassing the address policy. Test-only (the fake
    /// server's): in a non-test build the field does not exist, so it cannot skip the policy.
    #[cfg(test)]
    pub overrides: HashMap<String, SocketAddr>,
    pub proxy: ProxyMode,
    pub connect_timeout: Duration,
    pub read_timeout: Option<Duration>,
    pub user_agent: String,
}

fn tls_config(roots: &Roots) -> Result<rustls::ClientConfig, Error> {
    use rustls_platform_verifier::BuilderVerifierExt;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| Error::Setup("the TLS protocol versions cannot be set"))?;
    let mut config = match roots {
        Roots::Platform => builder
            .with_platform_verifier()
            .map_err(|_| Error::NoTrustStore)?
            .with_no_client_auth(),
        #[cfg(test)]
        Roots::Only(certificates) => {
            let mut store = rustls::RootCertStore::empty();
            for certificate in certificates {
                store
                    .add(certificate.clone())
                    .map_err(|_| Error::Setup("a trusted root certificate is malformed"))?;
            }
            builder.with_root_certificates(store).with_no_client_auth()
        }
    };
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

impl Transport {
    /// The reviewer's shipped transport: the OS trust store, public addresses only, no proxy,
    /// the [`CONNECT_TIMEOUT`] connect timeout and no idle read timeout (the per-page budget is
    /// the caller's, [`PAGE_BUDGET`]).
    #[must_use]
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "called by the send path of docs/api-reviewer-plan.md step 4"
        )
    )]
    pub fn reviewer() -> Self {
        Self {
            roots: Roots::Platform,
            addresses: AddressPolicy::Public,
            lookup: None,
            #[cfg(test)]
            overrides: HashMap::new(),
            proxy: ProxyMode::Direct,
            connect_timeout: CONNECT_TIMEOUT,
            read_timeout: None,
            user_agent: concat!("mimik-reviewer/", env!("CARGO_PKG_VERSION")).to_owned(),
        }
    }

    /// Builds one client over this configuration, following `redirect` (the reviewer passes
    /// `Policy::none()`: every redirect is refused).
    ///
    /// # Errors
    ///
    /// TLS that cannot be set up, or a client that cannot be built.
    pub fn build(&self, redirect: Policy) -> Result<reqwest::Client, Error> {
        let tls = tls_config(&self.roots)?;
        let resolver: Arc<dyn Resolve> = Arc::new(Resolver {
            policy: self.addresses,
            lookup: self.lookup.clone().unwrap_or_else(system_lookup),
            #[cfg(test)]
            overrides: self.overrides.clone(),
        });
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        let mut builder = reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .https_only(true)
            .http1_only()
            .referer(false)
            .redirect(redirect)
            .user_agent(self.user_agent.clone())
            .default_headers(headers)
            .connect_timeout(self.connect_timeout)
            .pool_max_idle_per_host(0)
            .dns_resolver(resolver);
        if let Some(read_timeout) = self.read_timeout {
            builder = builder.read_timeout(read_timeout);
        }
        builder = match &self.proxy {
            ProxyMode::System => builder,
            ProxyMode::Direct => builder.no_proxy(),
            #[cfg(test)]
            ProxyMode::Explicit(proxy) => builder.no_proxy().proxy((**proxy).clone()),
        };
        builder
            .build()
            .map_err(|_| Error::Setup("the HTTP client cannot be built"))
    }
}

/// Whether `name` is a domain name and not an IP literal in any form. The URL parser turns
/// `127.1`, `0x7f.1`, `2130706433` and `[::1]` into IP hosts, so [`check_url`] refuses those
/// through the host check; this validates the remaining labels and refuses brackets directly.
fn is_domain_name(name: &str) -> bool {
    if name.is_empty() || name.starts_with('[') || name.ends_with(']') {
        return false;
    }
    name.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}

/// Checks a URL the way a base URL is checked: HTTPS, a domain name, no userinfo.
///
/// # Errors
///
/// The refusal reason, as a fixed text.
pub fn check_url(url: &Url) -> Result<(), &'static str> {
    if url.scheme() != "https" {
        return Err("it must be HTTPS");
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("it must not carry a user name or password");
    }
    match url.host() {
        Some(url::Host::Domain(name)) if is_domain_name(name) => Ok(()),
        _ => Err("it must name a domain, not an IP address"),
    }
}

/// Parses and checks a base URL (build time).
///
/// # Errors
///
/// The refusal reason, as a fixed text.
pub fn validate_base_url(raw: &str) -> Result<Url, &'static str> {
    let url = Url::parse(raw).map_err(|_| "it is malformed")?;
    check_url(&url)?;
    Ok(url)
}

/// The URL of a request about to be sent, checked again (before each send).
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
fn checked_request_url(uri: &http::Uri) -> Result<Url, Error> {
    let raw = uri.to_string();
    let url = Url::parse(&raw).map_err(|_| Error::Base("the request URL is malformed"))?;
    check_url(&url).map_err(Error::Base)?;
    Ok(url)
}

/// Reads a whole response body through the bound.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
async fn bounded(mut response: reqwest::Response, max: usize) -> Result<Bytes, Error> {
    if response
        .content_length()
        .is_some_and(|length| length > max as u64)
    {
        return Err(Error::TooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(classify_error)? {
        if body.len() + chunk.len() > max {
            return Err(Error::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(body))
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
fn rebuild<B>(
    status: http::StatusCode,
    headers: http::HeaderMap,
    body: B,
) -> Result<http::Response<B>, http::Error> {
    let mut out = http::Response::builder().status(status).body(body)?;
    *out.headers_mut() = headers;
    Ok(out)
}

/// Maps this module's error into rig's transport error: refusals are permanent, transport
/// failures are transient.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
fn rig_error(error: Error) -> http_client::Error {
    match error {
        Error::Timeout | Error::Connect | Error::Transfer => http_client::Error::transient(error),
        _ => http_client::Error::permanent(error),
    }
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
fn rig_reqwest(error: reqwest::Error) -> http_client::Error {
    rig_error(classify_error(error))
}

/// The reviewer's client over the shared configuration: no redirects, no proxy, HTTPS only, a
/// domain base URL checked at build time and before every send, bodies bounded at
/// [`BODY_LIMIT`]. It implements rig's `HttpClientExt`, so rig-core's provider wires send
/// through it.
#[derive(Clone)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
pub struct Https {
    client: reqwest::Client,
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
impl Https {
    /// Builds the client; redirects are always refused.
    ///
    /// # Errors
    ///
    /// TLS that cannot be set up, or a client that cannot be built.
    pub fn new(transport: &Transport) -> Result<Self, Error> {
        transport
            .build(Policy::none())
            .map(|client| Self { client })
    }

    /// One request outside rig (tests and plain callers): the URL is checked again, the reply's
    /// status maps to a fixed message, and the body is read through the bound.
    ///
    /// # Errors
    ///
    /// A refused URL, a refused redirect or address, a transport failure, an oversized body,
    /// or a non-success status.
    pub async fn request(
        &self,
        method: http::Method,
        url: &Url,
        body: Bytes,
    ) -> Result<Bytes, Error> {
        check_url(url).map_err(Error::Base)?;
        let response = self
            .client
            .request(method, url.clone())
            .header(CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(classify_error)?;
        let status = response.status();
        if status.is_redirection() {
            return Err(Error::Redirect);
        }
        if !status.is_success() {
            // The error body is never read.
            return Err(Error::Status(status.as_u16()));
        }
        bounded(response, BODY_LIMIT).await
    }
}

impl HttpClientExt for Https {
    fn send(
        &self,
        request: http::Request<Bytes>,
    ) -> impl Future<Output = http_client::Result<http::Response<LazyBody>>> + Send + 'static {
        let this = self.clone();
        async move {
            let (parts, body) = request.into_parts();
            let url = checked_request_url(&parts.uri).map_err(rig_error)?;
            let response = this
                .client
                .request(parts.method.clone(), url)
                .headers(parts.headers.clone())
                .body(body)
                .send()
                .await
                .map_err(rig_reqwest)?;
            let status = response.status();
            let headers = response.headers().clone();
            if status.is_redirection() {
                return Err(rig_error(Error::Redirect));
            }
            if !status.is_success() {
                // The error body is never read: it could only be logged, and nothing is.
                return Err(http_client::Error::non_success_with_details(
                    status,
                    headers,
                    String::new(),
                ));
            }
            let bytes = bounded(response, BODY_LIMIT).await.map_err(rig_error)?;
            let lazy: LazyBody = Box::pin(async move { Ok::<Bytes, http_client::Error>(bytes) });
            rebuild(status, headers, lazy).map_err(http_client::Error::Protocol)
        }
    }

    fn send_multipart(
        &self,
        _request: http::Request<MultipartForm>,
    ) -> impl Future<Output = http_client::Result<http::Response<LazyBody>>> + Send + 'static {
        std::future::ready(Err(http_client::Error::permanent(Error::Setup(
            "multipart requests are not used",
        ))))
    }

    fn send_streaming(
        &self,
        request: http::Request<Bytes>,
    ) -> impl Future<Output = http_client::Result<StreamingResponse>> + Send {
        let this = self.clone();
        async move {
            let (parts, body) = request.into_parts();
            let url = checked_request_url(&parts.uri).map_err(rig_error)?;
            let response = this
                .client
                .request(parts.method.clone(), url)
                .headers(parts.headers.clone())
                .body(body)
                .send()
                .await
                .map_err(rig_reqwest)?;
            let status = response.status();
            let headers = response.headers().clone();
            if status.is_redirection() {
                return Err(rig_error(Error::Redirect));
            }
            if !status.is_success() {
                return Err(http_client::Error::non_success_with_details(
                    status,
                    headers,
                    String::new(),
                ));
            }
            // The state is `None` once an error has been yielded: the stream ends there
            // instead of reading on past the bound or the read failure.
            let stream =
                futures_util::stream::unfold(Some((response, 0_usize)), |state| async move {
                    let (mut response, seen) = state?;
                    match response.chunk().await {
                        Ok(Some(chunk)) => {
                            let seen = seen.saturating_add(chunk.len());
                            if seen > BODY_LIMIT {
                                Some((Err(rig_error(Error::TooLarge)), None))
                            } else {
                                Some((Ok(chunk), Some((response, seen))))
                            }
                        }
                        Ok(None) => None,
                        Err(error) => Some((Err(rig_reqwest(error)), None)),
                    }
                });
            let boxed: http_client::BoxedStream = Box::pin(stream);
            rebuild(status, headers, boxed).map_err(http_client::Error::Protocol)
        }
    }
}
