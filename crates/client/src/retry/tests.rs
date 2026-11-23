use super::*;
use crate::error::ServiceError;
use crate::http::{Headers, HttpResponse};
use crate::transport::TransportError;
use model::shapes::Model;
use std::time::Duration;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// An operation with two modelled errors: one retryable as throttling,
/// one not retryable, and one retryable as transient by its error code.
const MODEL: &str = r#"{
    "metadata": {"protocol": "json"},
    "operations": {"Op": {"name": "Op", "http": {"method": "POST", "requestUri": "/"},
        "errors": [{"shape": "SlowDown"}, {"shape": "Denied"}, {"shape": "Busy"}]}},
    "shapes": {
        "SlowDown": {"type": "structure", "members": {}, "exception": true,
            "retryable": {"throttling": true}},
        "Denied": {"type": "structure", "members": {}, "exception": true},
        "Busy": {"type": "structure", "members": {}, "exception": true,
            "error": {"code": "BusyCode"}, "retryable": {"throttling": false}}
    }
}"#;

fn response(status: u16) -> HttpResponse {
    HttpResponse {
        status,
        headers: Headers::new(),
        body: Vec::new(),
    }
}

fn error(code: &str, status: u16) -> ServiceError {
    let mut error = model::value::Map::new();
    error.insert("Code".into(), model::value::Value::String(code.into()));
    ServiceError {
        error,
        ..ServiceError::new("Op", status)
    }
}

/// What botocore's standard conditions say of a response: retryable, and
/// if so whether as throttling.
fn kind_of(model: &Model, status: u16, code: Option<&str>) -> Option<ErrorKind> {
    let operation = &model.operations()[0];
    let response = response(status);
    let error = code.map(|code| error(code, status));
    let attempt = Attempt::Response {
        response: &response,
        error: error.as_ref(),
    };
    classify(model, operation, &attempt)
}

/// `StandardRetryConditions` with `ThrottlingErrorDetector`: transient
/// codes and statuses, the throttling codes, a 429 no modelled error
/// covers, and modelled `retryable` errors; nothing else.
#[test]
fn responses_are_classified_as_standard_mode_does() -> TestResult {
    let model = Model::from_json(MODEL.as_bytes())?;
    use ErrorKind::{Throttling, Transient};
    for (status, code, kind) in [
        (500, None, Some(Transient)),
        (502, None, Some(Transient)),
        (503, None, Some(Transient)),
        (504, None, Some(Transient)),
        (501, None, None),
        (400, Some("RequestTimeout"), Some(Transient)),
        (400, Some("RequestTimeoutException"), Some(Transient)),
        (400, Some("PriorRequestNotComplete"), Some(Throttling)),
        (400, Some("Throttling"), Some(Throttling)),
        (400, Some("ThrottlingException"), Some(Throttling)),
        (
            400,
            Some("ProvisionedThroughputExceededException"),
            Some(Throttling),
        ),
        (400, Some("RequestLimitExceeded"), Some(Throttling)),
        (503, Some("SlowDown"), Some(Throttling)),
        (429, None, Some(Throttling)),
        (429, Some("Unmodelled"), Some(Throttling)),
        (429, Some("Denied"), None),
        (400, Some("SlowDown"), Some(Throttling)),
        (400, Some("BusyCode"), Some(Transient)),
        (400, Some("Busy"), None),
        (403, Some("AccessDenied"), None),
        (400, Some("ValidationException"), None),
        (200, None, None),
    ] {
        assert_eq!(kind_of(&model, status, code), kind, "{status} {code:?}");
    }
    Ok(())
}

/// A transport failure botocore's session raises is retried as transient;
/// an interrupt is not.
#[test]
fn transport_failures_are_transient() -> TestResult {
    let model = Model::from_json(MODEL.as_bytes())?;
    let operation = &model.operations()[0];
    for (error, kind) in [
        (TransportError::Connect, Some(ErrorKind::Transient)),
        (TransportError::ReadTimeout, Some(ErrorKind::Transient)),
        (TransportError::Interrupted, None),
    ] {
        assert_eq!(classify(&model, operation, &Attempt::Failed(&error)), kind);
    }
    Ok(())
}

/// `needs_retry` under `MaxAttemptsChecker`: a retry while attempts are
/// left, `MaxAttemptsReached` once they are not, whatever the error, and
/// never a retry for an error that is not retryable.
#[test]
fn retries_stop_at_max_attempts_and_say_so() {
    let policy = Standard::new(3, ErrorKind::Transient.base());
    let first = policy.needs_retry(1, Some(ErrorKind::Transient), None, 0.5);
    assert!(first.delay.is_some() && !first.max_attempts_reached);
    let second = policy.needs_retry(2, Some(ErrorKind::Transient), None, 0.5);
    assert!(second.delay.is_some() && !second.max_attempts_reached);
    let third = policy.needs_retry(3, Some(ErrorKind::Transient), None, 0.5);
    assert_eq!(
        third,
        Verdict {
            delay: None,
            max_attempts_reached: true,
            retry_cost: None,
            exhausted_delay: None,
        }
    );
    let not_retryable = policy.needs_retry(1, None, None, 0.5);
    assert_eq!(not_retryable.delay, None);
    assert!(!not_retryable.max_attempts_reached);
    let one_attempt = Standard::new(1, ErrorKind::Transient.base());
    assert!(
        one_attempt
            .needs_retry(1, None, None, 0.5)
            .max_attempts_reached
    );
}

/// `delay_amount`: `random() * min(base * 2 ** (attempt - 1), 20)`, the
/// base 1 s for throttling and the client's otherwise.
#[test]
fn the_delay_is_the_jittered_exponential_backoff() {
    let policy = Standard::new(10, Duration::from_millis(25));
    let delay = |attempt, kind| policy.needs_retry(attempt, Some(kind), None, 0.5).delay;
    assert_eq!(
        delay(1, ErrorKind::Transient),
        Some(Duration::from_micros(12_500))
    );
    assert_eq!(
        delay(3, ErrorKind::Transient),
        Some(Duration::from_millis(50))
    );
    assert_eq!(
        delay(2, ErrorKind::Throttling),
        Some(Duration::from_secs(1))
    );
    assert_eq!(
        delay(9, ErrorKind::Throttling),
        Some(Duration::from_secs(10))
    );
}

/// `x-amz-retry-after`, in milliseconds, raises the delay to it, but no
/// more than five seconds past the backoff's own.
#[test]
fn retry_after_raises_the_delay_within_five_seconds() {
    let policy = Standard::new(5, ErrorKind::Transient.base());
    let delay = |after: Option<Duration>| {
        policy
            .needs_retry(1, Some(ErrorKind::Throttling), after, 0.5)
            .delay
    };
    assert_eq!(delay(None), Some(Duration::from_millis(500)));
    assert_eq!(
        delay(Some(Duration::from_secs(2))),
        Some(Duration::from_secs(2))
    );
    assert_eq!(
        delay(Some(Duration::from_secs(30))),
        Some(Duration::from_millis(5_500))
    );
    assert_eq!(
        delay(Some(Duration::ZERO)),
        Some(Duration::from_millis(500))
    );
    let mut headers = Headers::new();
    for (value, after) in [
        ("1500", Some(Duration::from_millis(1_500))),
        ("0", Some(Duration::ZERO)),
        ("-1", None),
        ("soon", None),
    ] {
        headers.set("x-amz-retry-after", value);
        assert_eq!(retry_after(&headers), after, "{value}");
    }
    assert_eq!(retry_after(&Headers::new()), None);
}

/// The quota: 14 per transient retry, 5 per throttling one; no retry once
/// it runs out; a success gives back the last retry's cost, or 1.
#[test]
fn the_quota_limits_retries_across_calls() {
    let policy = Standard::new(100, ErrorKind::Transient.base());
    let mut granted = 0;
    while policy
        .needs_retry(1, Some(ErrorKind::Transient), None, 0.0)
        .delay
        .is_some()
    {
        granted += 1;
    }
    assert_eq!(granted, 35);
    let refused = policy.needs_retry(1, Some(ErrorKind::Transient), None, 0.0);
    assert_eq!((refused.delay, refused.max_attempts_reached), (None, false));
    // The 10 left cover two throttling retries.
    let throttled = || {
        policy
            .needs_retry(1, Some(ErrorKind::Throttling), None, 0.0)
            .retry_cost
    };
    assert_eq!(
        [throttled(), throttled(), throttled()],
        [Some(5), Some(5), None]
    );
    policy.succeeded(Some(14));
    assert_eq!(
        policy
            .needs_retry(1, Some(ErrorKind::Transient), None, 0.0)
            .retry_cost,
        Some(14)
    );
    policy.succeeded(None);
    assert_eq!(
        policy
            .needs_retry(1, Some(ErrorKind::Throttling), None, 0.0)
            .retry_cost,
        None
    );
}

/// `_get_retry_after_delay`: `int(value) / 1000.0` as Python reads it
/// (whitespace, a sign, underscores), ignored when negative, unreadable or
/// beyond a float (`OverflowError`); a value too large for the delay to
/// hold still counts, as the cap at `t_i + 5` makes any such value equal.
#[test]
fn retry_after_is_read_as_python_s_int() {
    let after = |value: &str| retry_after(&Headers::from_pairs([("x-amz-retry-after", value)]));
    assert_eq!(after(" 1500 "), Some(Duration::from_millis(1500)));
    assert_eq!(after("+2_000"), Some(Duration::from_secs(2)));
    assert_eq!(after("-1"), None);
    assert_eq!(after("soon"), None);
    assert_eq!(after("1.5"), None);
    assert_eq!(after(&"9".repeat(30)), Some(Duration::MAX));
    assert_eq!(after(&"9".repeat(400)), None);
}

/// When the quota refuses a retry, the verdict still carries the delay
/// botocore computes for a long-polling operation's sleep.
#[test]
fn an_exhausted_quota_still_reports_the_delay() {
    let policy = Standard::new(100, Duration::from_millis(50));
    let mut refused = None;
    for attempt in 1..60 {
        let verdict = policy.needs_retry(attempt, Some(ErrorKind::Transient), None, 0.5);
        if verdict.delay.is_none() {
            refused = Some(verdict);
            break;
        }
    }
    let refused = refused.unwrap_or_else(|| panic!("the quota never ran out"));
    assert!(
        refused
            .exhausted_delay
            .is_some_and(|delay| delay > Duration::ZERO)
    );
}
