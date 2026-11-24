//! Python's own operations on a [`Value`] that more than one part of the
//! client applies: truth, and `int()` of a `float`.
use model::value::Value;

/// `bool(value)`, as `if value:` reads it.
pub(crate) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Integer(number) => *number != 0,
        Value::BigInteger(_) | Value::Timestamp(_) => true,
        Value::Float(number) => *number != 0.0,
        Value::String(text) => !text.is_empty(),
        Value::Blob(bytes) => !bytes.is_empty(),
        Value::List(items) => !items.is_empty(),
        Value::Map(map) => !map.is_empty(),
    }
}

/// Why `int()` of a `float` fails: a `ValueError` for NaN, an
/// `OverflowError` for an infinity. Each displays CPython's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FloatToInt {
    NaN,
    Infinite,
}

impl std::fmt::Display for FloatToInt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NaN => "cannot convert float NaN to integer",
            Self::Infinite => "cannot convert float infinity to integer",
        })
    }
}

/// `int(number)` of a `float`: toward zero, of any size.
pub(crate) fn int_of_float(number: f64) -> Result<Value, FloatToInt> {
    if number.is_nan() {
        return Err(FloatToInt::NaN);
    }
    if number.is_infinite() {
        return Err(FloatToInt::Infinite);
    }
    let whole = number.trunc();
    // -2**63 and 2**63, which are exact as floats.
    if (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&whole) {
        return Ok(Value::Integer(whole as i64));
    }
    Ok(Value::BigInteger(whole_digits(whole).into()))
}

/// The decimal digits of a whole float beyond `i64`: its 53-bit mantissa
/// doubled once per binary exponent, in base 10**9.
fn whole_digits(whole: f64) -> String {
    const BASE: u64 = 1_000_000_000;
    let bits = whole.to_bits();
    let exponent = ((bits >> 52) & 0x7ff).saturating_sub(1075);
    let mantissa = (bits & ((1 << 52) - 1)) | (1 << 52);
    let mut limbs = vec![
        mantissa % BASE,
        mantissa / BASE % BASE,
        mantissa / BASE / BASE,
    ];
    for _ in 0..exponent {
        let mut carry = 0;
        for limb in &mut limbs {
            let doubled = *limb * 2 + carry;
            (*limb, carry) = (doubled % BASE, doubled / BASE);
        }
        if carry > 0 {
            limbs.push(carry);
        }
    }
    while limbs.len() > 1 && limbs.last() == Some(&0) {
        limbs.pop();
    }
    let mut digits = String::from(if whole < 0.0 { "-" } else { "" });
    for (index, limb) in limbs.iter().rev().enumerate() {
        digits.push_str(&if index == 0 {
            limb.to_string()
        } else {
            format!("{limb:09}")
        });
    }
    digits
}

#[cfg(test)]
mod tests;
