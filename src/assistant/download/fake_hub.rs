//! A fake Hugging Face for tests: an HTTPS server on loopback with a test CA made at run time,
//! answering by a closure and recording every request (host, path, headers). Hostnames are
//! mapped to it by the client's test overrides, so the real names and the redirect policy are
//! exercised.
use super::client::{AddressPolicy, Endpoints, ProxyMode, Roots, SHIPPED_HOSTS};
use bytes::Bytes;
use futures_util::StreamExt;
use http_body_util::{BodyExt, Full, StreamBody, combinators::BoxBody};
use hyper::{
    Request, Response,
    body::{Frame, Incoming},
    header::{HOST, HeaderMap, HeaderName, HeaderValue},
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::net::TcpListener;

/// What a request looked like.
#[derive(Clone, Debug)]
pub(crate) struct Seen {
    pub host: String,
    pub path: String,
    pub query: String,
    pub headers: HeaderMap,
}

/// How a request is answered.
#[derive(Clone)]
pub(crate) enum Reply {
    /// A complete answer.
    Full {
        status: u16,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
    },
    /// Headers and the first `first` bytes of `body`, then nothing, ever.
    Stall {
        status: u16,
        headers: Vec<(&'static str, String)>,
        first: Vec<u8>,
    },
    /// `body` in `piece`-byte pieces, `gap` apart.
    Slow {
        status: u16,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
        piece: usize,
        gap: Duration,
    },
    /// The request waits this long, then the inner reply is sent.
    Delayed(Duration, Box<Reply>),
}

impl Reply {
    pub(crate) fn json(text: &str) -> Self {
        Self::Full {
            status: 200,
            headers: vec![("content-type", "application/json".into())],
            body: text.as_bytes().to_vec(),
        }
    }

    pub(crate) fn status(status: u16) -> Self {
        Self::Full {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub(crate) fn redirect(location: &str) -> Self {
        Self::Full {
            status: 302,
            headers: vec![("location", location.to_owned())],
            body: Vec::new(),
        }
    }

    pub(crate) fn text(body: &str) -> Self {
        Self::Full {
            status: 200,
            headers: Vec::new(),
            body: body.as_bytes().to_vec(),
        }
    }

    /// `content`, honouring a `Range: bytes=N-` header the way the Hub does.
    pub(crate) fn file(content: &[u8], range: Option<&str>) -> Self {
        let total = content.len();
        if let Some(from) = range
            .and_then(|text| text.strip_prefix("bytes="))
            .and_then(|text| text.strip_suffix('-'))
            .and_then(|text| text.parse::<usize>().ok())
        {
            if from >= total {
                return Self::status(416);
            }
            return Self::Full {
                status: 206,
                headers: vec![(
                    "content-range",
                    format!("bytes {from}-{}/{total}", total - 1),
                )],
                body: content[from..].to_vec(),
            };
        }
        Self::Full {
            status: 200,
            headers: Vec::new(),
            body: content.to_vec(),
        }
    }
}

type Responder = Box<dyn FnMut(&Seen) -> Reply + Send>;

pub(crate) struct FakeHub {
    pub addr: SocketAddr,
    /// The test CA's certificate: the only root the tests' client trusts.
    pub ca: CertificateDer<'static>,
    pub seen: Arc<Mutex<Vec<Seen>>>,
    respond: Arc<Mutex<Responder>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeHub {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Test CA and a server certificate for `names`, as (CA DER, certificate DER, key DER).
pub(crate) fn certificates(
    names: &[&str],
) -> (CertificateDer<'static>, CertificateDer<'static>, Vec<u8>) {
    use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
    let ca_key = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let issuer = rcgen::Issuer::new(ca_params, ca_key);
    let leaf_key = KeyPair::generate().unwrap();
    let leaf_params = CertificateParams::new(
        names
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let leaf = leaf_params.signed_by(&leaf_key, &issuer).unwrap();
    (
        ca.der().clone(),
        leaf.der().clone(),
        leaf_key.serialize_der(),
    )
}

impl FakeHub {
    /// Serves `names` (certificate subject names) answering by `respond`.
    pub(crate) async fn start(
        names: &[&str],
        respond: impl FnMut(&Seen) -> Reply + Send + 'static,
    ) -> Self {
        let (ca, leaf, key) = certificates(names);
        let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![leaf],
            PrivateKeyDer::from(PrivatePkcs8KeyDer::from(key)),
        )
        .unwrap();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let respond: Arc<Mutex<Responder>> = Arc::new(Mutex::new(Box::new(respond)));
        let task = {
            let (seen, respond) = (Arc::clone(&seen), Arc::clone(&respond));
            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        return;
                    };
                    let (acceptor, seen, respond) =
                        (acceptor.clone(), Arc::clone(&seen), Arc::clone(&respond));
                    tokio::spawn(async move {
                        let Ok(tls) = acceptor.accept(stream).await else {
                            return;
                        };
                        let service = service_fn(move |request: Request<Incoming>| {
                            let (seen, respond) = (Arc::clone(&seen), Arc::clone(&respond));
                            async move {
                                Ok::<_, std::convert::Infallible>(
                                    answer(&request, &seen, &respond).await,
                                )
                            }
                        });
                        let _ = hyper::server::conn::http1::Builder::new()
                            .serve_connection(TokioIo::new(tls), service)
                            .await;
                    });
                }
            })
        };
        Self {
            addr,
            ca,
            seen,
            respond,
            task,
        }
    }

    /// Replaces the responder.
    pub(crate) fn respond(&self, respond: impl FnMut(&Seen) -> Reply + Send + 'static) {
        *self.respond.lock().unwrap() = Box::new(respond);
    }

    /// Client endpoints that reach this server under the shipped host names, trusting only its
    /// CA, with no proxy, on its port, and with the address policy off.
    pub(crate) fn endpoints(&self) -> Endpoints {
        let mut overrides = HashMap::new();
        for host in SHIPPED_HOSTS {
            overrides.insert(host.to_owned(), self.addr);
        }
        for host in ["evil.example", "other.example"] {
            overrides.insert(host.to_owned(), self.addr);
        }
        Endpoints {
            api_base: format!("https://huggingface.co:{}", self.addr.port()),
            hosts: SHIPPED_HOSTS
                .iter()
                .map(|host| (*host).to_owned())
                .collect(),
            port: self.addr.port(),
            roots: Roots::Only(vec![self.ca.clone()]),
            addresses: AddressPolicy::Any,
            proxy: ProxyMode::Direct,
            lookup: None,
            overrides,
        }
    }

    pub(crate) fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

fn boxed(bytes: Vec<u8>) -> BoxBody<Bytes, std::io::Error> {
    Full::new(Bytes::from(bytes))
        .map_err(|never| match never {})
        .boxed()
}

fn build(
    status: u16,
    headers: &[(&'static str, String)],
    body: BoxBody<Bytes, std::io::Error>,
) -> Response<BoxBody<Bytes, std::io::Error>> {
    let mut response = Response::builder().status(status);
    for (name, value) in headers {
        response = response.header(
            HeaderName::from_static(name),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    response.body(body).unwrap()
}

async fn answer(
    request: &Request<Incoming>,
    seen: &Arc<Mutex<Vec<Seen>>>,
    respond: &Arc<Mutex<Responder>>,
) -> Response<BoxBody<Bytes, std::io::Error>> {
    let record = Seen {
        host: request
            .headers()
            .get(HOST)
            .and_then(|value| value.to_str().ok())
            .map(|host| host.split(':').next().unwrap_or(host).to_owned())
            .unwrap_or_default(),
        path: request.uri().path().to_owned(),
        query: request.uri().query().unwrap_or_default().to_owned(),
        headers: request.headers().clone(),
    };
    seen.lock().unwrap().push(record.clone());
    let mut reply = (respond.lock().unwrap())(&record);
    if let Reply::Delayed(wait, inner) = reply {
        tokio::time::sleep(wait).await;
        reply = *inner;
    }
    match reply {
        Reply::Full {
            status,
            headers,
            body,
        } => build(status, &headers, boxed(body)),
        Reply::Stall {
            status,
            headers,
            first,
        } => {
            let stream = futures_util::stream::once(async move {
                Ok::<_, std::io::Error>(Frame::data(Bytes::from(first)))
            })
            .chain(futures_util::stream::pending());
            build(status, &headers, BodyExt::boxed(StreamBody::new(stream)))
        }
        Reply::Slow {
            status,
            headers,
            body,
            piece,
            gap,
        } => {
            let pieces: Vec<Vec<u8>> = body.chunks(piece).map(<[u8]>::to_vec).collect();
            let stream = futures_util::stream::iter(pieces).then(move |piece| async move {
                tokio::time::sleep(gap).await;
                Ok::<_, std::io::Error>(Frame::data(Bytes::from(piece)))
            });
            build(status, &headers, BodyExt::boxed(StreamBody::new(stream)))
        }
        Reply::Delayed(..) => unreachable!("delays do not nest"),
    }
}
