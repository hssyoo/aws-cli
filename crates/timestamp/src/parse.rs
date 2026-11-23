//! Reading the text forms back.
//!
//! botocore reads a response's timestamps with `botocore.utils.
//! parse_timestamp`: a number is seconds since the epoch, and any other
//! text goes to `dateutil.parser.parse`, which reads far more forms than
//! these. Each parser here reads one form, the one services send, whole,
//! and refuses anything else: a caller that meets another form decides
//! what to do with it.
use super::{
    Civil, DAY_NAMES, FIRST_MICROS, LAST_MICROS, MICROS_PER_SECOND, MONTH_NAMES, Timestamp, Zoned,
};

impl Timestamp {
    /// Reads `YYYYMMDDTHHMMSSZ`, an `X-Amz-Date` value: `None` for any
    /// other text, and for a date or time that does not exist.
    pub fn parse_iso8601_basic(text: &str) -> Option<Self> {
        let mut scan = Scan::new(text);
        let (year, month, day) = (scan.digits(4)?, scan.digits(2)?, scan.digits(2)?);
        scan.byte(b'T')?;
        let (hour, minute, second) = (scan.digits(2)?, scan.digits(2)?, scan.digits(2)?);
        scan.byte(b'Z')?;
        scan.end()?;
        Self::from_civil(civil(year, month, day, hour, minute, second, 0)?)
    }
    /// Reads `YYYY-MM-DDTHH:MM:SS`, then an optional fraction, then a zone:
    /// `Z`, or an offset `+HH:MM` or `+HHMM`, which is taken away. The `T`
    /// may be `t` or a space, and `Z` may be `z`, as dateutil reads them.
    /// Past six digits the fraction is cut, as dateutil cuts it. A time
    /// without a zone is refused: botocore would read it in the local zone.
    pub fn parse_iso8601(text: &str) -> Option<Self> {
        Self::parse_iso8601_zoned(text)?.instant()
    }
    /// [`Timestamp::parse_iso8601`]'s form with the zone optional, as the
    /// wall time and the zone written: what `dateutil.parser.parse` returns
    /// for it, naive without a zone and `+00:00` for `Z`.
    pub fn parse_iso8601_zoned(text: &str) -> Option<Zoned> {
        let mut scan = Scan::new(text);
        let year = scan.digits(4)?;
        scan.byte(b'-')?;
        let month = scan.digits(2)?;
        scan.byte(b'-')?;
        let day = scan.digits(2)?;
        scan.one_of(b"Tt ")?;
        let (hour, minute, second) = scan.clock()?;
        let microsecond = match scan.byte(b'.') {
            Some(()) => scan.fraction()?,
            None => 0,
        };
        let offset = match scan.peek() {
            None => None,
            Some(_) => Some(scan.iso_zone()?),
        };
        scan.end()?;
        zoned(
            civil(year, month, day, hour, minute, second, microsecond)?,
            offset,
        )
    }
    /// Reads seconds since the epoch as Python's `float` reads a decimal:
    /// an optional sign, digits with an optional point and digits on at
    /// least one side of it, and an optional exponent, `e` or `E` with an
    /// optional sign and digits, as `repr` writes one below 1e-4. Past six
    /// places the fraction is rounded to the microsecond, ties to even.
    /// Spaces, underscores and the names `float` knows are refused.
    pub fn parse_epoch_seconds(text: &str) -> Option<Self> {
        let (negative, unsigned) = match text.as_bytes().first()? {
            b'-' => (true, &text[1..]),
            b'+' => (false, &text[1..]),
            _ => (false, text),
        };
        let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
            Some((mantissa, exponent)) => (mantissa, exponent_value(exponent)?),
            None => (unsigned, 0),
        };
        let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        let all_digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
        if whole.len() + fraction.len() == 0 || !all_digits(whole) || !all_digits(fraction) {
            return None;
        }
        let (whole, fraction) = shift_point(whole, fraction, exponent)?;
        // Past 19 digits no second is in the range.
        let seconds: i64 = match whole.as_str() {
            "" => 0,
            _ if whole.len() > 19 => return None,
            digits => digits.parse().ok()?,
        };
        let micros = seconds
            .checked_mul(MICROS_PER_SECOND)?
            .checked_add(rounded_micros(&fraction))?;
        Self::from_unix_micros(if negative { -micros } else { micros })
    }
    /// Reads `Day, DD Mon YYYY HH:MM:SS GMT`: the day's name is optional,
    /// the day of the month may have one digit, and the zone may be `GMT`,
    /// `UTC` or an offset `+HHMM`, which is taken away. The day's name is
    /// not checked against the date, as dateutil does not check it.
    pub fn parse_rfc822(text: &str) -> Option<Self> {
        Self::parse_rfc822_zoned(text)?.instant()
    }
    /// [`Timestamp::parse_rfc822`]'s form as the wall time and the zone
    /// written, `GMT` and `UTC` being `+00:00`.
    pub fn parse_rfc822_zoned(text: &str) -> Option<Zoned> {
        let mut scan = Scan::new(text);
        scan.day_name()?;
        let day = scan.digits(2).or_else(|| scan.digits(1))?;
        scan.byte(b' ')?;
        let month = scan.name(&MONTH_NAMES)? + 1;
        scan.byte(b' ')?;
        let year = scan.digits(4)?;
        scan.byte(b' ')?;
        let (hour, minute, second) = scan.clock()?;
        scan.byte(b' ')?;
        let offset = scan.rfc822_zone()?;
        scan.end()?;
        zoned(
            civil(year, month, day, hour, minute, second, 0)?,
            Some(offset),
        )
    }
    /// This instant moved by `micros`, `None` past the range.
    fn shifted(self, micros: i64) -> Option<Self> {
        let moved = self.micros.checked_add(micros)?;
        (FIRST_MICROS..=LAST_MICROS)
            .contains(&moved)
            .then_some(Self { micros: moved })
    }
}

/// A wall time that exists, with an offset in microseconds east of UTC.
fn zoned(civil: Civil, offset_micros: Option<i64>) -> Option<Zoned> {
    Timestamp::from_civil(civil)?;
    let offset_seconds = offset_micros
        .map(|micros| i32::try_from(micros / MICROS_PER_SECOND))
        .transpose()
        .ok()?;
    Some(Zoned {
        civil,
        offset_seconds,
    })
}

impl Zoned {
    /// The instant a zoned wall time names; `None` for a naive one or one
    /// past the range.
    fn instant(&self) -> Option<Timestamp> {
        let offset = i64::from(self.offset_seconds?) * MICROS_PER_SECOND;
        Timestamp::from_civil(self.civil)?.shifted(-offset)
    }
}

/// A [`Civil`] of the fields read, `None` for a field too large for its
/// type: [`Timestamp::from_civil`] checks the ranges.
fn civil(
    year: u32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    microsecond: u32,
) -> Option<Civil> {
    let small = |value: u32| u8::try_from(value).ok();
    Some(Civil {
        year: i32::try_from(year).ok()?,
        month: small(month)?,
        day: small(day)?,
        hour: small(hour)?,
        minute: small(minute)?,
        second: small(second)?,
        microsecond,
    })
}

/// An exponent's value: an optional sign and one or more digits, `None`
/// for anything else. A magnitude past a million is a million, which moves
/// the point as far past every instant of the range as any larger one.
fn exponent_value(text: &str) -> Option<i64> {
    const LIMIT: i64 = 1_000_000;
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let significant = digits.trim_start_matches('0');
    let magnitude = match significant.len() {
        0 => 0,
        1..=6 => significant.parse::<i64>().ok()?.min(LIMIT),
        _ => LIMIT,
    };
    Some(if text.starts_with('-') {
        -magnitude
    } else {
        magnitude
    })
}

/// The whole and fraction digits of `whole.fraction` times ten to the
/// `exponent`, the whole part without leading zeros, and the fraction empty
/// when it is below half a microsecond. `None` for a whole part longer than
/// any second of the range.
fn shift_point(whole: &str, fraction: &str, exponent: i64) -> Option<(String, String)> {
    let digits = format!("{whole}{fraction}");
    let significant = digits.trim_start_matches('0');
    let leading = digits.len() - significant.len();
    let point = i64::try_from(whole.len()).ok()? + exponent - i64::try_from(leading).ok()?;
    let length = i64::try_from(significant.len()).ok()?;
    // Below the seventh place a fraction rounds to no microsecond.
    if significant.is_empty() || point < -6 {
        return Some((String::new(), String::new()));
    }
    if point > 20 {
        return None;
    }
    Some(match point {
        _ if point <= 0 => {
            let zeros = "0".repeat(usize::try_from(-point).ok()?);
            (String::new(), format!("{zeros}{significant}"))
        }
        _ if point >= length => {
            let zeros = "0".repeat(usize::try_from(point - length).ok()?);
            (format!("{significant}{zeros}"), String::new())
        }
        _ => {
            let (left, right) = significant.split_at(usize::try_from(point).ok()?);
            (left.to_owned(), right.to_owned())
        }
    })
}

/// The microseconds of a fraction's digits, rounded at the seventh, ties
/// to even: at most one million.
fn rounded_micros(fraction: &str) -> i64 {
    let digits = fraction.as_bytes();
    let kept = digits
        .iter()
        .chain(std::iter::repeat(&b'0'))
        .take(6)
        .fold(0, |value, digit| value * 10 + i64::from(digit - b'0'));
    let rest = digits.get(6..).unwrap_or(&[]);
    let Some((&first, after)) = rest.split_first() else {
        return kept;
    };
    let above_half = first > b'5' || (first == b'5' && after.iter().any(|digit| *digit != b'0'));
    let half = first == b'5' && !above_half;
    match above_half || (half && kept % 2 == 1) {
        true => kept + 1,
        false => kept,
    }
}

/// A cursor over the text being read.
struct Scan<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Scan<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            bytes: text.as_bytes(),
            at: 0,
        }
    }
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }
    fn byte(&mut self, expected: u8) -> Option<()> {
        (self.peek()? == expected).then(|| self.at += 1)
    }
    fn one_of(&mut self, set: &[u8]) -> Option<u8> {
        let byte = self.peek().filter(|byte| set.contains(byte))?;
        self.at += 1;
        Some(byte)
    }
    /// Exactly `count` ASCII digits, as a number.
    fn digits(&mut self, count: usize) -> Option<u32> {
        let run = self.bytes.get(self.at..self.at + count)?;
        if !run.iter().all(u8::is_ascii_digit) {
            return None;
        }
        self.at += count;
        Some(
            run.iter()
                .fold(0, |value, digit| value * 10 + u32::from(digit - b'0')),
        )
    }
    /// `HH:MM:SS`, as its three numbers.
    fn clock(&mut self) -> Option<(u32, u32, u32)> {
        let hour = self.digits(2)?;
        self.byte(b':')?;
        let minute = self.digits(2)?;
        self.byte(b':')?;
        Some((hour, minute, self.digits(2)?))
    }
    /// An optional `Day, ` before an RFC 822 date: `None` for a name that
    /// is not a day's or that `, ` does not follow.
    fn day_name(&mut self) -> Option<()> {
        if self.peek().is_some_and(|byte| byte.is_ascii_alphabetic()) {
            self.name(&DAY_NAMES)?;
            self.byte(b',')?;
            self.byte(b' ')?;
        }
        Some(())
    }
    /// One or more digits after a point, as microseconds, cut at six.
    fn fraction(&mut self) -> Option<u32> {
        let start = self.at;
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.at += 1;
        }
        let digits = &self.bytes[start..self.at];
        if digits.is_empty() {
            return None;
        }
        let micros = digits
            .iter()
            .chain(std::iter::repeat(&b'0'))
            .take(6)
            .fold(0, |value, digit| value * 10 + u32::from(digit - b'0'));
        Some(micros)
    }
    /// The index in `names` of the one that comes next.
    fn name(&mut self, names: &[&str]) -> Option<u32> {
        let index = names.iter().position(|name| {
            self.bytes
                .get(self.at..self.at + name.len())
                .is_some_and(|run| run == name.as_bytes())
        })?;
        self.at += names[index].len();
        u32::try_from(index).ok()
    }
    /// `Z`, `z`, `+HH:MM` or `+HHMM`, as microseconds east of UTC.
    fn iso_zone(&mut self) -> Option<i64> {
        if self.one_of(b"Zz").is_some() {
            return Some(0);
        }
        let sign = self.one_of(b"+-")?;
        let hours = self.digits(2)?;
        let _ = self.byte(b':');
        let minutes = self.digits(2)?;
        offset(sign, hours, minutes)
    }
    /// `GMT`, `UTC` or `+HHMM`, as microseconds east of UTC.
    fn rfc822_zone(&mut self) -> Option<i64> {
        if self.name(&["GMT", "UTC"]).is_some() {
            return Some(0);
        }
        let sign = self.one_of(b"+-")?;
        let (hours, minutes) = (self.digits(2)?, self.digits(2)?);
        offset(sign, hours, minutes)
    }
    fn end(&self) -> Option<()> {
        (self.at == self.bytes.len()).then_some(())
    }
}

/// A zone offset of `hours` and `minutes` east of UTC for `+`, west for
/// `-`, in microseconds.
fn offset(sign: u8, hours: u32, minutes: u32) -> Option<i64> {
    if hours > 23 || minutes > 59 {
        return None;
    }
    let micros = (i64::from(hours) * 3_600 + i64::from(minutes) * 60) * MICROS_PER_SECOND;
    Some(if sign == b'-' { -micros } else { micros })
}
