//! The body of the fuzz target over argparse's `type=int` and `type=float`
//! conversion (`fuzz/fuzz_targets/convert.rs`), which the stable replay
//! test (`client::testing::fuzz`'s tests) also runs.
use crate::convert::{Converted, ValueType, convert};
use std::ffi::OsStr;

/// Both conversions of any argv value. An `int` is canonical decimal; a
/// `float` reads back as a double; and text `int()` takes, `float()` takes
/// as the same number, rounded once.
pub fn convert_both(bytes: &[u8]) {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return;
    };
    let int = convert(ValueType::Int, OsStr::new(text));
    let float = convert(ValueType::Float, OsStr::new(text));
    if let Converted::Value(digits) = &float {
        let named = ["NaN", "Infinity", "-Infinity"].contains(&digits.as_str());
        assert!(named || digits.parse::<f64>().is_ok(), "{text:?}: {digits}");
    }
    let Converted::Value(digits) = &int else {
        return;
    };
    let magnitude = digits.strip_prefix('-').unwrap_or(digits);
    assert!(
        magnitude.bytes().all(|byte| byte.is_ascii_digit()),
        "{text:?}: {digits}"
    );
    assert!(
        magnitude == "0" || !magnitude.starts_with('0'),
        "{text:?}: {digits}"
    );
    assert!(digits != "-0", "{text:?}");
    assert!(
        matches!(float, Converted::Value(_)),
        "{text:?}: int() takes it, float() not"
    );
    if let Converted::Value(float) = float {
        assert_eq!(
            digits.parse::<f64>().ok(),
            float.parse::<f64>().ok(),
            "{text:?}"
        );
    }
}
