//! The download client (`docs/model-downloads.md`, DEP-27): a `reqwest` client separate from
//! the model client, HTTPS only, port 443 only, exact host list checked before every redirect
//! hop is sent, a resolver that refuses local addresses, the system proxy for downloads only,
//! no cookies, no `Referer`, identity encoding only, and errors that never carry a URL.
//! The TLS config, the resolver, the trust roots, the proxy modes, the public-address policy
//! and the URL-free error classification are shared with the API reviewer in
//! [`super::https`], which also holds the reviewer's own client.
use crate::assistant::model::HfSpec;
use bytes::Bytes;
use reqwest::{
    header::{CONTENT_ENCODING, CONTENT_RANGE, LOCATION, RANGE},
    redirect::Policy,
};
#[cfg(test)]
use std::{collections::HashMap, net::SocketAddr};
use std::{collections::HashSet, sync::Arc, time::Duration};
use url::Url;

// Shared with `super::https` (re-exported so this module's tests keep their `use super::*`).
#[cfg(test)]
pub(crate) use super::https::public;
pub(crate) use super::https::{AddressPolicy, Lookup, ProxyMode, Roots};

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

/// Exact host names a `resolve` redirect may reach (no suffix or wildcard rule): the hosts the
/// real GET chain of both pinned files showed on 2026-10-29 (`huggingface.co`, one 302 to
/// `us.aws.cdn.hf.co`, then 206), plus the hosts Hugging Face documents for LFS and Xet
/// storage. A host outside the list is refused by name, and the list is then updated by hand.
/// `tests::the_shipped_host_list_is_the_reviewed_one` is a tripwire: changing the list is a
/// deliberate edit of both.
pub(crate) const SHIPPED_HOSTS: [&str; 10] = [
    "huggingface.co",
    "us.aws.cdn.hf.co",
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
    /// Test-only (the fake hub's addresses); empty in the shipped values.
    #[cfg(test)]
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
            #[cfg(test)]
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
    /// The exact redirect hosts and the one port, to name why a redirect was not followed.
    hosts: Arc<HashSet<String>>,
    port: u16,
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
        let hosts = Arc::new(hosts);
        let policy = {
            let (hosts, port) = (Arc::clone(&hosts), endpoints.port);
            Policy::custom(move |attempt| {
                match judge(attempt.url(), attempt.previous().len(), &hosts, port) {
                    Verdict::Follow => attempt.follow(),
                    // Not followed: the caller sees the 3xx and says why from its Location.
                    Verdict::Refuse(_) => attempt.stop(),
                }
            })
        };
        let transport = super::https::Transport {
            roots: endpoints.roots.clone(),
            addresses: endpoints.addresses,
            lookup: endpoints.lookup.clone(),
            #[cfg(test)]
            overrides: endpoints.overrides.clone(),
            proxy: endpoints.proxy.clone(),
            connect_timeout: CONNECT_TIMEOUT,
            read_timeout: Some(READ_TIMEOUT),
            user_agent: concat!("mimik-model-download/", env!("CARGO_PKG_VERSION")).to_owned(),
        };
        let build = |redirect: Policy| -> Result<reqwest::Client, Error> {
            transport.build(redirect).map_err(|error| match error {
                super::https::Error::NoTrustStore => Error::NoTrustStore,
                super::https::Error::Setup(reason) => Error::Setup(reason),
                _ => Error::Setup("the HTTP client cannot be built"),
            })
        };
        Ok(Self {
            api: build(Policy::none())?,
            files: build(policy)?,
            api_base,
            hosts,
            port: endpoints.port,
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

    /// Why a 3xx answer was not followed, from the answer itself (nothing is shared between
    /// requests): a followed kind of redirect with a destination the policy refuses names the
    /// host or the shape; any other 3xx is just its status.
    fn refused_redirect(&self, response: &reqwest::Response) -> Error {
        let status = response.status().as_u16();
        let target = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|location| response.url().join(location).ok());
        match (matches!(status, 301 | 302 | 303 | 307 | 308), target) {
            (true, Some(target)) => match judge(&target, 0, &self.hosts, self.port) {
                Verdict::Refuse(Rejection::Host(host)) => Error::Redirect(host),
                // Acceptable in itself: it was the third hop that was refused.
                Verdict::Refuse(Rejection::Shape) | Verdict::Follow => Error::RedirectShape,
            },
            _ => Error::Status(status),
        }
    }

    async fn send(
        &self,
        files: bool,
        url: Url,
        range_from: Option<u64>,
    ) -> Result<reqwest::Response, Error> {
        let client = if files { &self.files } else { &self.api };
        let mut request = client.get(url);
        if let Some(offset) = range_from {
            request = request.header(RANGE, format!("bytes={offset}-"));
        }
        let response = request.send().await.map_err(classify)?;
        let status = response.status();
        if status.is_redirection() {
            // From the files client: the policy stopped it. From the API client: refused.
            return Err(if files {
                self.refused_redirect(&response)
            } else {
                Error::Status(status.as_u16())
            });
        }
        if matches!(status.as_u16(), 401 | 403) {
            return Err(Error::Denied(status.as_u16()));
        }
        // Every value of every Content-Encoding header must be `identity`.
        for value in response.headers().get_all(CONTENT_ENCODING) {
            let text = value.to_str().map_err(|_| Error::Encoding)?;
            if text
                .split(',')
                .any(|token| !token.trim().eq_ignore_ascii_case("identity"))
            {
                return Err(Error::Encoding);
            }
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

/// Turns a `reqwest` error into one that never holds a URL, through the shared classification.
fn classify(error: reqwest::Error) -> Error {
    match super::https::classify(error) {
        super::https::Failure::Timeout => Error::Timeout,
        super::https::Failure::Address => Error::Address,
        super::https::Failure::Tls => Error::Tls,
        super::https::Failure::Connect => Error::Connect,
        super::https::Failure::Redirect => Error::RedirectShape,
        super::https::Failure::Transfer => Error::Transfer,
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "client_batch_tests.rs"]
mod batch_tests;
