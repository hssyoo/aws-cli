//! argparse's `type=int` and `type=float` conversion (`_get_value`), which
//! a model operation's table applies while it parses: `CLIArgument` gives
//! `long` members `int` and `float`/`double` members `float`, and
//! `PageArgument` gives the integer page options `int`.
//!
//! It is not the pinned tables' `CliType::Integer`: that is a later step,
//! the unpack loop's `int()` of a value argparse stored as a string, with
//! its own `invalid literal for int()` failure (`unpack`). An argument has
//! one or the other.
//!
//! Text is converted as Python's `int()` and `float()` read ASCII text. Any
//! other text, Python reads through Unicode tables (other digits, other
//! spaces), and a value that is not Unicode decodes to surrogates; both are
//! [`Converted::Undecided`], which the parse hands to Python.
use crate::unpack::{PYTHON_INT_MAX_STR_DIGITS, literal_digits};
use std::ffi::OsStr;

/// The conversion an argument's value goes through while it is parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    Int,
    Float,
}
impl ValueType {
    /// The type's `__name__`, which argparse's failure names.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Int => "int",
            Self::Float => "float",
        }
    }
}

/// What converting one value gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Converted {
    /// The converted value as a JSON number's text: an `int` in canonical
    /// decimal, any magnitude; a `float`, the double Python's correctly
    /// rounded conversion gives, in the shortest digits that read back
    /// exactly, or as `json.dumps` spells a value that is not finite
    /// (`Infinity`, `-Infinity`, `NaN`). `str::parse::<f64>` reads every
    /// one back. Digits are not `float.__repr__`'s (`1e16`, not `1e+16`).
    Value(String),
    /// `int()` or `float()` raises ValueError: argparse's
    /// `invalid <type> value: <repr>`.
    Invalid,
    /// Python reads the text through tables native does not model.
    Undecided,
}

/// `value_type(text)`, as argparse calls it.
pub fn convert(value_type: ValueType, text: &OsStr) -> Converted {
    let Some(text) = text.to_str().filter(|text| text.is_ascii()) else {
        return Converted::Undecided;
    };
    // int() and float() strip C's six whitespace characters from ASCII
    // text, not the separators U+001C to U+001F that `str.strip()` removes.
    let body = text.trim_matches([' ', '\t', '\n', '\u{b}', '\u{c}', '\r']);
    let converted = match value_type {
        ValueType::Int => int(body),
        ValueType::Float => float(body),
    };
    converted.map_or(Converted::Invalid, Converted::Value)
}

fn split_sign(text: &str) -> (bool, &str) {
    match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    }
}

/// `int(text)` in base 10: a sign, digits with single underscores between
/// them, leading zeros allowed, and at most `sys.get_int_max_str_digits()`
/// digits (a longer literal is a ValueError).
fn int(text: &str) -> Option<String> {
    let (negative, digits) = split_sign(text);
    let count = literal_digits(digits)?;
    if count > PYTHON_INT_MAX_STR_DIGITS {
        return None;
    }
    let digits: String = digits.chars().filter(char::is_ascii_digit).collect();
    let magnitude = digits.trim_start_matches('0');
    Some(match (negative, magnitude) {
        (_, "") => "0".to_owned(),
        (true, magnitude) => format!("-{magnitude}"),
        (false, magnitude) => magnitude.to_owned(),
    })
}

/// `float(text)`: `inf`, `infinity` or `nan` in any case after a sign, or
/// a decimal literal (digits, an optional point with optional digits on
/// either side but some digits, an optional exponent), with underscores
/// only between two digits (`_Py_string_to_number_with_underscores`).
fn float(text: &str) -> Option<String> {
    let (negative, rest) = split_sign(text);
    let special = match rest.to_ascii_lowercase().as_str() {
        "inf" | "infinity" => Some(f64::INFINITY),
        "nan" => Some(f64::NAN),
        _ => None,
    };
    if let Some(value) = special {
        return Some(json_float(if negative { -value } else { value }));
    }
    if !underscores_between_digits(text) {
        return None;
    }
    let rest: String = rest.chars().filter(|c| *c != '_').collect();
    let (mantissa, exponent) = match rest.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (rest.as_str(), None),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    if (whole.is_empty() && fraction.is_empty()) || !digits(whole) || !digits(fraction) {
        return None;
    }
    let exponent = match exponent {
        None => "0",
        Some(exponent) => {
            let (_, magnitude) = split_sign(exponent);
            (!magnitude.is_empty() && digits(magnitude)).then_some(exponent)?
        }
    };
    let sign = if negative { "-" } else { "" };
    let normalized = format!("{sign}0{whole}.{fraction}0e{exponent}");
    normalized.parse().ok().map(json_float)
}

/// A double as JSON number text, the non-finite ones as `json.dumps`
/// writes them.
fn json_float(value: f64) -> String {
    match value {
        value if value.is_nan() => "NaN".to_owned(),
        f64::INFINITY => "Infinity".to_owned(),
        f64::NEG_INFINITY => "-Infinity".to_owned(),
        value => format!("{value:?}"),
    }
}

fn underscores_between_digits(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().all(|(at, byte)| {
        *byte != b'_'
            || (at > 0
                && bytes[at - 1].is_ascii_digit()
                && bytes.get(at + 1).is_some_and(u8::is_ascii_digit))
    })
}

#[cfg(test)]
mod tests;
