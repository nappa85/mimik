//! The download client (assisted sheet plan §1.4b, DEP-27): a `reqwest` client separate from
//! the model client, HTTPS only, port 443 only, exact host list checked before every redirect
//! hop is sent, a resolver that refuses local addresses, the system proxy for downloads only,
//! no cookies, no `Referer`, identity encoding only, and errors that never carry a URL.
use crate::assistant::model::HfSpec;
use bytes::Bytes;
use reqwest::{
    dns::{Addrs, Name, Resolve, Resolving},
    header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_RANGE, HeaderMap, HeaderValue, RANGE},
    redirect::Policy,
};
use rustls::pki_types::CertificateDer;
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    net::{IpAddr, Ipv6Addr, SocketAddr},
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use url::Url;

/// At most this many redirect hops are followed from a `resolve` URL.
const MAX_HOPS: usize = 2;
/// Connect timeout and the idle read timeout of a transfer.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Total deadlines of the small requests (slow-loris bound).
pub(crate) const METADATA_DEADLINE: Duration = Duration::from_secs(20);
pub(crate) const LICENCE_DEADLINE: Duration = Duration::from_secs(30);

/// The only API and `resolve` host.
pub(crate) const API_BASE: &str = "https://huggingface.co";

/// Exact host names a `resolve` redirect may reach (no suffix or wildcard rule). Fixed from
/// Hugging Face's documentation; the full real redirect chain of the two pinned files is
/// still to be observed by hand and this list reduced to what it shows (plan, "Live checks
/// owed"), so these are candidates until then. `tests::the_shipped_host_list_is_the_reviewed_one`
/// is a tripwire: changing the list is a deliberate edit of both.
pub(crate) const SHIPPED_HOSTS: [&str; 9] = [
    "huggingface.co",
    "cdn-lfs.huggingface.co",
    "cdn-lfs-us-1.huggingface.co",
    "cdn-lfs-eu-1.huggingface.co",
    "cdn-lfs.hf.co",
    "cdn-lfs-us-1.hf.co",
    "cdn-lfs-eu-1.hf.co",
    "cas-bridge.xethub.hf.co",
    "transfer.xethub.hf.co",
];

/// A download error. Its text never holds a URL (presigned redirect URLs carry credentials);
/// a rejected redirect names the host only.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum Error {
    #[error(
        "the server refused access (status {0}): download the file by hand and use local:<name>.gguf"
    )]
    Denied(u16),
    #[error("the server answered status {0}")]
    Status(u16),
    #[error("a redirect to {0} was refused: that host is not on the list of download hosts")]
    Redirect(String),
    #[error(
        "a redirect was refused: more than two hops, or not HTTPS on the expected port without credentials"
    )]
    RedirectShape,
    #[error("the connection timed out")]
    Timeout,
    #[error("the connection failed")]
    Connect,
    #[error("the server's certificate is not trusted")]
    Tls,
    #[error(
        "no trusted root certificates were found on this system: install the system CA certificates (for example the ca-certificates package)"
    )]
    NoTrustStore,
    #[error("the answer is longer than allowed")]
    TooLarge,
    #[error("the answer uses an encoding other than identity")]
    Encoding,
    #[error("the server's address is not allowed (local, private or reserved)")]
    Address,
    #[error("the transfer failed")]
    Transfer,
    #[error("{0}")]
    Setup(&'static str),
}

/// The marker error a refused address travels in through the connector.
#[derive(Debug, thiserror::Error)]
#[error("address refused")]
struct AddressRefused;

/// Which addresses a hostname may resolve to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AddressPolicy {
    /// Only public unicast addresses (the shipped policy).
    Public,
    /// Anything (the tests' loopback fake server; never constructed by `shipped`).
    #[cfg(test)]
    Any,
}

impl AddressPolicy {
    pub(crate) fn allows(self, ip: IpAddr) -> bool {
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
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
        // Documentation (2001:db8::/32) and the discard prefix.
        || (ip.segments()[0] == 0x2001 && ip.segments()[1] == 0x0db8)
        || ip.segments()[0] == 0x0100 && ip.segments()[1..4] == [0, 0, 0])
}

/// Whether `ip` is a public unicast address: not loopback, private, link-local, multicast,
/// unspecified, shared (CGNAT), reserved or documentation space.
pub(crate) fn public(ip: IpAddr) -> bool {
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
pub(crate) type Lookup = Arc<
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

struct Resolver {
    policy: AddressPolicy,
    lookup: Lookup,
    /// Fixed addresses (tests only: the fake server's).
    overrides: HashMap<String, SocketAddr>,
}

impl Resolve for Resolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().trim_end_matches('.').to_ascii_lowercase();
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
pub(crate) enum Roots {
    /// The operating system's trust store, through the platform verifier.
    Platform,
    /// Exactly these certificates and nothing else (tests).
    Only(Vec<CertificateDer<'static>>),
}

/// How the system proxy is used.
#[derive(Clone)]
pub(crate) enum ProxyMode {
    /// reqwest's system proxy (environment variables, `NO_PROXY`, OS settings).
    System,
    /// No proxy at all (tests are isolated from the ambient environment).
    Direct,
    /// This proxy (tests).
    Explicit(Box<reqwest::Proxy>),
}

/// Everything the client needs; the shipped values are fixed in code, with no knob.
#[derive(Clone)]
pub(crate) struct Endpoints {
    pub api_base: String,
    pub hosts: Vec<String>,
    pub port: u16,
    pub roots: Roots,
    pub addresses: AddressPolicy,
    pub proxy: ProxyMode,
    pub lookup: Option<Lookup>,
    pub overrides: HashMap<String, SocketAddr>,
}

impl Endpoints {
    /// The shipped values: Hugging Face, the fixed host list, port 443, the OS trust store,
    /// public addresses only, the system proxy.
    pub(crate) fn shipped() -> Self {
        Self {
            api_base: API_BASE.to_owned(),
            hosts: SHIPPED_HOSTS
                .iter()
                .map(|host| (*host).to_owned())
                .collect(),
            port: 443,
            roots: Roots::Platform,
            addresses: AddressPolicy::Public,
            proxy: ProxyMode::System,
            lookup: None,
            overrides: HashMap::new(),
        }
    }
}

/// A rejected redirect destination, recorded by the policy (the request is never sent).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Rejection {
    Host(String),
    Shape,
}

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Follow,
    Refuse(Rejection),
}

/// The normalised host of `url`, if it is a domain name.
fn domain(url: &Url) -> Option<String> {
    match url.host() {
        Some(url::Host::Domain(name)) => Some(name.trim_end_matches('.').to_ascii_lowercase()),
        _ => None,
    }
}

fn judge(url: &Url, hops: usize, hosts: &HashSet<String>, port: u16) -> Verdict {
    if hops > MAX_HOPS
        || url.scheme() != "https"
        || url.port_or_known_default() != Some(port)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Verdict::Refuse(Rejection::Shape);
    }
    match domain(url) {
        Some(host) if hosts.contains(&host) => Verdict::Follow,
        Some(host) => Verdict::Refuse(Rejection::Host(host)),
        None => Verdict::Refuse(Rejection::Shape),
    }
}

/// The client: one for the API (no redirects), one for `resolve` (redirects under the policy).
#[derive(Clone)]
pub(crate) struct Client {
    api: reqwest::Client,
    files: reqwest::Client,
    api_base: Url,
    rejected: Arc<Mutex<Option<Rejection>>>,
}

/// A transfer in progress: the response and what it says of itself.
pub(crate) struct Body {
    response: reqwest::Response,
    pub status: u16,
    pub content_range: Option<String>,
    pub content_length: Option<u64>,
}

impl Body {
    /// The next piece of the body, `None` at the end.
    ///
    /// # Errors
    ///
    /// A transfer failure or an idle read timeout.
    pub(crate) async fn chunk(&mut self) -> Result<Option<Bytes>, Error> {
        self.response.chunk().await.map_err(classify)
    }
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

impl Client {
    /// Builds the two clients over `endpoints`.
    ///
    /// # Errors
    ///
    /// A malformed base URL, a base that is not HTTPS, or TLS that cannot be set up.
    pub(crate) fn new(endpoints: &Endpoints) -> Result<Self, Error> {
        let api_base = Url::parse(&endpoints.api_base)
            .map_err(|_| Error::Setup("the API base URL is malformed"))?;
        if api_base.scheme() != "https"
            || api_base.host_str().is_none()
            || !api_base.username().is_empty()
            || api_base.password().is_some()
        {
            return Err(Error::Setup("the API base URL must be plain HTTPS"));
        }
        let hosts: HashSet<String> = endpoints
            .hosts
            .iter()
            .map(|host| host.trim_end_matches('.').to_ascii_lowercase())
            .collect();
        let rejected = Arc::new(Mutex::new(None));
        let policy = {
            let (hosts, port, rejected) = (hosts, endpoints.port, Arc::clone(&rejected));
            Policy::custom(move |attempt| {
                match judge(attempt.url(), attempt.previous().len(), &hosts, port) {
                    Verdict::Follow => attempt.follow(),
                    Verdict::Refuse(why) => {
                        *rejected
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(why);
                        attempt.stop()
                    }
                }
            })
        };
        let resolver = Arc::new(Resolver {
            policy: endpoints.addresses,
            lookup: endpoints.lookup.clone().unwrap_or_else(system_lookup),
            overrides: endpoints.overrides.clone(),
        });
        let tls = tls_config(&endpoints.roots)?;
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        let build = |redirect: Policy| -> Result<reqwest::Client, Error> {
            let mut builder = reqwest::Client::builder()
                .tls_backend_preconfigured(tls.clone())
                .https_only(true)
                .http1_only()
                .referer(false)
                .redirect(redirect)
                .user_agent(concat!("mimik-model-download/", env!("CARGO_PKG_VERSION")))
                .default_headers(headers.clone())
                .connect_timeout(CONNECT_TIMEOUT)
                .read_timeout(READ_TIMEOUT)
                .pool_max_idle_per_host(0)
                .dns_resolver(Arc::clone(&resolver));
            builder = match &endpoints.proxy {
                ProxyMode::System => builder,
                ProxyMode::Direct => builder.no_proxy(),
                ProxyMode::Explicit(proxy) => builder.no_proxy().proxy((**proxy).clone()),
            };
            builder
                .build()
                .map_err(|_| Error::Setup("the HTTP client cannot be built"))
        };
        Ok(Self {
            api: build(Policy::none())?,
            files: build(policy)?,
            api_base,
            rejected,
        })
    }

    fn url(&self, segments: &[&str], query: Option<&str>) -> Url {
        let mut url = self.api_base.clone();
        if let Ok(mut path) = url.path_segments_mut() {
            path.clear().extend(segments);
        }
        url.set_query(query);
        url
    }

    /// The metadata URL of `spec`'s file at its revision (`blobs=true` lists sizes and LFS).
    pub(crate) fn metadata_url(&self, spec: &HfSpec) -> Url {
        self.url(
            &[
                "api",
                "models",
                &spec.org,
                &spec.repo,
                "revision",
                &spec.revision,
            ],
            Some("blobs=true"),
        )
    }

    /// The `resolve` URL of `path` in `spec`'s repository at its revision.
    pub(crate) fn resolve_url(&self, spec: &HfSpec, path: &str) -> Url {
        let mut segments = vec![
            spec.org.as_str(),
            spec.repo.as_str(),
            "resolve",
            &spec.revision,
        ];
        segments.extend(path.split('/'));
        self.url(&segments, None)
    }

    fn take_rejection(&self) -> Error {
        match self
            .rejected
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            Some(Rejection::Host(host)) => Error::Redirect(host),
            _ => Error::RedirectShape,
        }
    }

    async fn send(
        &self,
        files: bool,
        url: Url,
        range_from: Option<u64>,
    ) -> Result<reqwest::Response, Error> {
        let client = if files { &self.files } else { &self.api };
        *self
            .rejected
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        let mut request = client.get(url);
        if let Some(offset) = range_from {
            request = request.header(RANGE, format!("bytes={offset}-"));
        }
        let response = request.send().await.map_err(classify)?;
        let status = response.status();
        if status.is_redirection() {
            // From the files client: the policy stopped it. From the API client: refused.
            return Err(if files {
                self.take_rejection()
            } else {
                Error::Status(status.as_u16())
            });
        }
        if matches!(status.as_u16(), 401 | 403) {
            return Err(Error::Denied(status.as_u16()));
        }
        if let Some(encoding) = response.headers().get(CONTENT_ENCODING)
            && !encoding.as_bytes().eq_ignore_ascii_case(b"identity")
        {
            return Err(Error::Encoding);
        }
        Ok(response)
    }

    async fn bounded(
        &self,
        files: bool,
        url: Url,
        max: usize,
        deadline: Duration,
    ) -> Result<Bytes, Error> {
        let work = async {
            let mut response = self.send(files, url, None).await?;
            if response.status().as_u16() != 200 {
                return Err(Error::Status(response.status().as_u16()));
            }
            if response
                .content_length()
                .is_some_and(|length| length > max as u64)
            {
                return Err(Error::TooLarge);
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(classify)? {
                if body.len() + chunk.len() > max {
                    return Err(Error::TooLarge);
                }
                body.extend_from_slice(&chunk);
            }
            Ok(Bytes::from(body))
        };
        tokio::time::timeout(deadline, work)
            .await
            .map_err(|_| Error::Timeout)?
    }

    /// The API answer at `url`, at most `max` bytes, within the metadata deadline.
    ///
    /// # Errors
    ///
    /// Any refusal or failure, without a URL.
    pub(crate) async fn api_json(&self, url: Url, max: usize) -> Result<Bytes, Error> {
        self.bounded(false, url, max, METADATA_DEADLINE).await
    }

    /// A small file from `resolve` (the licence), under the redirect policy, within the
    /// licence deadline.
    ///
    /// # Errors
    ///
    /// Any refusal or failure, without a URL.
    pub(crate) async fn small_file(&self, url: Url, max: usize) -> Result<Bytes, Error> {
        self.bounded(true, url, max, LICENCE_DEADLINE).await
    }

    /// Opens the model file's body from `resolve`, from byte `offset` when it is not 0. The
    /// caller checks the status (200 or 206) and `Content-Range`.
    ///
    /// # Errors
    ///
    /// Any refusal or failure, without a URL.
    pub(crate) async fn open(&self, url: Url, offset: u64) -> Result<Body, Error> {
        let response = self.send(true, url, (offset > 0).then_some(offset)).await?;
        let status = response.status().as_u16();
        if status != 200 && status != 206 {
            return Err(Error::Status(status));
        }
        Ok(Body {
            status,
            content_range: response
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
            content_length: response.content_length(),
            response,
        })
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

/// Turns a `reqwest` error into one that never holds a URL.
fn classify(error: reqwest::Error) -> Error {
    let error = error.without_url();
    if error.is_timeout() {
        return Error::Timeout;
    }
    if chain_has::<AddressRefused>(&error) {
        return Error::Address;
    }
    if chain_has::<rustls::Error>(&error) {
        return Error::Tls;
    }
    if error.is_connect() {
        return Error::Connect;
    }
    if error.is_redirect() {
        return Error::RedirectShape;
    }
    Error::Transfer
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
