//! What the `classify_retry` stage sees of an attempt, and what it may
//! decide in place of the retry policy (botocore's `needs-retry` handlers).
use crate::error::ServiceError;
use crate::http::HttpResponse;
use crate::transport::TransportError;

/// How one attempt ended.
#[derive(Clone, Copy, Debug)]
pub enum Attempt<'a> {
    /// A response arrived; `error` is the service error it parsed to, for
    /// one that is not a success.
    Response {
        response: &'a HttpResponse,
        error: Option<&'a ServiceError>,
    },
    /// No response arrived.
    Failed(&'a TransportError),
}

/// A decision that overrides the retry policy's for one attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryDecision {
    /// Try again, within the attempts and the retry quota left.
    Retry,
    /// Do not try again.
    Stop,
}
