//! UTC instants at microsecond precision, the resolution of Python's `datetime`.
//!
//! The range is Python's: `datetime.MINYEAR` (1) to `datetime.MAXYEAR` (9999).
//! Civil-date arithmetic is the proleptic Gregorian calendar, as `datetime.date`
//! uses.
#![forbid(unsafe_code)]

mod parse;

const SECONDS_PER_DAY: i64 = 86_400;
const MICROS_PER_SECOND: u32 = 1_000_000;
const DAY_NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// A calendar date and time of day in UTC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Civil {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub micros: u32,
}

/// An instant in UTC between 0001-01-01 and 9999-12-31T23:59:59.999999.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Timestamp {
    seconds: i64,
    micros: u32,
}

impl Timestamp {
    /// `datetime.datetime.min` as UTC.
    pub const MIN: Self = Self {
        seconds: -62_135_596_800,
        micros: 0,
    };
    /// `datetime.datetime.max` as UTC.
    pub const MAX: Self = Self {
        seconds: 253_402_300_799,
        micros: 999_999,
    };

    /// The instant `seconds` and `micros` after the Unix epoch, if in range.
    pub fn from_unix(seconds: i64, micros: u32) -> Option<Self> {
        let instant = Self { seconds, micros };
        (micros < MICROS_PER_SECOND && Self::MIN <= instant && instant <= Self::MAX)
            .then_some(instant)
    }

    /// The instant a valid civil date and time names, as `datetime.datetime(...)` accepts it.
    pub fn from_civil(civil: Civil) -> Option<Self> {
        let valid = (1..=12).contains(&civil.month)
            && (1..=days_in_month(civil.year, civil.month)).contains(&civil.day)
            && civil.hour < 24
            && civil.minute < 60
            && civil.second < 60
            && (1..=9999).contains(&civil.year);
        if !valid {
            return None;
        }
        let days = days_from_civil(civil.year, civil.month, civil.day);
        let clock = i64::from(civil.hour * 3600 + civil.minute * 60 + civil.second);
        Self::from_unix(days * SECONDS_PER_DAY + clock, civil.micros)
    }

    /// `datetime.datetime.fromtimestamp(value, timezone.utc)`: the float's fraction
    /// rounds half to even at microseconds (`_PyTime_ObjectToTimeval`).
    pub fn from_epoch(value: f64) -> Option<Self> {
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
        let in_range = (Self::MIN.seconds as f64..=Self::MAX.seconds as f64).contains(&whole);
        let (seconds, micros) = (whole as i64, micros as u32);
        if in_range {
            Self::from_unix(seconds, micros)
        } else {
            None
        }
    }

    /// Whole seconds since the Unix epoch, rounded toward the past.
    pub const fn unix_seconds(self) -> i64 {
        self.seconds
    }

    /// The microseconds past [`Timestamp::unix_seconds`].
    pub const fn micros(self) -> u32 {
        self.micros
    }

    /// The civil date and time of this instant.
    pub fn civil(self) -> Civil {
        let days = self.seconds.div_euclid(SECONDS_PER_DAY);
        let clock = self.seconds.rem_euclid(SECONDS_PER_DAY);
        let (year, month, day) = civil_from_days(days);
        let clock = clock as u32;
        Civil {
            year,
            month,
            day,
            hour: clock / 3600,
            minute: clock / 60 % 60,
            second: clock % 60,
            micros: self.micros,
        }
    }

    /// `datetime.weekday()`: Monday is 0.
    pub fn weekday(self) -> usize {
        let days = self.seconds.div_euclid(SECONDS_PER_DAY);
        // 1970-01-01 was a Thursday (3).
        (days + 3).rem_euclid(7) as usize
    }

    /// botocore's `Serializer._timestamp_iso8601`: `%Y-%m-%dT%H:%M:%SZ`, or with
    /// `.%f` before the `Z` when the microseconds are not zero.
    pub fn iso8601(self) -> String {
        let c = self.civil();
        let mut text = format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            c.year, c.month, c.day, c.hour, c.minute, c.second
        );
        if c.micros > 0 {
            text.push_str(&format!(".{:06}", c.micros));
        }
        text.push('Z');
        text
    }

    /// `email.utils.formatdate(timeval, usegmt=True)`, which botocore's
    /// `Serializer._timestamp_rfc822` uses; the microseconds are dropped.
    pub fn rfc822(self) -> String {
        let c = self.civil();
        format!(
            "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
            DAY_NAMES[self.weekday()],
            c.day,
            MONTH_NAMES[c.month as usize - 1],
            c.year,
            c.hour,
            c.minute,
            c.second
        )
    }

    /// An ISO 8601 date, optionally with a time (`T`, `t` or a space between them),
    /// a fraction (truncated to microseconds, as dateutil does) and a `Z` or
    /// `±HH[:MM]` offset. No offset means UTC, as botocore's
    /// `parse_to_aware_datetime` assumes.
    pub fn parse_iso8601(text: &str) -> Option<Self> {
        parse::iso8601(text)
    }

    /// The RFC 822 date botocore writes and HTTP uses,
    /// `[Day, ]DD Mon YYYY HH:MM:SS GMT`, also with `UTC`, `Z` or `±HHMM`.
    pub fn parse_rfc822(text: &str) -> Option<Self> {
        parse::rfc822(text)
    }

    fn offset_by(self, seconds: i64) -> Option<Self> {
        Self::from_unix(self.seconds.checked_sub(seconds)?, self.micros)
    }
}

/// Days from 1970-01-01 to a proleptic Gregorian date (H. Hinnant's
/// `days_from_civil`).
pub fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month = i64::from(month);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The proleptic Gregorian date `days` after 1970-01-01 (H. Hinnant's
/// `civil_from_days`).
pub fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year as i32, month as u32, day as u32)
}

/// `calendar.monthrange(year, month)[1]`.
pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

#[cfg(test)]
mod tests;
