//! `value < min` with Python's exact comparison of `int` and `float`.
use super::Undecided;
use crate::shapes::Bound;
use crate::value::Value;

/// Beyond this magnitude no `f64` has a fraction and every `i64` is smaller.
const TWO_POW_63: f64 = 9_223_372_036_854_775_808.0;

/// `integer < bound`.
pub(super) fn int_below(value: i64, bound: Bound) -> bool {
    match bound {
        Bound::Integer(min) => value < min,
        Bound::Float(min) => int_below_float(value, min),
    }
}

/// `int < float`, exactly: for an integer `i`, `i < f` is `i < ceil(f)`.
fn int_below_float(value: i64, min: f64) -> bool {
    if min.is_nan() {
        return false;
    }
    let ceiling = min.ceil();
    if ceiling >= TWO_POW_63 {
        return true;
    }
    if ceiling < -TWO_POW_63 {
        return false;
    }
    // In range: the ceiling is an integer that fits in i64.
    value < ceiling as i64
}

/// `float < int`, exactly: for an integer `m`, `f < m` is `floor(f) < m`.
fn float_below_int(value: f64, min: i64) -> bool {
    if value.is_nan() {
        return false;
    }
    let floor = value.floor();
    if floor >= TWO_POW_63 {
        return false;
    }
    if floor < -TWO_POW_63 {
        return true;
    }
    (floor as i64) < min
}

/// `params < min` for a value the numeric validators admitted.
pub(super) fn value_below(value: &Value, bound: Bound) -> Result<bool, Undecided> {
    Ok(match (value, bound) {
        (Value::Bool(flag), bound) => int_below(i64::from(*flag), bound),
        (Value::Integer(integer), bound) => int_below(*integer, bound),
        (Value::Float(float), Bound::Integer(min)) => float_below_int(*float, min),
        (Value::Float(float), Bound::Float(min)) => *float < min,
        (Value::BigInteger(text), bound) => big_below(text, bound)?,
        _ => false,
    })
}

/// An integer beyond `i64` is below every bound when negative and above it
/// when positive, unless the bound itself is a float beyond 2^63.
fn big_below(text: &str, bound: Bound) -> Result<bool, Undecided> {
    if let Bound::Float(min) = bound {
        if min.is_nan() {
            return Ok(false);
        }
        if min.abs() >= TWO_POW_63 {
            return Err(Undecided::Comparison);
        }
    }
    Ok(text.starts_with('-'))
}
