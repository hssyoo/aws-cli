//! The CLI's response timestamps (`customizations/timestampformat.py`).
//!
//! The CLI replaces botocore's timestamp parser at session start, so a
//! response carries no datetimes: under `wire` each timestamp member is the
//! value as parsed from the body (XML text, or a JSON number or string),
//! and under `iso8601`, the default, it is the text
//! `parse_timestamp(value).isoformat()`. The client's parser calls
//! [`TimestampParser::parse`] where botocore calls its `timestamp_parser`.
//!
//! Blobs need nothing here: the CLI's `cli_binary_format` handlers install
//! an identity blob parser under both values, so a blob member is its
//! base64 text as the body gave it.
use model::pyrepr::float_from_text;
use model::value::Value;
use timestamp::{Timestamp, Zoned};

/// `cli_timestamp_format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampFormat {
    Iso8601,
    Wire,
}

/// The `ConfigurationError` an unknown `cli_timestamp_format` raises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownTimestampFormat(pub String);

impl std::fmt::Display for UnknownTimestampFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Unknown cli_timestamp_format value: {}, valid values are \"wire\" or \"iso8601\"",
            self.0
        )
    }
}

impl std::error::Error for UnknownTimestampFormat {}

impl TimestampFormat {
    /// `add_timestamp_parser`: the profile's value, `iso8601` when unset.
    pub fn from_setting(value: Option<&str>) -> Result<Self, UnknownTimestampFormat> {
        match value {
            None | Some("iso8601") => Ok(Self::Iso8601),
            Some("wire") => Ok(Self::Wire),
            Some(other) => Err(UnknownTimestampFormat(other.into())),
        }
    }
}

/// Why a timestamp could not be printed as the Python route prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimestampError {
    /// An epoch value under `iso8601` in a local zone with daylight-saving
    /// time: dateutil's `tzlocal` picks between today's two offsets per
    /// instant, which only Python reproduces. Known before any request
    /// from [`TimestampParser::needs_local_zone`].
    LocalZoneHasDaylightSaving,
    /// A value only dateutil can read, or one `parse_timestamp` fails on.
    Unsupported(String),
}

/// The CLI's `timestamp_parser` for one session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimestampParser {
    format: TimestampFormat,
    /// [`cli_core::local_zone::LocalZone::fixed_offset`]: seconds east of
    /// UTC, `None` for a zone with daylight-saving time.
    local_offset: Option<i64>,
}

impl TimestampParser {
    pub const fn new(format: TimestampFormat, local_offset: Option<i64>) -> Self {
        Self {
            format,
            local_offset,
        }
    }

    /// Whether an epoch value would need a local zone this parser lacks.
    /// A caller whose response may carry an epoch timestamp (a JSON
    /// protocol, or a `unixTimestamp` member) routes to Python when true.
    pub const fn needs_local_zone(&self) -> bool {
        matches!(self.format, TimestampFormat::Iso8601) && self.local_offset.is_none()
    }

    /// `identity` or `iso_format` of a wire value.
    pub fn parse(&self, wire: &Value) -> Result<Value, TimestampError> {
        match self.format {
            TimestampFormat::Wire => Ok(wire.clone()),
            TimestampFormat::Iso8601 => self.iso_format(wire).map(Value::String),
        }
    }

    /// `parse_timestamp(value).isoformat()`: a number, or text `float()`
    /// reads, is an epoch in the local zone; other text is read as dateutil
    /// reads the ISO 8601 and RFC 822 forms services send.
    fn iso_format(&self, wire: &Value) -> Result<String, TimestampError> {
        let unsupported = || TimestampError::Unsupported(model::pyrepr::py_repr(wire));
        let epoch = match wire {
            Value::Integer(seconds) => Timestamp::from_unix_seconds(*seconds),
            Value::Bool(flag) => Timestamp::from_unix_seconds(i64::from(*flag)),
            Value::Float(seconds) => Timestamp::from_epoch_float(*seconds),
            Value::String(text) => match float_from_text(text) {
                Some(seconds) => Timestamp::from_epoch_float(seconds),
                None => {
                    return written(text)
                        .map(|zoned| zoned.isoformat())
                        .ok_or_else(unsupported);
                }
            },
            _ => return Err(unsupported()),
        };
        let epoch = epoch.ok_or_else(unsupported)?;
        let offset = self
            .local_offset
            .ok_or(TimestampError::LocalZoneHasDaylightSaving)?;
        let offset = i32::try_from(offset).map_err(|_| unsupported())?;
        epoch
            .zoned(offset)
            .map(|zoned| zoned.isoformat())
            .ok_or_else(unsupported)
    }
}

/// dateutil's reading of the forms services write.
fn written(text: &str) -> Option<Zoned> {
    Timestamp::parse_iso8601_zoned(text).or_else(|| Timestamp::parse_rfc822_zoned(text))
}

#[cfg(test)]
mod tests;
