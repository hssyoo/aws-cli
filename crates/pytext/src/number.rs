//! `float()` and `int()` of a literal, after the caller strips the text as
//! its Python reads it. CPython strips C's six whitespace characters from
//! ASCII and maps other Unicode whitespace and decimal digits to ASCII
//! first (`_PyUnicode_TransformDecimalAndSpaceToASCII`); each caller states
//! what it strips and what it leaves to Python.

/// The ASCII whitespace `float()` and `int()` strip: C's `isspace`, not
/// the separators U+001C to U+001F that `str.strip()` also removes.
pub const C_SPACE: [char; 6] = [' ', '\t', '\n', '\u{b}', '\u{c}', '\r'];

/// `float(text)` of stripped text: a sign, then `inf`, `infinity` or `nan`
/// in any case, or a decimal literal (digits, an optional point with
/// digits on at least one side, an optional exponent) with underscores
/// only between digits (`_Py_string_to_number_with_underscores`). `None`
/// where `float()` raises ValueError.
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

/// `int(text)` in base 10 of stripped text: a sign and ASCII digits with
/// single underscores between them, as canonical decimal text (no leading
/// zeros, no `-0`) of any length. A caller that must refuse what
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
pub fn digit_part(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with('_')
        && !text.ends_with('_')
        && !text.contains("__")
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'_')
}
