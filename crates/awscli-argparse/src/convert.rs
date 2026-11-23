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
use crate::unpack::PYTHON_INT_MAX_STR_DIGITS;
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
    let body = text.trim_matches(pytext::number::C_SPACE);
    let converted = match value_type {
        ValueType::Int => int(body),
        ValueType::Float => float(body),
    };
    converted.map_or(Converted::Invalid, Converted::Value)
}

/// `int(text)` in base 10: [`pytext::number::int`], and at most
/// `sys.get_int_max_str_digits()` digits (a longer literal is a
/// ValueError).
fn int(text: &str) -> Option<String> {
    let canonical = pytext::number::int(text)?;
    (pytext::number::digit_count(text) <= PYTHON_INT_MAX_STR_DIGITS).then_some(canonical)
}

/// `float(text)`: [`pytext::number::float`], as JSON number text.
fn float(text: &str) -> Option<String> {
    pytext::number::float(text).map(json_float)
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

#[cfg(test)]
mod tests;
