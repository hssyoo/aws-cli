use super::*;
use std::time::Duration;

/// botocore's `ExponentialBackoff.delay_amount` ceiling: the base doubles
/// with each attempt after the first, and stops at 20 seconds.
#[test]
fn the_backoff_ceiling_doubles_up_to_twenty_seconds() {
    let throttling = ErrorKind::Throttling.base();
    let transient = ErrorKind::Transient.base();
    assert_eq!(throttling, Duration::from_secs(1));
    assert_eq!(transient, Duration::from_millis(50));
    assert_eq!(backoff_ceiling(1, transient), Duration::from_millis(50));
    assert_eq!(backoff_ceiling(2, transient), Duration::from_millis(100));
    assert_eq!(backoff_ceiling(5, transient), Duration::from_millis(800));
    assert_eq!(backoff_ceiling(5, throttling), Duration::from_secs(16));
    assert_eq!(backoff_ceiling(6, throttling), BACKOFF_MAX);
    assert_eq!(backoff_ceiling(64, transient), BACKOFF_MAX);
    assert_eq!(backoff_ceiling(u64::MAX, throttling), BACKOFF_MAX);
    assert_eq!(
        backoff_ceiling(3, Duration::from_millis(25)),
        Duration::from_millis(100)
    );
}

/// A uniform delay below the ceiling, from a random word: the smallest word
/// gives none, and the largest stays below the ceiling.
#[test]
fn a_backoff_delay_is_drawn_below_its_ceiling() {
    let ceiling = backoff_ceiling(3, ErrorKind::Transient.base());
    assert_eq!(backoff_delay(&mut || 0, 3, ErrorKind::Transient.base()), Duration::ZERO);
    let largest = backoff_delay(&mut || u64::MAX, 3, ErrorKind::Transient.base());
    assert!(largest < ceiling && largest > ceiling / 2, "{largest:?}");
}

/// `RetryQuota` and `RetryQuotaChecker`: 500 to start, 14 per retry and 5
/// per throttling retry, nothing past empty, and the last retry's cost back
/// (or 1 for a call that never retried) on success, up to the start.
#[test]
fn the_retry_quota_charges_and_refunds_as_botocore_does() {
    let mut quota = Quota::default();
    assert_eq!(quota.available, 500);
    assert_eq!(quota.acquire(ErrorKind::Transient), Some(14));
    assert_eq!(quota.acquire(ErrorKind::Throttling), Some(5));
    assert_eq!(quota.available, 481);
    quota.release(Some(5));
    quota.release(None);
    assert_eq!(quota.available, 487);
    let mut empty = Quota { available: 13 };
    assert_eq!(empty.acquire(ErrorKind::Transient), None);
    assert_eq!(empty.available, 13);
    let mut full = Quota::default();
    full.release(Some(14));
    assert_eq!(full.available, 500);
}
