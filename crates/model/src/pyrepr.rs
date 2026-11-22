//! Python's `repr()` and `str()` of a [`Value`], as botocore's error reports
//! print parameter values, and `str(type(value))`.
use crate::value::Value;
use std::fmt::Write as _;
use timestamp::Timestamp;

mod printable;

/// `str.isprintable()` for one character (`Py_UNICODE_ISPRINTABLE`).
pub fn is_printable(character: char) -> bool {
    let code = u32::from(character);
    let (pairs, _) = printable::NON_PRINTABLE.as_chunks::<2>();
    let index = pairs.partition_point(|[_, last]| *last < code);
    pairs.get(index).is_none_or(|[first, _]| code < *first)
}

/// `float.__repr__`: the shortest digits that round-trip, in fixed notation
/// when the decimal point falls within 16 digits, as `float_repr_style
/// 'short'` with `Py_DTSF_ADD_DOT_0` writes them.
pub fn float_repr(number: f64) -> String {
    if number.is_nan() {
        return "nan".into();
    }
    let sign = if number.is_sign_negative() { "-" } else { "" };
    if number.is_infinite() {
        return format!("{sign}inf");
    }
    let scientific = format!("{:e}", number.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let digits = mantissa.replace('.', "");
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let point = exponent + 1;
    let body = if point <= -4 || point > 16 {
        exponential(&digits, exponent)
    } else {
        fixed(&digits, point)
    };
    format!("{sign}{body}")
}

fn exponential(digits: &str, exponent: i32) -> String {
    let (first, rest) = digits.split_at(1);
    let fraction = if rest.is_empty() {
        String::new()
    } else {
        format!(".{rest}")
    };
    let sign = if exponent < 0 { '-' } else { '+' };
    format!("{first}{fraction}e{sign}{:02}", exponent.unsigned_abs())
}

fn fixed(digits: &str, point: i32) -> String {
    let length = i32::try_from(digits.len()).unwrap_or(i32::MAX);
    if point <= 0 {
        let zeros = "0".repeat(point.unsigned_abs() as usize);
        format!("0.{zeros}{digits}")
    } else if point >= length {
        let zeros = "0".repeat((point - length).unsigned_abs() as usize);
        format!("{digits}{zeros}.0")
    } else {
        let (whole, fraction) = digits.split_at(point.unsigned_abs() as usize);
        format!("{whole}.{fraction}")
    }
}

/// `repr(value)` for the Python object the value stands for.
pub fn py_repr(value: &Value) -> String {
    let mut out = String::new();
    write_repr(&mut out, value);
    out
}

/// `str(value)`: a string is itself, a datetime its `isoformat(' ')`, and
/// everything else its `repr`.
pub fn py_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Timestamp(at) => datetime_str(*at),
        other => py_repr(other),
    }
}

/// `str(type(value))`.
pub fn py_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "<class 'NoneType'>",
        Value::Bool(_) => "<class 'bool'>",
        Value::Integer(_) | Value::BigInteger(_) => "<class 'int'>",
        Value::Float(_) => "<class 'float'>",
        Value::String(_) => "<class 'str'>",
        Value::Blob(_) => "<class 'bytes'>",
        Value::Timestamp(_) => "<class 'datetime.datetime'>",
        Value::List(_) => "<class 'list'>",
        Value::Map(_) => "<class 'dict'>",
    }
}

fn write_repr(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("None"),
        Value::Bool(true) => out.push_str("True"),
        Value::Bool(false) => out.push_str("False"),
        Value::Integer(number) => out.push_str(&number.to_string()),
        Value::BigInteger(text) => out.push_str(text),
        Value::Float(number) => out.push_str(&float_repr(*number)),
        Value::String(text) => string_repr(out, text),
        Value::Blob(bytes) => bytes_repr(out, bytes),
        Value::Timestamp(at) => datetime_repr(out, *at),
        Value::List(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_repr(out, item);
            }
            out.push(']');
        }
        Value::Map(map) => {
            out.push('{');
            for (index, (key, item)) in map.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                string_repr(out, key);
                out.push_str(": ");
                write_repr(out, item);
            }
            out.push('}');
        }
    }
}

/// `unicode_repr`: single quotes unless the text has one and no double quote.
fn string_repr(out: &mut String, text: &str) {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    out.push(quote);
    for character in text.chars() {
        match character {
            c if c == quote || c == '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c < ' ' || c == '\u{7f}' => {
                let _ = write!(out, "\\x{:02x}", u32::from(c));
            }
            c if c.is_ascii() || is_printable(c) => out.push(c),
            c => {
                let code = u32::from(c);
                let _ = match code {
                    0..=0xff => write!(out, "\\x{code:02x}"),
                    0x100..=0xffff => write!(out, "\\u{code:04x}"),
                    _ => write!(out, "\\U{code:08x}"),
                };
            }
        }
    }
    out.push(quote);
}

/// `bytes.__repr__`.
fn bytes_repr(out: &mut String, bytes: &[u8]) {
    let quote = if bytes.contains(&b'\'') && !bytes.contains(&b'"') {
        b'"'
    } else {
        b'\''
    };
    out.push('b');
    out.push(char::from(quote));
    for &byte in bytes {
        match byte {
            b if b == quote || b == b'\\' => {
                out.push('\\');
                out.push(char::from(b));
            }
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b if !(b' '..0x7f).contains(&b) => {
                let _ = write!(out, "\\x{b:02x}");
            }
            b => out.push(char::from(b)),
        }
    }
    out.push(char::from(quote));
}

/// `datetime.__str__`: `isoformat(sep=' ')` with the `+00:00` offset.
fn datetime_str(at: Timestamp) -> String {
    let c = at.civil();
    let mut text = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        c.year, c.month, c.day, c.hour, c.minute, c.second
    );
    if c.micros != 0 {
        let _ = write!(text, ".{:06}", c.micros);
    }
    text.push_str("+00:00");
    text
}

/// `datetime.__repr__`: trailing zero microseconds, then seconds, are left out.
fn datetime_repr(out: &mut String, at: Timestamp) {
    let c = at.civil();
    let mut fields = vec![
        c.year.unsigned_abs(),
        c.month,
        c.day,
        c.hour,
        c.minute,
        c.second,
        c.micros,
    ];
    for _ in 0..2 {
        if fields.last() == Some(&0) {
            fields.pop();
        }
    }
    let fields: Vec<String> = fields.iter().map(u32::to_string).collect();
    let _ = write!(
        out,
        "datetime.datetime({}, tzinfo=datetime.timezone.utc)",
        fields.join(", ")
    );
}

#[cfg(test)]
mod tests;
