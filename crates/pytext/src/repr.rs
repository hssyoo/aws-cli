//! `repr()` of Python's `str`, `bytes` and `float`, and `str.isprintable`.
use std::fmt::Write as _;

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

/// `unicode_repr`'s quote: a double quote only when the text holds a
/// single quote and no double quote.
pub fn quote_for(text: &[u8]) -> char {
    if text.contains(&b'\'') && !text.contains(&b'"') {
        '"'
    } else {
        '\''
    }
}

/// `text` as it appears between `quote`s in its repr: a character outside
/// ASCII as is when printable, else `\xNN`, `\uNNNN` or `\UNNNNNNNN`.
pub fn write_escaped(out: &mut String, text: &str, quote: char) {
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
}

/// `repr(text)` for a `str`.
pub fn str_repr(text: &str) -> String {
    let quote = quote_for(text.as_bytes());
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    write_escaped(&mut out, text, quote);
    out.push(quote);
    out
}

/// `bytes.__repr__`: `b` and the quoted value, every byte outside
/// printable ASCII as `\xNN`.
pub fn bytes_repr(bytes: &[u8]) -> String {
    let quote = quote_for(bytes);
    let mut out = String::with_capacity(bytes.len() + 3);
    out.push('b');
    out.push(quote);
    for &byte in bytes {
        match byte {
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b if char::from(b) == quote || b == b'\\' => {
                out.push('\\');
                out.push(char::from(b));
            }
            b if !(b' '..0x7f).contains(&b) => {
                let _ = write!(out, "\\x{b:02x}");
            }
            b => out.push(char::from(b)),
        }
    }
    out.push(quote);
    out
}

#[cfg(test)]
mod tests;
