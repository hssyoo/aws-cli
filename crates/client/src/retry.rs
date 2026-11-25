//! botocore's standard retry mode for the model client
//! (`botocore/retries/standard.py`), on the parts it shares with the S3
//! client ([`cli_core::retry`]): what the `classify_retry` stage sees of an
//! attempt and may decide, the classification, and the handler.
use crate::error::ServiceError;
use crate::http::HttpResponse;
use crate::transport::TransportError;
pub use cli_core::retry::ErrorKind;
use cli_core::retry::{Quota, backoff_ceiling};
use model::shapes::{Model, Operation};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetryDecision {
    /// Try again, within the attempts and the retry quota left.
    Retry,
    /// Do not try again.
    Stop,
    /// The check raised, as a botocore retry checker's exception ends the
    /// call from `needs-retry`.
    Fail(crate::hooks::HookError),
}

/// `TransientRetryableChecker._TRANSIENT_ERROR_CODES`.
const TRANSIENT_CODES: [&str; 3] = [
    "RequestTimeout",
    "RequestTimeoutException",
    "PriorRequestNotComplete",
];
/// `TransientRetryableChecker._TRANSIENT_STATUS_CODES`.
const TRANSIENT_STATUSES: [u16; 4] = [500, 502, 503, 504];
/// `ThrottledRetryableChecker._THROTTLED_ERROR_CODES`.
const THROTTLED_CODES: [&str; 14] = [
    "Throttling",
    "ThrottlingException",
    "ThrottledException",
    "RequestThrottledException",
    "TooManyRequestsException",
    "ProvisionedThroughputExceededException",
    "TransactionInProgressException",
    "RequestLimitExceeded",
    "BandwidthLimitExceeded",
    "LimitExceededException",
    "RequestThrottled",
    "SlowDown",
    "PriorRequestNotComplete",
    "EC2ThrottledException",
];
/// `ThrottlingStatusCodeChecker._THROTTLING_STATUS_CODES`.
const THROTTLING_STATUS: u16 = 429;
/// `ExponentialBackoff._RETRY_AFTER_MAX_ADDITIONAL`.
const RETRY_AFTER_ADDITIONAL_MAX: Duration = Duration::from_secs(5);

/// Whether standard mode retries an attempt, and as which kind of error:
/// botocore's `StandardRetryConditions` without its attempt count, and
/// `ThrottlingErrorDetector` for the kind. A service's own checks
/// (`retries/special.py`) are its customization's `classify_retry`.
pub fn classify(model: &Model, operation: &Operation, attempt: &Attempt<'_>) -> Option<ErrorKind> {
    let (response, error) = match attempt {
        Attempt::Failed(error) => return error.is_retryable().then_some(ErrorKind::Transient),
        Attempt::Response { response, error } => (*response, *error),
    };
    let code = error.and_then(ServiceError::code);
    let modeled = code.and_then(|code| modeled_retryable(model, operation, code));
    let unmodeled_429 = response.status == THROTTLING_STATUS
        && !code.is_some_and(|code| is_modeled(model, operation, code));
    let throttled = code.is_some_and(|code| THROTTLED_CODES.contains(&code));
    let retryable = code.is_some_and(|code| TRANSIENT_CODES.contains(&code))
        || TRANSIENT_STATUSES.contains(&response.status)
        || throttled
        || unmodeled_429
        || modeled.is_some();
    if !retryable {
        return None;
    }
    let throttling =
        throttled || modeled.map_or(unmodeled_429, |kind| kind == ErrorKind::Throttling);
    Some(if throttling {
        ErrorKind::Throttling
    } else {
        ErrorKind::Transient
    })
}

/// `ModeledRetryErrorDetector.detect_error_type`: the kind of the
/// operation's first retryable error shape whose code is `code`.
fn modeled_retryable(model: &Model, operation: &Operation, code: &str) -> Option<ErrorKind> {
    operation.errors.iter().find_map(|&shape| {
        let retryable = model.shape(shape).traits.retryable.as_ref()?;
        (model.error_code(shape) == Some(code)).then_some(if retryable.throttling {
            ErrorKind::Throttling
        } else {
            ErrorKind::Transient
        })
    })
}

/// `ModeledRetryErrorDetector.is_modeled_error`.
fn is_modeled(model: &Model, operation: &Operation, code: &str) -> bool {
    operation
        .errors
        .iter()
        .any(|&shape| model.error_code(shape) == Some(code))
}

/// `ExponentialBackoff._get_retry_after_delay`: `x-amz-retry-after` in
/// milliseconds, `int(value) / 1000.0`, as Python's `int()` reads it (the
/// `HeadersDict` value, so a repeated header's values joined, which no
/// `int()` reads);
/// `None` for a value it cannot read, a negative one, or one beyond a
/// float (`OverflowError`). A value beyond a `Duration` is the longest
/// one, which the delay's cap at `t_i + 5` makes equal to any other.
pub fn retry_after(headers: &crate::http::Headers) -> Option<Duration> {
    let value = model::pyrepr::int_from_text(&headers.joined("x-amz-retry-after")?).ok()?;
    match value {
        model::value::Value::Integer(millis) => {
            u64::try_from(millis).ok().map(Duration::from_millis)
        }
        model::value::Value::BigInteger(digits) => {
            let float: f64 = digits.parse().ok()?;
            (float.is_finite() && float >= 0.0).then_some(Duration::MAX)
        }
        _ => None,
    }
}

/// What [`Standard::needs_retry`] decided for one attempt.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Verdict {
    /// The wait before the next attempt; `None` for no retry.
    pub delay: Option<Duration>,
    /// `ResponseMetadata.MaxAttemptsReached`.
    pub max_attempts_reached: bool,
    /// What the retry took from the quota.
    pub retry_cost: Option<u32>,
    /// The delay a retry would have waited when only the quota refused it,
    /// which a long-polling operation still sleeps
    /// (`RetryHandler.needs_retry`).
    pub exhausted_delay: Option<Duration>,
}

/// botocore's standard retry handler for one client (`RetryHandler` with
/// `StandardRetryConditions`, `ExponentialBackoff` and
/// `RetryQuotaChecker`), its quota shared by the client's calls.
#[derive(Debug)]
pub struct Standard {
    max_attempts: u32,
    /// The backoff's base for errors that are not throttling.
    transient_base: Duration,
    quota: Mutex<Quota>,
}

impl Standard {
    pub fn new(max_attempts: u32, transient_base: Duration) -> Self {
        Self {
            max_attempts,
            transient_base,
            quota: Mutex::new(Quota::default()),
        }
    }
    pub const fn max_attempts(&self) -> u32 {
        self.max_attempts
    }
    /// `RetryHandler.needs_retry` after the 1-based `attempt`, of the kind
    /// `retryable` says (`None` for an error not retried), with the
    /// response's `x-amz-retry-after` and the jitter's draw, `random()`.
    pub fn needs_retry(
        &self,
        attempt: u32,
        retryable: Option<ErrorKind>,
        retry_after: Option<Duration>,
        random: f64,
    ) -> Verdict {
        // `MaxAttemptsChecker` runs first and marks the limit whatever the
        // error.
        let max_attempts_reached = attempt >= self.max_attempts;
        let refused = Verdict {
            delay: None,
            max_attempts_reached,
            retry_cost: None,
            exhausted_delay: None,
        };
        let Some(kind) = retryable.filter(|_| !max_attempts_reached) else {
            return refused;
        };
        let base = match kind {
            ErrorKind::Throttling => kind.base(),
            ErrorKind::Transient => self.transient_base,
        };
        let backoff = backoff_ceiling(u64::from(attempt), base).mul_f64(random);
        let delay = retry_after.map_or(backoff, |after| {
            backoff.max(after.min(backoff + RETRY_AFTER_ADDITIONAL_MAX))
        });
        let Some(cost) = self
            .quota
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .acquire(kind)
        else {
            return Verdict {
                exhausted_delay: Some(delay),
                ..refused
            };
        };
        Verdict {
            delay: Some(delay),
            max_attempts_reached,
            retry_cost: Some(cost),
            exhausted_delay: None,
        }
    }
    /// `RetryQuotaChecker.release_retry_quota` for a call that succeeded:
    /// the last retry's cost back, or `NO_RETRY_INCREMENT` without one.
    pub fn succeeded(&self, last_retry_cost: Option<u32>) {
        self.quota
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .release(last_retry_cost);
    }
}

#[cfg(test)]
mod tests;
