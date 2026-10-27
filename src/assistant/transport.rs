//! The model client's transport and its policy (assisted sheet plan §1.4, "Crate" and
//! "Client policy"; §1.4a, "Endpoint"): one HTTP/1 exchange per connection, over a stream
//! Mimik opened itself and checked before its first byte, and response bodies refused past
//! [`BODY_LIMIT`], plain and streaming, without buffering them. It implements rig's
//! `HttpClientExt`, so rig-core builds the requests and Mimik keeps the wire.
//!
//! The exchange is `hyper`'s client connection on that stream (2c-2a): there is no pool, no
//! proxy, no redirect, no retry and no decompression to turn off, and every connection goes
//! through a [`Connect`] that the runtime gives the endpoint check (`reqwest`'s Unix-socket
//! connector has no per-connection hook that could see the socket). Plain HTTP only: no TLS
//! code is reachable from here.
use bytes::Bytes;
use futures_util::StreamExt;
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use rig_core::http_client::{self, HttpClientExt, LazyBody, MultipartForm, StreamingResponse};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// The most bytes of a response body, plain or streamed (as probed in 2b).
pub(crate) const BODY_LIMIT: usize = 1024 * 1024;

/// Where the development endpoint listens (2c-1; no endpoint check).
#[cfg(any(test, feature = "dev-endpoint"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Endpoint {
    /// A loopback IP literal and port, as `http://<host>:<port>` (checked by [`tcp`]).
    Tcp(url::Url),
    /// A Unix socket path; requests name the fixed authority [`UNIX_AUTHORITY`].
    Unix(std::path::PathBuf),
}

/// The authority of request URLs on a Unix socket. It is only a name: every connection goes
/// to the connector's socket and nowhere else.
pub(crate) const UNIX_AUTHORITY: &str = "localhost";

/// Why an endpoint URL is refused.
#[cfg(any(test, feature = "dev-endpoint"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    NotHttp,
    Userinfo,
    Host,
    Port,
    Path,
}

#[cfg(feature = "dev-endpoint")]
impl Refused {
    /// The reason, as text for the operator.
    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::NotHttp => "the URL must start with http://",
            Self::Userinfo => "the URL must not carry a user name or password",
            Self::Host => "the host must be 127.0.0.1 (or [::1])",
            Self::Port => "the URL needs an explicit port from 1 to 65535",
            Self::Path => "the URL must not have a path, query or fragment",
        }
    }
}

/// Checks a TCP endpoint URL (plan §1.4, "Client policy"): `http` only; the host an IP
/// literal equal to `127.0.0.1` (or `[::1]` when `ipv6_loopback`, the 2c-1 development
/// endpoint only), with no userinfo or zone ID; an explicit port; no path, query or
/// fragment. The raw authority is checked as typed as well as the parsed URL, because the
/// WHATWG parser turns `127.1`, `0x7f.1` or `2130706433` into `127.0.0.1`.
#[cfg(any(test, feature = "dev-endpoint"))]
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

/// A connected stream to the model server.
pub(crate) enum Stream {
    Tcp(tokio::net::TcpStream),
    Unix(tokio::net::UnixStream),
}

impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(stream) => Pin::new(stream).poll_read(cx, buf),
            Self::Unix(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Self::Tcp(stream) => Pin::new(stream).poll_write(cx, buf),
            Self::Unix(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(stream) => Pin::new(stream).poll_flush(cx),
            Self::Unix(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(stream) => Pin::new(stream).poll_shutdown(cx),
            Self::Unix(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}

/// A boxed connection attempt.
pub(crate) type Connecting = Pin<Box<dyn Future<Output = std::io::Result<Stream>> + Send>>;

/// Opens one connection to the model server. The runtime's connectors check, before the
/// first byte, that it reaches the current child (§1.4a, "Endpoint check").
pub(crate) trait Connect: Send + Sync + std::fmt::Debug {
    fn connect(&self) -> Connecting;
    /// The authority request URLs carry (`127.0.0.1:<port>`, or [`UNIX_AUTHORITY`]).
    fn authority(&self) -> String;
}

/// The development endpoint's connector: straight to the address or the socket path, no
/// check (2c-1, `dev-endpoint` builds and tests only).
#[cfg(any(test, feature = "dev-endpoint"))]
#[derive(Debug)]
pub(crate) struct Direct(pub Endpoint);

#[cfg(any(test, feature = "dev-endpoint"))]
impl Connect for Direct {
    fn connect(&self) -> Connecting {
        let endpoint = self.0.clone();
        Box::pin(async move {
            match endpoint {
                Endpoint::Tcp(url) => {
                    let host = url.host_str().unwrap_or_default().trim_matches(['[', ']']);
                    let ip: std::net::IpAddr = host
                        .parse()
                        .map_err(|_| std::io::Error::other("not an IP literal"))?;
                    let port = url.port().ok_or_else(|| std::io::Error::other("no port"))?;
                    tokio::net::TcpStream::connect((ip, port))
                        .await
                        .map(Stream::Tcp)
                }
                Endpoint::Unix(path) => tokio::net::UnixStream::connect(path)
                    .await
                    .map(Stream::Unix),
            }
        })
    }

    fn authority(&self) -> String {
        match &self.0 {
            Endpoint::Tcp(url) => format!(
                "{}:{}",
                url.host_str().unwrap_or_default(),
                url.port().unwrap_or_default()
            ),
            Endpoint::Unix(_) => UNIX_AUTHORITY.to_owned(),
        }
    }
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

/// rig's transport: one connector, one exchange per connection.
#[derive(Clone, Debug)]
pub(crate) struct Bounded {
    connector: Arc<dyn Connect>,
    authority: String,
}

/// Aborts the connection's task when dropped, which closes the socket: a dropped response
/// (a superseded or expired job) closes its connection at once.
struct Driver(tokio::task::AbortHandle);

impl Drop for Driver {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Bounded {
    pub(crate) fn new(connector: Arc<dyn Connect>) -> Self {
        let authority = connector.authority();
        Self {
            connector,
            authority,
        }
    }

    /// The development endpoint's transport.
    #[cfg(any(test, feature = "dev-endpoint"))]
    pub(crate) fn direct(endpoint: Endpoint) -> Self {
        Self::new(Arc::new(Direct(endpoint)))
    }

    /// The base URL rig and the tokenizer build request URLs from (no trailing slash).
    pub(crate) fn base(&self) -> String {
        format!("http://{}", self.authority)
    }

    /// Whether a request URI goes to the endpoint: `http` and the connector's authority.
    fn destined(&self, uri: &http::Uri) -> bool {
        uri.scheme_str() == Some("http")
            && uri.authority().map(http::uri::Authority::as_str) == Some(self.authority.as_str())
    }

    /// Checks the destination, connects, and sends one request in origin form with its `Host`.
    #[expect(
        clippy::result_large_err,
        reason = "the error type is rig's `http_client::Error`"
    )]
    async fn exchange(
        &self,
        request: http::Request<Bytes>,
    ) -> http_client::Result<(http::Response<hyper::body::Incoming>, Driver)> {
        let (mut parts, body) = request.into_parts();
        if !self.destined(&parts.uri) {
            return Err(http_client::Error::instance(Elsewhere));
        }
        let target = parts
            .uri
            .path_and_query()
            .map_or("/", http::uri::PathAndQuery::as_str)
            .to_owned();
        parts.uri = target
            .parse()
            .map_err(|error: http::uri::InvalidUri| http_client::Error::Protocol(error.into()))?;
        let host = http::HeaderValue::from_str(&self.authority)
            .map_err(|error| http_client::Error::Protocol(error.into()))?;
        parts.headers.insert(http::header::HOST, host);
        let request = http::Request::from_parts(parts, Full::new(body));
        let stream = self
            .connector
            .connect()
            .await
            .map_err(http_client::Error::instance)?;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(http_client::Error::instance)?;
        let driver = Driver(tokio::spawn(connection).abort_handle());
        let response = sender
            .send_request(request)
            .await
            .map_err(http_client::Error::instance)?;
        Ok((response, driver))
    }
}

/// Why a body was not read whole.
enum BodyError {
    Transport(hyper::Error),
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
async fn bounded_body(mut body: hyper::body::Incoming, limit: usize) -> Result<Bytes, BodyError> {
    let mut out = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(BodyError::Transport)?;
        if let Ok(chunk) = frame.into_data() {
            if out.len() + chunk.len() > limit {
                return Err(BodyError::Over);
            }
            out.extend_from_slice(&chunk);
        }
    }
    Ok(Bytes::from(out))
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

fn owned<T: Into<Bytes>>(request: http::Request<T>) -> http::Request<Bytes> {
    let (parts, body) = request.into_parts();
    http::Request::from_parts(parts, body.into())
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
        let this = self.clone();
        let request = owned(request);
        async move {
            let (response, driver) = this.exchange(request).await?;
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
            let body = bounded_body(response.into_body(), BODY_LIMIT)
                .await
                .map_err(BodyError::into_http)?;
            drop(driver);
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
        let this = self.clone();
        let request = owned(request);
        async move {
            let (response, driver) = this.exchange(request).await?;
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
            let stream = response.into_body().into_data_stream().map(move |chunk| {
                // The driver lives as long as the stream: dropping the stream closes the
                // connection.
                let _ = &driver;
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
