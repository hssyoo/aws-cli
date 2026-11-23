//! The body of the fuzz target over the timestamp readers
//! (`fuzz/fuzz_targets/timestamps.rs`), which the stable replay test
//! (`client::testing::fuzz`'s tests) also runs.
use crate::Timestamp;

/// A reader and the printer of the form it reads.
type Form = (fn(&str) -> Option<Timestamp>, fn(&Timestamp) -> String);

/// Every reader over any text. None panics, and each value read prints in
/// its form and reads back as itself.
pub fn parse(bytes: &[u8]) {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return;
    };
    if let Some(zoned) = Timestamp::parse_iso8601_zoned(text) {
        assert_eq!(
            Timestamp::parse_iso8601_zoned(&zoned.isoformat()),
            Some(zoned)
        );
    }
    if let Some(zoned) = Timestamp::parse_rfc822_zoned(text) {
        assert!(zoned.offset_seconds.is_some());
    }
    let readers: [Form; 3] = [
        (iso8601_instant, Timestamp::iso8601),
        (Timestamp::parse_iso8601_basic, Timestamp::iso8601_basic),
        (Timestamp::parse_rfc822, Timestamp::rfc822),
    ];
    for (read, print) in readers {
        if let Some(instant) = read(text) {
            assert_eq!(read(&print(&instant)), Some(instant), "{text:?}");
        }
    }
    // Epoch seconds as botocore reads them, `float` then `fromtimestamp`,
    // read back from what they print, where a double holds each
    // microsecond (to 2**33 seconds).
    let from_float = |text: &str| text.parse().ok().and_then(Timestamp::from_epoch_float);
    if let Some(instant) = from_float(text).filter(|i| i.unix_seconds().abs() < 1 << 33) {
        assert_eq!(
            from_float(&instant.epoch_seconds()),
            Some(instant),
            "{text:?}"
        );
    }
}

/// An ISO 8601 text's instant, `None` for a naive one.
fn iso8601_instant(text: &str) -> Option<Timestamp> {
    Timestamp::parse_iso8601_zoned(text)?.instant()
}
