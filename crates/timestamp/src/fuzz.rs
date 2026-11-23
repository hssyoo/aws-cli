//! The body of the fuzz target over the timestamp readers
//! (`fuzz/fuzz_targets/timestamps.rs`), which the stable replay test
//! (`client::testing::fuzz`'s tests) also runs.
use crate::Timestamp;

/// Every reader over any text. None panics, and each value read prints in
/// its form and reads back as itself.
pub fn parse(bytes: &[u8]) {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return;
    };
    if let Some(zoned) = Timestamp::parse_iso8601_zoned(text) {
        assert_eq!(Timestamp::parse_iso8601_zoned(&zoned.isoformat()), Some(zoned));
    }
    if let Some(zoned) = Timestamp::parse_rfc822_zoned(text) {
        assert!(zoned.offset_seconds.is_some());
    }
    let readers: [(fn(&str) -> Option<Timestamp>, fn(&Timestamp) -> String); 4] = [
        (Timestamp::parse_iso8601, Timestamp::iso8601),
        (Timestamp::parse_iso8601_basic, Timestamp::iso8601_basic),
        (Timestamp::parse_epoch_seconds, Timestamp::epoch_seconds),
        (Timestamp::parse_rfc822, Timestamp::rfc822),
    ];
    for (read, print) in readers {
        if let Some(instant) = read(text) {
            assert_eq!(read(&print(&instant)), Some(instant), "{text:?}");
        }
    }
}
