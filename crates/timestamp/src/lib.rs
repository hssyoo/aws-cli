//! UTC instants to the microsecond, and the text forms botocore writes and
//! reads them in: ISO 8601 basic (SigV4's `X-Amz-Date`) and extended,
//! seconds since the epoch, and RFC 822.
//!
//! The range is Python's `datetime`: from 0001-01-01 to the last
//! microsecond of 9999-12-31. One civil-date implementation serves every
//! form ([`Civil`]).
//!
//! ```
//! use timestamp::Timestamp;
//!
//! let instant = Timestamp::from_unix_micros(1_440_938_160_500_000);
//! assert_eq!(instant.map(|t| t.iso8601_basic()).as_deref(), Some("20150830T123600Z"));
//! assert_eq!(instant.map(|t| t.epoch_seconds()).as_deref(), Some("1440938160.5"));
//! let rfc822 = instant.map(|t| t.rfc822());
//! assert_eq!(rfc822.as_deref(), Some("Sun, 30 Aug 2015 12:36:00 GMT"));
//! ```
#![forbid(unsafe_code)]
#[cfg(any(test, feature = "testing"))]
pub mod fuzz;
mod parse;

use std::time::{SystemTime, UNIX_EPOCH};

const MICROS_PER_SECOND: i64 = 1_000_000;
const SECONDS_PER_DAY: i64 = 86_400;
/// 0001-01-01T00:00:00Z and 9999-12-31T23:59:59Z, `datetime.min` and the
/// last second of `datetime.max`.
const FIRST_SECOND: i64 = -62_135_596_800;
const LAST_SECOND: i64 = 253_402_300_799;
const FIRST_MICROS: i64 = FIRST_SECOND * MICROS_PER_SECOND;
const LAST_MICROS: i64 = (LAST_SECOND + 1) * MICROS_PER_SECOND - 1;
const DAY_NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// An instant in UTC, to the microsecond.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp {
    /// Microseconds since 1970-01-01T00:00:00Z.
    micros: i64,
}

/// A UTC date and time of day, each field in its calendar range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Civil {
    /// 1 to 9999.
    pub year: i32,
    /// 1 to 12.
    pub month: u8,
    /// 1 to the month's length.
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    /// 0 to 999,999.
    pub microsecond: u32,
}

/// A wall time and the UTC offset it is in, in seconds east, or `None` for
/// a naive time: what `datetime.isoformat` prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Zoned {
    pub civil: Civil,
    pub offset_seconds: Option<i32>,
}

impl Zoned {
    /// `datetime.isoformat()`: `YYYY-MM-DDTHH:MM:SS`, `.ffffff` when there
    /// are microseconds, then the offset as `+HH:MM`, with `:SS` when it has
    /// seconds.
    pub fn isoformat(&self) -> String {
        let c = self.civil;
        let mut text = format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            c.year, c.month, c.day, c.hour, c.minute, c.second
        );
        if c.microsecond != 0 {
            text.push_str(&format!(".{:06}", c.microsecond));
        }
        if let Some(offset) = self.offset_seconds {
            let sign = if offset < 0 { '-' } else { '+' };
            let magnitude = offset.unsigned_abs();
            text.push_str(&format!(
                "{sign}{:02}:{:02}",
                magnitude / 3_600,
                magnitude % 3_600 / 60
            ));
            if magnitude % 60 != 0 {
                text.push_str(&format!(":{:02}", magnitude % 60));
            }
        }
        text
    }
}

impl Timestamp {
    /// 0001-01-01T00:00:00Z, `datetime.datetime.min` in UTC.
    pub const MIN: Self = Self {
        micros: FIRST_MICROS,
    };
    /// 9999-12-31T23:59:59.999999Z, `datetime.datetime.max` in UTC.
    pub const MAX: Self = Self {
        micros: LAST_MICROS,
    };
    /// The instant `micros` microseconds after the epoch, `None` outside
    /// the range.
    pub fn from_unix_micros(micros: i64) -> Option<Self> {
        (FIRST_MICROS..=LAST_MICROS)
            .contains(&micros)
            .then_some(Self { micros })
    }
    /// The instant `seconds` after the epoch, `None` outside the range.
    pub fn from_unix_seconds(seconds: i64) -> Option<Self> {
        seconds
            .checked_mul(MICROS_PER_SECOND)
            .and_then(Self::from_unix_micros)
    }
    /// The clock's current instant (`datetime.datetime.utcnow`).
    pub fn now() -> Self {
        let micros = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(after) => i64::try_from(after.as_micros()).unwrap_or(i64::MAX),
            Err(before) => i64::try_from(before.duration().as_micros()).map_or(i64::MIN, |m| -m),
        };
        Self {
            micros: micros.clamp(FIRST_MICROS, LAST_MICROS),
        }
    }
    /// The date and time `civil` names, `None` for one that does not exist.
    pub fn from_civil(civil: Civil) -> Option<Self> {
        let valid = (1..=12).contains(&civil.month)
            && (1..=days_in_month(civil.year, civil.month)).contains(&civil.day)
            && civil.hour < 24
            && civil.minute < 60
            && civil.second < 60
            && civil.microsecond < 1_000_000
            && (1..=9999).contains(&civil.year);
        if !valid {
            return None;
        }
        let days = days_from_civil(civil.year, civil.month, civil.day);
        let seconds = days * SECONDS_PER_DAY
            + i64::from(civil.hour) * 3_600
            + i64::from(civil.minute) * 60
            + i64::from(civil.second);
        Self::from_unix_micros(seconds * MICROS_PER_SECOND + i64::from(civil.microsecond))
    }
    /// `datetime.datetime.fromtimestamp(value, tz)`'s instant: the float's
    /// fraction rounds half to even at the microsecond
    /// (`_PyTime_ObjectToTimeval`). `None` for a value that is not finite or
    /// is outside the range.
    pub fn from_epoch_float(value: f64) -> Option<Self> {
        if !value.is_finite() {
            return None;
        }
        let mut whole = value.trunc();
        let mut micros = ((value - whole) * 1e6).round_ties_even();
        if micros >= 1e6 {
            micros -= 1e6;
            whole += 1.0;
        } else if micros < 0.0 {
            micros += 1e6;
            whole -= 1.0;
        }
        if !(FIRST_SECOND as f64..=LAST_SECOND as f64).contains(&whole) {
            return None;
        }
        // Both are whole numbers in range, so the conversions are exact.
        Self::from_unix_micros(whole as i64 * MICROS_PER_SECOND + micros as i64)
    }
    /// This instant's wall time `offset_seconds` east of UTC, as
    /// `fromtimestamp(t, tz)` gives it for a fixed-offset zone. `None` when
    /// the wall time is outside the range.
    pub fn zoned(&self, offset_seconds: i32) -> Option<Zoned> {
        let shifted = self
            .micros
            .checked_add(i64::from(offset_seconds) * MICROS_PER_SECOND)
            .and_then(Self::from_unix_micros)?;
        Some(Zoned {
            civil: shifted.civil(),
            offset_seconds: Some(offset_seconds),
        })
    }
    /// Microseconds since the epoch.
    pub fn unix_micros(&self) -> i64 {
        self.micros
    }
    /// Whole seconds since the epoch, rounded toward the past
    /// (`calendar.timegm(value.timetuple())`).
    pub fn unix_seconds(&self) -> i64 {
        self.micros.div_euclid(MICROS_PER_SECOND)
    }
    /// The microseconds past [`Timestamp::unix_seconds`].
    pub fn subsec_micros(&self) -> u32 {
        // Below one million, so it fits.
        u32::try_from(self.micros.rem_euclid(MICROS_PER_SECOND)).unwrap_or(0)
    }
    /// The UTC date and time of day.
    pub fn civil(&self) -> Civil {
        let seconds = self.unix_seconds();
        let (year, month, day) = civil_from_days(seconds.div_euclid(SECONDS_PER_DAY));
        let of_day = seconds.rem_euclid(SECONDS_PER_DAY);
        let field = |value: i64| u8::try_from(value).unwrap_or(0);
        Civil {
            year,
            month,
            day,
            hour: field(of_day / 3_600),
            minute: field(of_day % 3_600 / 60),
            second: field(of_day % 60),
            microsecond: self.subsec_micros(),
        }
    }
    /// `YYYYMMDD`, the date of a SigV4 credential scope.
    pub fn basic_date(&self) -> String {
        let civil = self.civil();
        format!("{:04}{:02}{:02}", civil.year, civil.month, civil.day)
    }
    /// `YYYYMMDDTHHMMSSZ`, the fraction dropped: botocore's
    /// `SIGV4_TIMESTAMP`, as `X-Amz-Date` carries it.
    pub fn iso8601_basic(&self) -> String {
        let civil = self.civil();
        format!(
            "{}T{:02}{:02}{:02}Z",
            self.basic_date(),
            civil.hour,
            civil.minute,
            civil.second
        )
    }
    /// `YYYY-MM-DDTHH:MM:SSZ`, with `.ffffff` before the `Z` when there
    /// are microseconds (botocore's `Serializer._timestamp_iso8601`).
    pub fn iso8601(&self) -> String {
        let civil = self.civil();
        let fraction = match civil.microsecond {
            0 => String::new(),
            micros => format!(".{micros:06}"),
        };
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{fraction}Z",
            civil.year, civil.month, civil.day, civil.hour, civil.minute, civil.second
        )
    }
    /// Seconds since the epoch as botocore's
    /// `Serializer._timestamp_unixtimestamp` gives them, printed as Python
    /// prints that number: an integer for a whole second, else the float
    /// `(seconds * 10**6 + microsecond) / 10**6`, as `repr` spells it.
    pub fn epoch_seconds(&self) -> String {
        if self.subsec_micros() == 0 {
            return self.unix_seconds().to_string();
        }
        // Exact: every microsecond of the range is below 2**53, and the
        // division rounds once, as Python's true division of integers does.
        pytext::repr::float_repr(self.micros as f64 / 1e6)
    }
    /// `Day, DD Mon YYYY HH:MM:SS GMT`, whole seconds: `formatdate(seconds,
    /// usegmt=True)`, as botocore's `Serializer._timestamp_rfc822` calls it.
    pub fn rfc822(&self) -> String {
        let civil = self.civil();
        let days = self.unix_seconds().div_euclid(SECONDS_PER_DAY);
        // 1970-01-01 was a Thursday.
        let weekday = usize::try_from((days + 3).rem_euclid(7)).unwrap_or(0);
        format!(
            "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
            DAY_NAMES[weekday],
            civil.day,
            MONTH_NAMES[usize::from(civil.month - 1)],
            civil.year,
            civil.hour,
            civil.minute,
            civil.second
        )
    }
}

fn is_leap(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i32, month: u8, day: u8) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month = i64::from(month);
    let shifted = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The proleptic Gregorian date `days` after 1970-01-01 (Howard Hinnant's
/// `civil_from_days`).
fn civil_from_days(days: i64) -> (i32, u8, u8) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    let small = |value: i64| u8::try_from(value).unwrap_or(0);
    (i32::try_from(year).unwrap_or(0), small(month), small(day))
}

#[cfg(test)]
mod tests;
