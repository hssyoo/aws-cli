use super::*;

#[test]
fn a_retry_is_admitted_while_an_attempt_is_left_and_the_quota_covers_it() {
    let mut quota = Quota::default();
    let mut attempts = Attempts::default();
    assert!(attempts.admit_retry(3, &mut quota, ErrorKind::Throttling));
    assert_eq!(attempts.last_retry_cost, Some(5));
    attempts.count += 1;
    assert!(attempts.admit_retry(3, &mut quota, ErrorKind::Transient));
    assert_eq!(attempts.last_retry_cost, Some(14));
    attempts.count += 1;
    assert!(!attempts.admit_retry(3, &mut quota, ErrorKind::Transient));
    assert_eq!(attempts.exhausted(3), Some(2));
    assert_eq!(quota.available, Quota::CAPACITY_MAX - 5 - 14);
    let mut empty = Quota { available: 4 };
    let mut first = Attempts::default();
    assert!(!first.admit_retry(3, &mut empty, ErrorKind::Throttling));
    assert_eq!((first.last_retry_cost, empty.available), (None, 4));
}
#[test]
fn a_429_the_crt_gave_up_on_is_a_throttle_unless_modeled() {
    let failure = |error_code: i32, status: i32, code: &[u8]| {
        let mut failure = TransferResult {
            error_code,
            response_status: status,
            ..TransferResult::default()
        };
        failure.service_error.code.set(code);
        failure
    };
    let gave_up = INVALID_RESPONSE_STATUS_ERROR_CODE;
    let modeled: &[&[u8]] = &[b"NoSuchBucket"];
    for (result, throttled) in [
        (failure(gave_up, 429, b""), true),
        (failure(gave_up, 429, b"TooManyRequests"), true),
        (failure(gave_up, 429, b"NoSuchBucket"), false),
        (failure(gave_up, 403, b""), false),
        (failure(gave_up, 503, b""), false),
        (
            failure(crt_sys::AWS_ERROR_S3_SLOW_DOWN, 429, b"SlowDown"),
            false,
        ),
        (failure(0, 200, b""), false),
    ] {
        assert_eq!(
            is_unmodeled_throttle(&result, modeled),
            throttled,
            "{result:?}"
        );
    }
}
