//! `float()` and `int()` of a literal. CPython first maps the text to ASCII
//! ([`ascii_literal`], `_PyUnicode_TransformDecimalAndSpaceToASCII`), then
//! strips C's six whitespace characters ([`C_SPACE`]) and reads what is
//! left ([`float`], [`int`]).

/// The ASCII whitespace `float()` and `int()` strip once the text is
/// ASCII: C's `isspace`, not the separators U+001C to U+001F that
/// `str.strip()` also removes ([`crate::text::strip`]).
pub const C_SPACE: [char; 6] = [' ', '\t', '\n', '\u{b}', '\u{c}', '\r'];

/// `float(text)` of [`ascii_literal`] text, stripped of [`C_SPACE`]: a
/// sign, then `inf`, `infinity` or `nan` in any case, or a decimal literal
/// (digits, an optional point with digits on at least one side, an
/// optional exponent) with underscores only between digits
/// (`_Py_string_to_number_with_underscores`). `None` where `float()`
/// raises ValueError.
pub fn float(text: &str) -> Option<f64> {
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let negative = text.starts_with('-');
    let magnitude = match unsigned.to_ascii_lowercase().as_str() {
        "inf" | "infinity" => f64::INFINITY,
        "nan" => f64::NAN,
        _ => decimal(unsigned)?,
    };
    Some(if negative { -magnitude } else { magnitude })
}

/// A decimal literal as `float()` reads it, underscores removed.
fn decimal(text: &str) -> Option<f64> {
    let (mantissa, exponent) = match text.find(['e', 'E']) {
        Some(at) => (&text[..at], Some(&text[at + 1..])),
        None => (text, None),
    };
    let (whole, fraction) = match mantissa.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (mantissa, None),
    };
    let has_digits = !whole.is_empty() || fraction.is_some_and(|f| !f.is_empty());
    let valid = has_digits
        && (whole.is_empty() || digit_part(whole))
        && fraction.is_none_or(|f| f.is_empty() || digit_part(f))
        && exponent.is_none_or(|e| digit_part(e.strip_prefix(['+', '-']).unwrap_or(e)));
    if !valid {
        return None;
    }
    text.replace('_', "").parse().ok()
}

/// `sys.get_int_max_str_digits()`'s default.
pub const INT_MAX_STR_DIGITS: usize = 4300;

/// The ValueError `int()` raises for a literal of `digits` digits past
/// [`INT_MAX_STR_DIGITS`].
pub fn digit_limit_message(digits: usize) -> String {
    format!(
        "Exceeds the limit ({INT_MAX_STR_DIGITS} digits) for integer string conversion: value \
         has {digits} digits; use sys.set_int_max_str_digits() to increase the limit"
    )
}

/// `int(text)` in base 10 of [`ascii_literal`] text, stripped of
/// [`C_SPACE`]: a sign and ASCII digits with single underscores between
/// them, as canonical decimal text (no leading zeros, no `-0`) of any
/// length. A caller that must refuse what
/// `sys.get_int_max_str_digits()` refuses counts [`digit_count`].
pub fn int(text: &str) -> Option<String> {
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    if !digit_part(digits) {
        return None;
    }
    let negative = text.starts_with('-');
    let digits = digits.replace('_', "");
    Some(match (negative, digits.trim_start_matches('0')) {
        (_, "") => "0".to_owned(),
        (true, magnitude) => format!("-{magnitude}"),
        (false, magnitude) => magnitude.to_owned(),
    })
}

/// The digits of a literal [`int`] accepts, leading zeros included: what
/// CPython compares with `sys.get_int_max_str_digits()`.
pub fn digit_count(text: &str) -> usize {
    text.bytes().filter(u8::is_ascii_digit).count()
}

/// `digit (_? digit)*`.
fn digit_part(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with('_')
        && !text.ends_with('_')
        && !text.contains("__")
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'_')
}

/// `_PyUnicode_TransformDecimalAndSpaceToASCII`, which `float()` and `int()`
/// apply first: ASCII as it is, each other character a space if it is
/// whitespace ([`crate::text::is_space`]) or its digit if it is a decimal
/// digit, and the text cut at the first that is neither, which it ends
/// with a `?` so the literal is invalid.
pub fn ascii_literal(text: &str) -> std::borrow::Cow<'_, str> {
    if text.chars().all(|character| u32::from(character) < 127) {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut ascii = String::with_capacity(text.len());
    for character in text.chars() {
        if u32::from(character) < 127 {
            ascii.push(character);
        } else if crate::text::is_space(character) {
            ascii.push(' ');
        } else if let Some(digit) = decimal_digit(character) {
            ascii.extend(char::from_digit(digit, 10));
        } else {
            ascii.push('?');
            break;
        }
    }
    std::borrow::Cow::Owned(ascii)
}

/// The first code point of each run of ten Unicode decimal digits
/// (`Py_UNICODE_TODECIMAL`, from CPython 3.14.8's `unicodetype_db.h`,
/// Unicode 16.0.0).
const DECIMAL_ZEROS: [u32; 76] = [
    0x30, 0x660, 0x6F0, 0x7C0, 0x966, 0x9E6, 0xA66, 0xAE6, 0xB66, 0xBE6, 0xC66, 0xCE6, 0xD66,
    0xDE6, 0xE50, 0xED0, 0xF20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80, 0x1A90,
    0x1B50, 0x1BB0, 0x1C40, 0x1C50, 0xA620, 0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0, 0xFF10,
    0x104A0, 0x10D30, 0x10D40, 0x11066, 0x110F0, 0x11136, 0x111D0, 0x112F0, 0x11450, 0x114D0,
    0x11650, 0x116C0, 0x116D0, 0x116DA, 0x11730, 0x118E0, 0x11950, 0x11BF0, 0x11C50, 0x11D50,
    0x11DA0, 0x11F50, 0x16130, 0x16A60, 0x16AC0, 0x16B50, 0x16D70, 0x1CCF0, 0x1D7CE, 0x1D7D8,
    0x1D7E2, 0x1D7EC, 0x1D7F6, 0x1E140, 0x1E2F0, 0x1E4F0, 0x1E5F1, 0x1E950, 0x1FBF0,
];

/// `Py_UNICODE_TODECIMAL(character)`.
fn decimal_digit(character: char) -> Option<u32> {
    let code = u32::from(character);
    let zero = DECIMAL_ZEROS
        .partition_point(|zero| *zero <= code)
        .checked_sub(1)?;
    let digit = code - DECIMAL_ZEROS.get(zero)?;
    (digit < 10).then_some(digit)
}
