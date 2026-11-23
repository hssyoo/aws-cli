//! The transport over `crt::http`: botocore's `URLLib3Session` as a client
//! builds it, on an HTTP-only CRT runtime. The one place in `client` that
//! sends.
//!
//! - **Trust**: the runtime's, the CLI's CA bundle settings (`--ca-bundle`,
//!   `AWS_CA_BUNDLE`, the profile's, or the payload's bundle), which the
//!   caller resolves.
//! - **Proxies**: the environment's, for each request's URL
//!   (`get_environ_proxies`): a cleartext request goes to the proxy with an
//!   absolute target, and an `https` one through its CONNECT tunnel. A
//!   proxy the native transport does not speak to fails the request; the
//!   router sends such settings to Python first.
//! - **On the wire**: `http.client`'s Host header and
//!   `Accept-Encoding: identity` first (`putrequest`), then the request's
//!   headers in order. The Host is the one the request was signed with:
//!   both come from [`credentials::Location`] (M069).
//! - **Keep-alive**: `tcp_keepalive` sets SO_KEEPALIVE at the system's
//!   timings, as botocore adds it to urllib3's socket options
//!   (`_compute_socket_options`). Each request connects anew.
use super::{Timeouts, Transport, TransportError};
use crate::http::{Headers, HttpRequest, HttpResponse};
use cli_core::proxy::{Environment, Forward, ForwardProxy};
use credentials::Location;
use std::sync::Arc;

/// Sends over a CRT runtime of its own.
#[derive(Debug)]
pub struct CrtTransport {
    /// Held so the client's runtime outlives every request.
    runtime: ::crt::HttpRuntime,
    client: ::crt::http::Client,
    env: cli_core::SnapshotEnv,
    keep_alive: bool,
}

impl CrtTransport {
    /// A transport whose TLS trusts `trust`, whose connections set
    /// SO_KEEPALIVE when `keep_alive` (`tcp_keepalive`), and whose requests
    /// take the proxies `env` names.
    ///
    /// # Errors
    /// [`TransportError::Client`] for a runtime the CRT could not build, a
    /// CA bundle it cannot load among the causes.
    pub fn new(
        trust: ::crt::runtime::Trust,
        keep_alive: bool,
        env: cli_core::SnapshotEnv,
        logger: Arc<::crt::debug::Logger>,
    ) -> Result<Self, TransportError> {
        let options = ::crt::RuntimeOptions {
            trust,
            verify_peer: true,
            event_loop_count: 1,
            ..::crt::RuntimeOptions::default()
        };
        let runtime = ::crt::HttpRuntime::new_with_logger(options, logger).map_err(|failure| {
            TransportError::Client {
                reason: failure.to_string(),
            }
        })?;
        let client = runtime.http_client();
        Ok(Self {
            runtime,
            client,
            env,
            keep_alive,
        })
    }
    /// The runtime the transport sends on.
    pub const fn runtime(&self) -> &::crt::HttpRuntime {
        &self.runtime
    }
}

/// Where a request to a URL goes, as urllib3 routes it.
struct Route<'a> {
    location: Location,
    /// The proxy a cleartext request is sent to, with an absolute target.
    forward: Option<&'a ForwardProxy>,
    /// The proxy that tunnels a TLS request.
    tunnel: Option<&'a ForwardProxy>,
}

impl<'a> Route<'a> {
    fn of(forward: &'a Forward, url: &str) -> Result<Self, TransportError> {
        let proxy = match forward {
            Forward::Direct => None,
            Forward::Proxy(proxy) => Some(proxy),
            Forward::Unsupported => {
                return Err(TransportError::Client {
                    reason: "the proxy for this URL is not supported natively".to_owned(),
                });
            }
        };
        let proxied = proxy.is_some() && !url.starts_with("https:");
        let location = Location::parse(url, proxied).ok_or_else(|| TransportError::Client {
            reason: format!("invalid URL {url}"),
        })?;
        let (forward, tunnel) = match (proxy, location.tls) {
            (Some(proxy), true) => (None, Some(proxy)),
            (proxy, _) => (proxy, None),
        };
        Ok(Self {
            location,
            forward,
            tunnel,
        })
    }
}

impl Transport for CrtTransport {
    fn send(
        &self,
        request: &HttpRequest,
        timeouts: Timeouts,
    ) -> Result<HttpResponse, TransportError> {
        let url = request.url.to_string();
        let forward = Environment::from_map(&self.env).forward(&url);
        let route = Route::of(&forward, &url)?;
        let location = &route.location;
        let (host, port) = route
            .forward
            .map_or((location.host.as_str(), location.port), |proxy| {
                (proxy.host.as_str(), proxy.port)
            });
        let mut headers = vec![
            ("Host", location.host_header.as_str()),
            ("Accept-Encoding", "identity"),
        ];
        headers.extend(request.headers.iter());
        let sent = ::crt::http::Request {
            host,
            port,
            tls: location.tls,
            method: request.method.as_str(),
            target: &location.target,
            headers: &headers,
            body: &request.body,
            tunnel: route.tunnel.map(|proxy| ::crt::http::Tunnel {
                host: &proxy.host,
                port: proxy.port,
            }),
            signing: None,
            keep_alive: self.keep_alive,
            connect_timeout: timeouts.connect,
            read_timeout: timeouts.read,
        };
        let response = self
            .client
            .send(&sent)
            .map_err(|error| transport_error(error, route.forward.or(route.tunnel)))?;
        Ok(HttpResponse {
            status: response.status,
            headers: Headers::from_pairs(
                response
                    .headers
                    .iter()
                    .map(|(name, value)| (name.as_str(), value.as_str())),
            ),
            body: response.body,
        })
    }
}

/// The exception botocore's session raises for `error`
/// (`URLLib3Session.send`).
fn transport_error(error: ::crt::http::Error, proxy: Option<&ForwardProxy>) -> TransportError {
    use ::crt::http::Error;
    match error {
        Error::ReadTimeout => TransportError::ReadTimeout,
        // urllib3's ProtocolError: the connection ended before a response.
        Error::Response(_) => TransportError::ConnectionClosed,
        // A request aws-c-http refuses to send, which http.client sends: no
        // botocore error corresponds, so it names the CRT's.
        Error::Connect(code) if ::crt::http::is_refusal(code) => TransportError::Client {
            reason: crt_message(code),
        },
        // urllib3 raises every failure to reach a proxy as ProxyError.
        Error::Connect(_) if proxy.is_some() => TransportError::Proxy {
            url: proxy.map(|proxy| proxy.url.clone()).unwrap_or_default(),
        },
        Error::Connect(
            ::crt::http::SOCKET_TIMEOUT_ERROR_CODE | ::crt::http::TLS_TIMEOUT_ERROR_CODE,
        ) => TransportError::ConnectTimeout,
        Error::Connect(code) if code == ::crt::http::TLS_FAILURE_ERROR_CODE => {
            TransportError::Tls {
                reason: crt_message(code),
            }
        }
        _ => TransportError::Connect,
    }
}

fn crt_message(code: i32) -> String {
    String::from_utf8_lossy(&::crt::s3::error_message(code)).into_owned()
}

#[cfg(test)]
mod tests;
