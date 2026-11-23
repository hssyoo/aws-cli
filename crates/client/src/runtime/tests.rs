use super::*;
use crate::testing::FixedRuntime;
use cli_core::interrupt::State;
use std::time::{Duration, Instant};

/// `str(uuid.uuid4())`: 8-4-4-4-12 lowercase hex, version 4, RFC 4122
/// variant.
fn is_uuid4(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    let lengths: Vec<usize> = groups.iter().map(|group| group.len()).collect();
    let hex = text
        .bytes()
        .all(|byte| byte == b'-' || byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    lengths == [8, 4, 4, 4, 12]
        && hex
        && groups[2].starts_with('4')
        && groups[3].starts_with(['8', '9', 'a', 'b'])
}

#[test]
fn the_system_runtime_makes_random_version_4_uuids() {
    static STATE: State = State::new();
    let runtime = SystemRuntime::new(&STATE);
    let ids: Vec<String> = (0..64).map(|_| runtime.uuid4()).collect();
    assert!(ids.iter().all(|id| is_uuid4(id)), "{ids:?}");
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), ids.len());
}

/// `random.random()`: a float in [0, 1), not stuck on one value.
#[test]
fn the_system_runtime_draws_floats_in_the_unit_interval() {
    static STATE: State = State::new();
    let runtime = SystemRuntime::new(&STATE);
    let draws: Vec<f64> = (0..1000).map(|_| runtime.random()).collect();
    assert!(draws.iter().all(|draw| (0.0..1.0).contains(draw)));
    assert!(draws.iter().any(|draw| *draw < 0.5) && draws.iter().any(|draw| *draw >= 0.5));
}

/// A sleep waits out its time, unless an interrupt was requested, which
/// ends it at once as interrupted.
#[test]
fn the_system_runtime_sleeps_until_interrupted() {
    static QUIET: State = State::new();
    static INTERRUPTED: State = State::new();
    let started = Instant::now();
    assert_eq!(
        SystemRuntime::new(&QUIET).sleep(Duration::from_millis(20)),
        Ok(())
    );
    assert!(started.elapsed() >= Duration::from_millis(20));
    INTERRUPTED.request();
    let started = Instant::now();
    assert_eq!(
        SystemRuntime::new(&INTERRUPTED).sleep(Duration::from_secs(30)),
        Err(Interrupted)
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn the_system_runtime_reads_the_clock() {
    static STATE: State = State::new();
    let before = timestamp::Timestamp::now();
    let now = SystemRuntime::new(&STATE).now();
    assert!(now >= before && now <= timestamp::Timestamp::now());
}

/// The test runtime is deterministic: a fixed clock, numbered uuids, a
/// fixed draw, and sleeps that are recorded instead of waited.
#[test]
fn the_fixed_runtime_is_deterministic() {
    let at = timestamp::Timestamp::from_unix_seconds(1_440_938_160);
    let runtime = FixedRuntime::new(at.unwrap_or_else(timestamp::Timestamp::now), 0.25);
    assert_eq!(Some(runtime.now()), at);
    assert_eq!(runtime.uuid4(), "00000000-0000-4000-8000-000000000001");
    assert_eq!(runtime.uuid4(), "00000000-0000-4000-8000-000000000002");
    assert!(is_uuid4(&runtime.uuid4()));
    assert_eq!(runtime.random(), 0.25);
    let started = Instant::now();
    assert_eq!(runtime.sleep(Duration::from_secs(20)), Ok(()));
    assert_eq!(runtime.sleep(Duration::from_millis(50)), Ok(()));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(
        runtime.slept(),
        [Duration::from_secs(20), Duration::from_millis(50)]
    );
}
