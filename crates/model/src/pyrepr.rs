//! Python's `repr()` and `str()` of a [`Value`], as botocore's error reports
//! print parameter values, and `str(type(value))`.
use crate::value::Value;
use std::fmt::Write as _;
use timestamp::Timestamp;

use pytext::number;
use pytext::repr::{bytes_repr, str_repr};
pub use pytext::repr::{float_repr, is_printable};

/// What CPython's `float()` and `int()` strip before a literal: C's six
/// from ASCII ([`pytext::number::C_SPACE`]), not the separators U+001C to
/// U+001F that `str.strip()` also removes, and Unicode whitespace beyond
/// ASCII (`_PyUnicode_TransformDecimalAndSpaceToASCII`). Unicode's
/// `White_Space` is exactly that set.
fn stripped(text: &str) -> &str {
    text.trim_matches(char::is_whitespace)
}

/// `float(text)`: [`pytext::number::float`] after [`stripped`].
pub fn float_from_text(text: &str) -> Option<f64> {
    number::float(stripped(text))
}

/// `int(text)` in base 10, after [`stripped`]: an `Integer` or, beyond
/// `i64`, a `BigInteger`, of at most `sys.get_int_max_str_digits()` digits.
/// `Err` holds the ValueError's text. Python also reads non-ASCII decimal
/// digits; those are refused here.
pub fn int_from_text(text: &str) -> Result<Value, String> {
    let body = stripped(text);
    let canonical = number::int(body)
        .ok_or_else(|| format!("invalid literal for int() with base 10: {}", str_repr(text)))?;
    let digits = number::digit_count(body);
    if digits > number::INT_MAX_STR_DIGITS {
        return Err(number::digit_limit_message(digits));
    }
    Ok(match canonical.parse::<i64>() {
        Ok(value) => Value::Integer(value),
        Err(_) => Value::BigInteger(canonical.into()),
    })
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

/// The type's name as Python's own messages spell it (`tp_name`):
/// `datetime.datetime` for a timestamp, the builtin's name otherwise.
pub fn py_type_name(value: &Value) -> &'static str {
    py_type(value)
        .trim_start_matches("<class '")
        .trim_end_matches("'>")
}

/// `type(value).__name__`: [`py_type_name`] without its module.
pub fn py_short_type_name(value: &Value) -> &'static str {
    let name = py_type_name(value);
    name.rsplit('.').next().unwrap_or(name)
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
        Value::String(text) => out.push_str(&str_repr(text)),
        Value::Blob(bytes) => out.push_str(&bytes_repr(bytes)),
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
                out.push_str(&str_repr(key));
                out.push_str(": ");
                write_repr(out, item);
            }
            out.push('}');
        }
    }
}

/// `datetime.__str__`: `isoformat(sep=' ')` with the `+00:00` offset.
fn datetime_str(at: Timestamp) -> String {
    let c = at.civil();
    let mut text = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        c.year, c.month, c.day, c.hour, c.minute, c.second
    );
    if c.microsecond != 0 {
        let _ = write!(text, ".{:06}", c.microsecond);
    }
    text.push_str("+00:00");
    text
}

/// `datetime.__repr__`: trailing zero microseconds, then seconds, are left out.
fn datetime_repr(out: &mut String, at: Timestamp) {
    let c = at.civil();
    let mut fields = vec![
        c.year.unsigned_abs(),
        u32::from(c.month),
        u32::from(c.day),
        u32::from(c.hour),
        u32::from(c.minute),
        u32::from(c.second),
        c.microsecond,
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
