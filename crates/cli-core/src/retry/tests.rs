use super::*;
use std::time::Duration;

/// The ceiling with a base other than the two defaults, as a service's own
/// backoff configuration gives one (`_SERVICE_BACKOFF_CONFIG`), and for
/// attempt counts past any doubling.
#[test]
fn the_ceiling_doubles_any_base_and_saturates() {
    for (attempt, base_ms, ceiling_ms) in [
        (1, 25, 25),
        (3, 25, 100),
        (64, 25, 20_000),
        (u64::MAX, 1_000, 20_000),
        (0, 50, 50),
    ] {
        assert_eq!(
            backoff_ceiling(attempt, Duration::from_millis(base_ms)),
            Duration::from_millis(ceiling_ms),
            "{attempt} {base_ms}"
        );
    }
    assert_eq!(
        backoff_ceiling(5, ErrorKind::Throttling.base()),
        Duration::from_secs(16)
    );
    assert_eq!(
        backoff_ceiling(6, ErrorKind::Throttling.base()),
        BACKOFF_MAX
    );
}

fn longest(attempt: u64, kind: ErrorKind) -> Duration {
    backoff_delay(&mut || u64::MAX, attempt, kind.base()) + Duration::from_nanos(1)
}

#[test]
fn each_backoffs_ceiling_doubles_from_its_kinds_scale_up_to_twenty_seconds() {
    for (attempt, seconds) in (1..).zip([1, 2, 4, 8, 16, 20, 20]) {
        assert_eq!(
            longest(attempt, ErrorKind::Throttling),
            Duration::from_secs(seconds)
        );
    }
    let transient = [
        50, 100, 200, 400, 800, 1600, 3200, 6400, 12_800, 20_000, 20_000,
    ];
    for (attempt, millis) in (1..).zip(transient) {
        assert_eq!(
            longest(attempt, ErrorKind::Transient),
            Duration::from_millis(millis)
        );
    }
    for kind in [ErrorKind::Throttling, ErrorKind::Transient] {
        assert_eq!(longest(u64::MAX, kind), Duration::from_secs(20));
    }
}
#[test]
fn backoffs_spread_across_the_whole_wait_below_the_ceiling() {
    let mut state: u64 = 0x5eed;
    let mut random = || {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    };
    for attempt in [1, 2] {
        let ceiling = Duration::from_secs(1 << (attempt - 1));
        let delays: Vec<_> = (0..1000)
            .map(|_| backoff_delay(&mut random, attempt, ErrorKind::Throttling.base()))
            .collect();
        assert!(delays.iter().all(|delay| *delay < ceiling));
        assert!(
            delays
                .iter()
                .min()
                .is_some_and(|delay| *delay < ceiling / 10)
        );
        assert!(
            delays
                .iter()
                .max()
                .is_some_and(|delay| *delay > ceiling * 9 / 10)
        );
    }
}
#[test]
fn the_quota_covers_retries_by_their_kinds_cost() {
    for (kind, retries) in [(ErrorKind::Transient, 35), (ErrorKind::Throttling, 100)] {
        let mut quota = Quota::default();
        for _ in 0..retries {
            assert!(quota.acquire(kind).is_some());
        }
        assert_eq!(quota.acquire(kind), None);
    }
    let mut quota = Quota { available: 13 };
    assert_eq!(quota.acquire(ErrorKind::Transient), None);
    assert_eq!(quota.acquire(ErrorKind::Throttling), Some(5));
    assert_eq!(quota.available, 8);
}
#[test]
fn a_success_gives_back_its_last_retrys_cost() {
    let mut quota = Quota { available: 8 };
    quota.release(Some(14));
    assert_eq!(quota.available, 22);
    quota.release(None);
    assert_eq!(quota.available, 23);
    let mut full = Quota::default();
    full.release(None);
    full.release(Some(5));
    assert_eq!(full.available, Quota::CAPACITY_MAX);
}
