//! Direct trusted IPv4 LAN admission and canonical sharing address.
use crate::AppError;
use axum::{
    extract::{ConnectInfo, Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use url::Url;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ExpectedOrigin {
    pub value: String,
    pub secure: bool,
}
#[derive(Clone)]
pub(crate) struct Access {
    network: Network,
    external: Option<Url>,
}

/// Parse the canonical advertised HTTP(S) origin without hidden credentials/paths.
/// # Errors
/// Rejects non-origin URLs and conflicting direct HTTP / external HTTPS authorities.
pub fn external_url(value: &str, network: Network) -> Result<Url, AppError> {
    let url = Url::parse(value)
        .map_err(|_| AppError::Invalid("external_url must be an HTTP(S) origin."))?;
    validate_url(&url)?;
    let origin = url.origin().ascii_serialization();
    let authority = origin
        .split_once("://")
        .ok_or(AppError::Invalid("external_url origin is invalid."))?
        .1;
    if url.scheme() == "https" && network.accepts_host(authority) {
        return Err(AppError::Invalid(
            "HTTPS external_url must use a distinct authority from the direct HTTP listener.",
        ));
    }
    Ok(url)
}
fn validate_url(url: &Url) -> Result<(), AppError> {
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(AppError::Invalid(
            "external_url must be an HTTP(S) origin without credentials, path prefix, query or fragment.",
        ));
    }
    if url.port() == Some(0) || url.origin().ascii_serialization().len() > 320 {
        return Err(AppError::Invalid(
            "external_url origin is too long or has an invalid port.",
        ));
    }
    Ok(())
}
impl Access {
    pub(crate) fn new(network: Network, external: Option<Url>) -> Result<Self, AppError> {
        network.validate()?;
        if let Some(url) = &external {
            external_url(url.as_str(), network)?;
        }
        Ok(Self { network, external })
    }
    fn origin_for(&self, host: &str) -> Option<ExpectedOrigin> {
        if let Some(external) = &self.external {
            let candidate = Url::parse(&format!("{}://{host}", external.scheme())).ok()?;
            if validate_url(&candidate).is_ok() && candidate.origin() == external.origin() {
                return Some(ExpectedOrigin {
                    value: external.origin().ascii_serialization(),
                    secure: external.scheme() == "https",
                });
            }
        }
        if self.network.accepts_host(host) {
            let url = Url::parse(&format!("http://{host}")).ok()?;
            return Some(ExpectedOrigin {
                value: url.origin().ascii_serialization(),
                secure: false,
            });
        }
        None
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub enum Network {
    #[default]
    Loopback,
    Lan {
        address: Ipv4Addr,
        netmask: Ipv4Addr,
    },
}
impl Network {
    /// Select an enumerated private interface, preferring the kernel route.
    /// # Errors
    /// Rejects missing, invalid or ambiguous interfaces; never picks by list order.
    pub fn discover(selected: Option<Ipv4Addr>) -> Result<Self, Box<dyn std::error::Error>> {
        let mut candidates = if_addrs::get_if_addrs()?
            .into_iter()
            .filter_map(|interface| match interface.addr {
                if_addrs::IfAddr::V4(addr) if addr.ip.is_private() => Some((addr.ip, addr.netmask)),
                _ => None,
            })
            .filter(|(address, netmask)| {
                Self::Lan {
                    address: *address,
                    netmask: *netmask,
                }
                .validate()
                .is_ok()
            })
            .collect::<Vec<_>>();
        candidates.sort_unstable();
        candidates.dedup();
        let preferred = selected.or_else(|| {
            let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
            // UDP connect selects a local route; no datagram is sent.
            socket.connect((Ipv4Addr::new(192, 0, 2, 1), 80)).ok()?;
            match socket.local_addr().ok()?.ip() {
                IpAddr::V4(ip) => Some(ip),
                IpAddr::V6(_) => None,
            }
        });
        select(&candidates, selected, preferred).map_err(Into::into)
    }
    pub(crate) fn validate(self) -> Result<(), AppError> {
        if let Self::Lan { address, netmask } = self {
            let mask = u32::from(netmask);
            let prefix = mask.leading_ones();
            if !address.is_private()
                || prefix == 0
                || mask != u32::MAX.checked_shl(32 - prefix).unwrap_or(0)
            {
                return Err(AppError::Invalid(
                    "LAN address must be private with a contiguous local interface netmask.",
                ));
            }
        }
        Ok(())
    }
    #[must_use]
    pub fn origin(self) -> String {
        match self {
            Self::Loopback => "http://127.0.0.1:3000".into(),
            Self::Lan { address, .. } => format!("http://{address}:3000"),
        }
    }
    #[must_use]
    pub fn bind_address(self) -> SocketAddr {
        SocketAddr::from((
            match self {
                Self::Loopback => Ipv4Addr::LOCALHOST,
                Self::Lan { .. } => Ipv4Addr::UNSPECIFIED,
            },
            3000,
        ))
    }
    fn accepts_peer(self, peer: IpAddr) -> bool {
        if peer.is_loopback() {
            return true;
        }
        match (self, peer) {
            (Self::Lan { address, netmask }, IpAddr::V4(peer)) => {
                peer.is_private()
                    && u32::from(peer) & u32::from(netmask)
                        == u32::from(address) & u32::from(netmask)
            }
            _ => false,
        }
    }
    fn accepts_host(self, host: &str) -> bool {
        host.eq_ignore_ascii_case("127.0.0.1:3000")
            || host.eq_ignore_ascii_case("localhost:3000")
            || matches!(self,Self::Lan{address,..} if host==format!("{address}:3000"))
    }
}
fn select(
    candidates: &[(Ipv4Addr, Ipv4Addr)],
    explicit: Option<Ipv4Addr>,
    preferred: Option<Ipv4Addr>,
) -> Result<Network, AppError> {
    let matches = if let Some(ip) = preferred {
        candidates
            .iter()
            .copied()
            .filter(|(address, _)| *address == ip)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let choice = if matches.len() == 1 {
        matches[0]
    } else if explicit.is_none() && candidates.len() == 1 {
        candidates[0]
    } else {
        return Err(AppError::Invalid(
            "Choose a local private --lan-address, or use --loopback for development.",
        ));
    };
    Ok(Network::Lan {
        address: choice.0,
        netmask: choice.1,
    })
}
pub(crate) async fn admission(
    State(access): State<Access>,
    mut request: Request,
    next: Next,
) -> Response {
    let network = access.network;
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip());
    if peer.is_some_and(|peer| !network.accepts_peer(peer))
        || (peer.is_none() && matches!(network, Network::Lan { .. }))
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut hosts = request.headers().get_all(header::HOST).iter();
    let host = hosts.next().and_then(|host| host.to_str().ok());
    let origin = host.and_then(|host| access.origin_for(host));
    if hosts.next().is_some()
        || origin.is_none()
        || request
            .uri()
            .authority()
            .is_some_and(|authority| access.origin_for(authority.as_str()) != origin)
        || request.uri().scheme_str().is_some_and(|scheme| {
            !origin
                .as_ref()
                .is_some_and(|origin| origin.value.starts_with(&format!("{scheme}://")))
        })
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let origin = origin.expect("checked origin");
    request.extensions_mut().insert(origin.clone());
    let response = next.run(request).await;
    if origin.secure {
        secure_cookies(response)
    } else {
        response
    }
}

fn secure_cookies(mut response: Response) -> Response {
    let mut cookies = Vec::new();
    for value in response.headers().get_all(header::SET_COOKIE) {
        let Ok(text) = value.to_str() else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        let Ok(mut cookie) = tower_sessions::cookie::Cookie::parse(text.to_owned()) else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        cookie.set_secure(true);
        let Ok(value) = cookie.to_string().parse() else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        cookies.push(value);
    }
    response.headers_mut().remove(header::SET_COOKIE);
    for value in cookies {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn external_origins_normalize_and_reject_ambiguous_or_nonorigin_values() {
        let network = Network::Loopback;
        for value in [
            "ftp://mimik.example",
            "https://dummy:dummy@mimik.example",
            "https://mimik.example/app",
            "https://mimik.example/?token=x",
            "https://mimik.example/#token=x",
            "https://localhost:3000",
        ] {
            assert!(external_url(value, network).is_err());
        }
        let url = external_url("https://MIMIK.example:443/", network).unwrap();
        assert_eq!(url.origin().ascii_serialization(), "https://mimik.example");
        let access = Access::new(network, Some(url)).unwrap();
        for authority in ["mimik.example", "mimik.example:443", "MIMIK.EXAMPLE"] {
            let origin = access.origin_for(authority).unwrap();
            assert!(origin.secure);
            assert_eq!(origin.value, "https://mimik.example");
        }
        assert!(!access.origin_for("localhost:3000").unwrap().secure);
        assert!(access.origin_for("attacker.example").is_none());
        assert_eq!(
            external_url("https://[fd00::1]:8443", network)
                .unwrap()
                .origin()
                .ascii_serialization(),
            "https://[fd00::1]:8443"
        );
    }
    #[test]
    fn https_cookie_policy_preserves_issuance_and_deletion_headers_and_fails_closed() {
        let response = axum::http::Response::builder()
            .status(303)
            .header(
                header::SET_COOKIE,
                "mimik_session=dummy; Path=/; HttpOnly; SameSite=Strict",
            )
            .header(
                header::SET_COOKIE,
                "mimik_session=; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT; SameSite=Strict",
            )
            .body(axum::body::Body::empty())
            .unwrap();
        let response = secure_cookies(response);
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let values = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| tower_sessions::cookie::Cookie::parse(value.to_str().unwrap()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(values.len(), 2);
        for cookie in &values {
            assert_eq!(cookie.secure(), Some(true));
            assert_eq!(cookie.path(), Some("/"));
            assert_eq!(
                cookie.same_site(),
                Some(tower_sessions::cookie::SameSite::Strict)
            );
        }
        assert_eq!(values[0].http_only(), Some(true));
        assert_eq!(values[1].value(), "");
        assert!(values[1].expires().is_some());
        let bad = axum::http::Response::builder()
            .header(header::SET_COOKIE, "broken")
            .body(axum::body::Body::empty())
            .unwrap();
        let response = secure_cookies(bad);
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!response.headers().contains_key(header::SET_COOKIE));
    }
    #[test]
    fn interface_selection_and_peer_policy_are_deterministic_and_private() {
        let ip = Ipv4Addr::new(192, 168, 1, 4);
        let mask = Ipv4Addr::new(255, 255, 255, 0);
        let network = select(&[(ip, mask)], None, None).unwrap();
        assert!(network.accepts_peer(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 5))));
        for peer in ["192.168.2.5", "8.8.8.8", "::1", "2001:db8::1"] {
            assert_eq!(network.accepts_peer(peer.parse().unwrap()), peer == "::1");
        }
        assert!(
            select(
                &[(ip, mask), (Ipv4Addr::new(10, 0, 0, 1), mask)],
                None,
                None
            )
            .is_err()
        );
        assert!(select(&[(ip, mask)], Some(Ipv4Addr::new(192, 168, 1, 9)), None).is_err());
        assert!(
            Network::Lan {
                address: ip,
                netmask: Ipv4Addr::new(255, 0, 255, 0)
            }
            .validate()
            .is_err()
        );
        assert!(
            Network::Lan {
                address: Ipv4Addr::new(8, 8, 8, 8),
                netmask: mask
            }
            .validate()
            .is_err()
        );
    }
    #[tokio::test]
    async fn denied_peers_hosts_and_absolute_uri_do_not_reach_handlers() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        use tower::ServiceExt;
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        let network = Network::Lan {
            address: Ipv4Addr::new(192, 168, 1, 4),
            netmask: Ipv4Addr::new(255, 255, 255, 0),
        };
        let app = axum::Router::new()
            .route(
                "/",
                axum::routing::get(move || {
                    let hits = counter.clone();
                    async move {
                        hits.fetch_add(1, Ordering::SeqCst);
                        StatusCode::OK
                    }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                Access::new(network, None).unwrap(),
                admission,
            ));
        for (peer, uri, host, status) in [
            (None, "/", "192.168.1.4:3000", StatusCode::FORBIDDEN),
            (
                Some("8.8.8.8:42"),
                "/",
                "192.168.1.4:3000",
                StatusCode::FORBIDDEN,
            ),
            (
                Some("192.168.2.2:42"),
                "/",
                "192.168.1.4:3000",
                StatusCode::FORBIDDEN,
            ),
            (
                Some("192.168.1.5:42"),
                "/",
                "attacker.example:3000",
                StatusCode::BAD_REQUEST,
            ),
            (
                Some("192.168.1.5:42"),
                "http://attacker.example:3000/",
                "192.168.1.4:3000",
                StatusCode::BAD_REQUEST,
            ),
            (
                Some("192.168.1.5:42"),
                "https://192.168.1.4:3000/",
                "192.168.1.4:3000",
                StatusCode::BAD_REQUEST,
            ),
        ] {
            let mut request = axum::http::Request::builder()
                .uri(uri)
                .header("host", host)
                .header("x-forwarded-for", "192.168.1.5")
                .body(axum::body::Body::empty())
                .unwrap();
            if let Some(peer) = peer {
                request
                    .extensions_mut()
                    .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
            }
            assert_eq!(app.clone().oneshot(request).await.unwrap().status(), status);
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        let mut request = axum::http::Request::builder()
            .uri("/")
            .header("host", "192.168.1.4:3000")
            .body(axum::body::Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo("192.168.1.5:42".parse::<SocketAddr>().unwrap()));
        assert_eq!(app.oneshot(request).await.unwrap().status(), StatusCode::OK);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }
}
