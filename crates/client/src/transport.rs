//! The only door to the network: the CRT adapter (T5b) sends through
//! `crt::http`, and tests use [`crate::testing::InMemoryTransport`].
use crate::http::{HttpRequest, HttpResponse};
use std::time::Duration;

/// botocore's `connect_timeout` and `read_timeout` (`Config`), each 60
/// seconds unless configured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    /// How long the connection, and a TLS handshake, may take.
    pub connect: Duration,
    /// How long the response may stay silent.
    pub read: Duration,
}

/// Why a request has no response: what botocore's `URLLib3Session.send`
/// raises, each the exception named.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransportError {
    /// `EndpointConnectionError`: refused, or no address for the host.
    Connect,
    /// `ConnectTimeoutError`.
    ConnectTimeout,
    /// `ReadTimeoutError`.
    ReadTimeout,
    /// `ConnectionClosedError`: the connection ended before a response did.
    ConnectionClosed,
    /// `SSLError`, with the TLS library's reason.
    Tls { reason: String },
    /// `ProxyConnectionError`: the proxy at `url`, credentials masked.
    Proxy { url: String },
    /// `HTTPClientError`, for anything else the client raises.
    Client { reason: String },
    /// The user interrupted the request (botocore lets `CancelledError`
    /// through).
    Interrupted,
}

impl TransportError {
    /// Whether standard retries try again (`TransientRetryableChecker`):
    /// every failure botocore's session raises is a `ConnectionError` or an
    /// `HTTPClientError`, which it retries; an interrupt is not.
    pub fn is_retryable(&self) -> bool {
        !matches!(self, Self::Interrupted)
    }
}

/// Names the failure; [`crate::error::CallError`] gives botocore's text,
/// which names the endpoint.
impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect => f.write_str("could not connect"),
            Self::ConnectTimeout => f.write_str("the connect timed out"),
            Self::ReadTimeout => f.write_str("the read timed out"),
            Self::ConnectionClosed => f.write_str("the connection closed before a response"),
            Self::Tls { reason } => write!(f, "TLS failed: {reason}"),
            Self::Proxy { url } => write!(f, "could not connect to the proxy {url}"),
            Self::Client { reason } => write!(f, "the HTTP client failed: {reason}"),
            Self::Interrupted => f.write_str("interrupted"),
        }
    }
}

impl std::error::Error for TransportError {}

/// Sends one request and reads its response whole.
pub trait Transport: Send + Sync + std::fmt::Debug {
    /// # Errors
    /// A request that got no response.
    fn send(
        &self,
        request: &HttpRequest,
        timeouts: Timeouts,
    ) -> Result<HttpResponse, TransportError>;
}

pub mod crt;

#[cfg(test)]
mod tests;
