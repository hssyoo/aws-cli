//! Text to [`Timestamp`]: the ISO 8601 and RFC 822 forms AWS services send.
use crate::{Civil, MONTH_NAMES, Timestamp};

/// A cursor over ASCII text.
struct Cursor<'a> {
    rest: &'a [u8],
}

impl<'a> Cursor<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            rest: text.as_bytes(),
        }
    }

    fn digits(&mut self, count: usize) -> Option<u32> {
        let (head, tail) = self.rest.split_at_checked(count)?;
        let mut value = 0;
        for byte in head {
            value = value * 10 + u32::from(digit(*byte)?);
        }
        self.rest = tail;
        Some(value)
    }

    /// One or two digits, as a day of the month may be written.
    fn short_number(&mut self) -> Option<u32> {
        let two = self.rest.get(1).copied().and_then(digit).is_some();
        self.digits(if two { 2 } else { 1 })
    }

    fn eat(&mut self, byte: u8) -> bool {
        let found = self.rest.first() == Some(&byte);
        if found {
            self.rest = &self.rest[1..];
        }
        found
    }

    fn eat_any(&mut self, bytes: &[u8]) -> Option<u8> {
        let first = *self.rest.first()?;
        bytes.contains(&first).then(|| {
            self.rest = &self.rest[1..];
            first
        })
    }

    fn word(&mut self, len: usize) -> Option<&'a [u8]> {
        let (head, tail) = self.rest.split_at_checked(len)?;
        self.rest = tail;
        Some(head)
    }

    fn is_done(&self) -> bool {
        self.rest.is_empty()
    }
}

fn digit(byte: u8) -> Option<u8> {
    byte.is_ascii_digit().then(|| byte - b'0')
}

pub(crate) fn iso8601(text: &str) -> Option<Timestamp> {
    let mut cursor = Cursor::new(text);
    let year = i32::try_from(cursor.digits(4)?).ok()?;
    let month = cursor.eat(b'-').then(|| cursor.digits(2))??;
    let day = cursor.eat(b'-').then(|| cursor.digits(2))??;
    let mut civil = Civil {
        year,
        month,
        day,
        hour: 0,
        minute: 0,
        second: 0,
        micros: 0,
    };
    if cursor.eat_any(b"Tt ").is_some() {
        clock(&mut cursor, &mut civil)?;
    }
    let offset = iso_offset(&mut cursor)?;
    cursor.is_done().then_some(())?;
    Timestamp::from_civil(civil)?.offset_by(offset)
}

fn clock(cursor: &mut Cursor<'_>, civil: &mut Civil) -> Option<()> {
    civil.hour = cursor.digits(2)?;
    civil.minute = cursor.eat(b':').then(|| cursor.digits(2))??;
    if cursor.eat(b':') {
        civil.second = cursor.digits(2)?;
        if cursor.eat(b'.') {
            civil.micros = fraction(cursor)?;
        }
    }
    Some(())
}

/// Fraction digits truncated to microseconds, as dateutil's `_parsems` does.
fn fraction(cursor: &mut Cursor<'_>) -> Option<u32> {
    let count = cursor
        .rest
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .count();
    let digits = cursor.word(count).filter(|digits| !digits.is_empty())?;
    let mut micros = 0;
    for place in 0..6 {
        let value = digits.get(place).map_or(0, |byte| u32::from(byte - b'0'));
        micros = micros * 10 + value;
    }
    Some(micros)
}

/// Seconds east of UTC: `Z`, `±HH`, `±HHMM` or `±HH:MM`; none is UTC.
fn iso_offset(cursor: &mut Cursor<'_>) -> Option<i64> {
    if cursor.eat_any(b"Zz").is_some() || cursor.is_done() {
        return Some(0);
    }
    let sign = match cursor.eat_any(b"+-")? {
        b'-' => -1,
        _ => 1,
    };
    let hours = cursor.digits(2)?;
    let minutes = if cursor.is_done() {
        0
    } else {
        cursor.eat(b':');
        cursor.digits(2)?
    };
    (hours < 24 && minutes < 60).then(|| sign * i64::from(hours * 3600 + minutes * 60))
}

pub(crate) fn rfc822(text: &str) -> Option<Timestamp> {
    let mut cursor = Cursor::new(text);
    if cursor.rest.get(3) == Some(&b',') {
        cursor.word(4)?;
        cursor.eat(b' ').then_some(())?;
    }
    let (year, month, day) = rfc822_date(&mut cursor)?;
    let civil = Civil {
        year,
        month,
        day,
        hour: cursor.digits(2)?,
        minute: cursor.eat(b':').then(|| cursor.digits(2))??,
        second: cursor.eat(b':').then(|| cursor.digits(2))??,
        micros: 0,
    };
    cursor.eat(b' ').then_some(())?;
    let offset = rfc822_zone(cursor.rest)?;
    Timestamp::from_civil(civil)?.offset_by(offset)
}

/// `DD Mon YYYY ` with the trailing space.
fn rfc822_date(cursor: &mut Cursor<'_>) -> Option<(i32, u32, u32)> {
    let day = cursor.short_number()?;
    cursor.eat(b' ').then_some(())?;
    let month_name = cursor.word(3)?;
    let month = MONTH_NAMES
        .iter()
        .position(|name| name.as_bytes() == month_name)?;
    cursor.eat(b' ').then_some(())?;
    let year = i32::try_from(cursor.digits(4)?).ok()?;
    cursor.eat(b' ').then_some(())?;
    Some((year, u32::try_from(month).ok()? + 1, day))
}

fn rfc822_zone(zone: &[u8]) -> Option<i64> {
    if matches!(zone, b"GMT" | b"UTC" | b"UT" | b"Z") {
        return Some(0);
    }
    let mut cursor = Cursor { rest: zone };
    let sign = match cursor.eat_any(b"+-")? {
        b'-' => -1,
        _ => 1,
    };
    let hours = cursor.digits(2)?;
    let minutes = cursor.digits(2)?;
    (cursor.is_done() && hours < 24 && minutes < 60)
        .then(|| sign * i64::from(hours * 3600 + minutes * 60))
}
