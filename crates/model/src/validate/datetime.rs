//! `ParamValidator._type_check_datetime`: whether botocore's
//! `parse_to_aware_datetime` accepts a value, as far as that can be known
//! without `dateutil`.
//!
//! `parse_timestamp` tries `datetime.fromtimestamp` on numbers and on
//! strings `float()` reads, then `dateutil.parser.parse`. A value in the
//! years both the UTC and local readings can hold is accepted. A string or
//! bytes value that is ISO 8601 (a zone-less time or a date alone too) or
//! RFC 822 is accepted. A value of another
//! Python type fails `float()` and `dateutil` alike and is rejected, as are
//! the empty string and a NaN number. Anything else depends on `dateutil`,
//! or on the `RuntimeError` `parse_timestamp` raises for an out-of-range
//! epoch, which the validator does not catch, so it is undecided.
use super::Undecided;
use crate::value::Value;
use timestamp::Timestamp;

/// Seconds of a day either side of UTC that a local reading may shift.
const DAY: i64 = 86_400;

pub(super) fn accepted(value: &Value) -> Result<bool, Undecided> {
    match value {
        Value::Timestamp(_) | Value::Bool(_) => Ok(true),
        Value::Null | Value::List(_) | Value::Map(_) => Ok(false),
        Value::Integer(seconds) => epoch(*seconds as f64, value),
        Value::Float(seconds) if seconds.is_nan() => Ok(false),
        Value::Float(seconds) => epoch(*seconds, value),
        Value::BigInteger(_) => Err(undecided(value)),
        Value::String(text) => text_accepted(text, value),
        // Timestamp text is ASCII: dateutil's parser refuses a lone
        // surrogate as it refuses any text it cannot read.
        Value::PyString(_) => Ok(false),
        Value::Blob(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => text_accepted(text, value),
            Err(_) => Err(undecided(value)),
        },
    }
}

fn undecided(value: &Value) -> Undecided {
    Undecided::Timestamp(crate::pyrepr::py_repr(value))
}

/// `datetime.fromtimestamp(seconds, tz)` succeeds for both time zones
/// botocore tries.
fn epoch(seconds: f64, value: &Value) -> Result<bool, Undecided> {
    let low = (Timestamp::MIN.unix_seconds() + DAY) as f64;
    let high = (Timestamp::MAX.unix_seconds() - DAY) as f64;
    if (low..=high).contains(&seconds) {
        Ok(true)
    } else {
        Err(undecided(value))
    }
}

fn text_accepted(text: &str, value: &Value) -> Result<bool, Undecided> {
    if text.is_empty() {
        return Ok(false);
    }
    if let Some(seconds) = crate::pyrepr::float_from_text(text) {
        if seconds.is_finite() {
            return epoch(seconds, value);
        }
        return Err(undecided(value));
    }
    if Timestamp::parse_iso8601_zoned(text).is_some()
        || Timestamp::parse_rfc822_zoned(text).is_some()
    {
        return Ok(true);
    }
    Err(undecided(value))
}
