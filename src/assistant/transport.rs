//! The model client's transport and its policy (assisted sheet plan §1.4, "Crate" and
//! "Client policy"): one `reqwest` client per endpoint, no proxies whatever the environment
//! says, no redirects, no retries, no decompression, plain HTTP/1 only, a fresh connection
//! per request, and response bodies refused past [`BODY_LIMIT`], plain and streaming,
//! without buffering them. It implements rig's `HttpClientExt`, so rig-core builds the
//! requests and Mimik keeps the wire.
use bytes::Bytes;
use futures_util::StreamExt;
use rig_core::http_client::{self, HttpClientExt, LazyBody, MultipartForm, StreamingResponse};
use std::path::{Path, PathBuf};

/// The most bytes of a response body, plain or streamed (as probed in 2b).
pub(crate) const BODY_LIMIT: usize = 1024 * 1024;

/// Where the model server listens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Endpoint {
    /// A loopback IP literal and port, as `http://<host>:<port>` (checked by [`tcp`]).
    Tcp(url::Url),
    /// A Unix socket path; requests name the fixed authority [`UNIX_AUTHORITY`].
    Unix(PathBuf),
}

/// The authority of request URLs on the Unix-socket connector. The connector ignores it:
/// every connection goes to the configured path and nowhere else.
pub(crate) const UNIX_AUTHORITY: &str = "localhost";

/// Why an endpoint URL is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    NotHttp,
    Userinfo,
    Host,
    Port,
    Path,
}

/// Checks a TCP endpoint URL (plan §1.4, "Client policy"): `http` only; the host an IP
/// literal equal to `127.0.0.1` (or `[::1]` when `ipv6_loopback`, the 2c-1 development
/// endpoint only), with no userinfo or zone ID; an explicit port; no path, query or
/// fragment. The raw authority is checked as typed as well as the parsed URL, because the
/// WHATWG parser turns `127.1`, `0x7f.1` or `2130706433` into `127.0.0.1`.
pub(crate) fn tcp(raw: &str, ipv6_loopback: bool) -> Result<url::Url, Refused> {
    let rest = raw.strip_prefix("http://").ok_or(Refused::NotHttp)?;
    let (authority, path) = rest.find('/').map_or((rest, ""), |at| rest.split_at(at));
    if !matches!(path, "" | "/") {
        return Err(Refused::Path);
    }
    if authority.contains('@') {
        return Err(Refused::Userinfo);
    }
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (host, port) = v6.split_once(']').ok_or(Refused::Host)?;
        if host != "::1" || !ipv6_loopback {
            return Err(Refused::Host);
        }
        (host, port.strip_prefix(':').ok_or(Refused::Port)?)
    } else {
        let (host, port) = authority.split_once(':').ok_or(Refused::Port)?;
        if host != "127.0.0.1" {
            return Err(Refused::Host);
        }
        (host, port)
    };
    let digits = !port.is_empty() && port.len() <= 5 && port.bytes().all(|b| b.is_ascii_digit());
    if !digits || port.starts_with('0') || port.parse::<u16>().is_err() {
        return Err(Refused::Port);
    }
    let url = url::Url::parse(raw).map_err(|_| Refused::Host)?;
    let parsed_host = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.to_string(),
        Some(url::Host::Ipv6(ip)) => ip.to_string(),
        _ => return Err(Refused::Host),
    };
    if parsed_host != host
        || url.port().map(|p| p.to_string()).as_deref() != Some(port)
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(Refused::Host);
    }
    Ok(url)
}

/// The policy client for `endpoint`.
///
/// # Errors
///
/// Returns the builder's error (it builds no TLS, so none is expected).
pub(crate) fn policy_client(endpoint: &Endpoint) -> Result<reqwest::Client, reqwest::Error> {
    let builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .http1_only()
        .https_only(false)
        .pool_max_idle_per_host(0);
    match endpoint {
        Endpoint::Tcp(_) => builder.build(),
        Endpoint::Unix(path) => unix(builder, path).build(),
    }
}

#[cfg(unix)]
fn unix(builder: reqwest::ClientBuilder, path: &Path) -> reqwest::ClientBuilder {
    builder.unix_socket(path)
}

#[cfg(not(unix))]
fn unix(builder: reqwest::ClientBuilder, _: &Path) -> reqwest::ClientBuilder {
    builder
}

/// A response body past the bound.
#[derive(Debug)]
pub(crate) struct OverBound;

impl std::fmt::Display for OverBound {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("response body over the bound")
    }
}

impl std::error::Error for OverBound {}

/// A request to a destination other than the endpoint.
#[derive(Debug)]
pub(crate) struct Elsewhere;

impl std::fmt::Display for Elsewhere {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("request for another destination than the model endpoint")
    }
}

impl std::error::Error for Elsewhere {}

/// rig's transport: the policy client bound to one endpoint.
#[derive(Clone, Debug)]
pub(crate) struct Bounded {
    client: reqwest::Client,
    endpoint: Endpoint,
}

impl Bounded {
    pub(crate) fn new(endpoint: Endpoint) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: policy_client(&endpoint)?,
            endpoint,
        })
    }

    /// The base URL rig and the tokenizer build request URLs from (no trailing slash).
    pub(crate) fn base(&self) -> String {
        match &self.endpoint {
            Endpoint::Tcp(url) => url.as_str().trim_end_matches('/').to_owned(),
            Endpoint::Unix(_) => format!("http://{UNIX_AUTHORITY}"),
        }
    }

    /// Whether a request URI goes to the endpoint: the same scheme, host and port on TCP,
    /// the fixed authority on the Unix socket.
    fn destined(&self, uri: &http::Uri) -> bool {
        if uri.scheme_str() != Some("http") {
            return false;
        }
        match &self.endpoint {
            Endpoint::Tcp(url) => {
                let authority = uri.authority().map(http::uri::Authority::as_str);
                let expected = format!(
                    "{}:{}",
                    url.host_str().unwrap_or_default(),
                    url.port().unwrap_or_default()
                );
                authority == Some(expected.as_str())
            }
            Endpoint::Unix(_) => {
                uri.authority().map(http::uri::Authority::as_str) == Some(UNIX_AUTHORITY)
            }
        }
    }

    fn builder<T: Into<Bytes>>(
        &self,
        request: http::Request<T>,
    ) -> Result<reqwest::RequestBuilder, Elsewhere> {
        let (parts, body) = request.into_parts();
        if !self.destined(&parts.uri) {
            return Err(Elsewhere);
        }
        Ok(self
            .client
            .request(parts.method, parts.uri.to_string())
            .headers(parts.headers)
            .body(body.into()))
    }
}

/// Why a body was not read whole.
enum BodyError {
    Transport(reqwest::Error),
    Over,
}

impl BodyError {
    fn into_http(self) -> http_client::Error {
        match self {
            Self::Transport(error) => http_client::Error::instance(error),
            Self::Over => http_client::Error::instance(OverBound),
        }
    }
}

/// Reads a whole body, refusing it as soon as it passes `limit`.
async fn bounded_body(response: reqwest::Response, limit: usize) -> Result<Bytes, BodyError> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(BodyError::Transport)?;
        if body.len() + chunk.len() > limit {
            return Err(BodyError::Over);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(body))
}

fn rebuild<B>(
    status: http::StatusCode,
    headers: http::HeaderMap,
    body: B,
) -> Result<http::Response<B>, http::Error> {
    let mut out = http::Response::builder().status(status).body(body)?;
    *out.headers_mut() = headers;
    Ok(out)
}

impl HttpClientExt for Bounded {
    fn send<T, U>(
        &self,
        request: http::Request<T>,
    ) -> impl Future<Output = http_client::Result<http::Response<LazyBody<U>>>> + Send + 'static
    where
        T: Into<Bytes> + Send,
        U: From<Bytes> + Send + 'static,
    {
        let builder = self.builder(request);
        async move {
            let response = builder
                .map_err(http_client::Error::instance)?
                .send()
                .await
                .map_err(http_client::Error::instance)?;
            let status = response.status();
            let headers = response.headers().clone();
            if !status.is_success() {
                // The error body is never read: it could only be logged, and nothing is.
                return Err(http_client::Error::non_success_with_details(
                    status,
                    headers,
                    String::new(),
                ));
            }
            let body = bounded_body(response, BODY_LIMIT)
                .await
                .map_err(BodyError::into_http)?;
            let lazy: LazyBody<U> = Box::pin(async move { Ok(U::from(body)) });
            rebuild(status, headers, lazy).map_err(http_client::Error::Protocol)
        }
    }

    fn send_multipart<U>(
        &self,
        _request: http::Request<MultipartForm>,
    ) -> impl Future<Output = http_client::Result<http::Response<LazyBody<U>>>> + Send + 'static
    where
        U: From<Bytes> + Send + 'static,
    {
        std::future::ready(Err(http_client::Error::instance(Elsewhere)))
    }

    fn send_streaming<T>(
        &self,
        request: http::Request<T>,
    ) -> impl Future<Output = http_client::Result<StreamingResponse>> + Send
    where
        T: Into<Bytes> + Send,
    {
        let builder = self.builder(request);
        async move {
            let response = builder
                .map_err(http_client::Error::instance)?
                .send()
                .await
                .map_err(http_client::Error::instance)?;
            let status = response.status();
            let headers = response.headers().clone();
            if !status.is_success() {
                return Err(http_client::Error::non_success_with_details(
                    status,
                    headers,
                    String::new(),
                ));
            }
            let mut seen = 0_usize;
            #[expect(
                clippy::result_large_err,
                reason = "the item type is rig's `http_client::Result`"
            )]
            let stream = response.bytes_stream().map(move |chunk| {
                let chunk = chunk.map_err(http_client::Error::instance)?;
                seen = seen.saturating_add(chunk.len());
                if seen > BODY_LIMIT {
                    return Err(http_client::Error::instance(OverBound));
                }
                Ok(chunk)
            });
            let boxed: http_client::BoxedStream = Box::pin(stream);
            rebuild(status, headers, boxed).map_err(http_client::Error::Protocol)
        }
    }
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
