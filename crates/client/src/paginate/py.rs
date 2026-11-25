//! The Python operations `PageIterator` applies to a page's values, each
//! with the exception text CPython raises where it fails: truth, `len`,
//! slicing, `+`, `==` and `int()`. `Err` holds the text.
use crate::error::PaginationError;
use crate::python::FloatToInt;
use model::value::{Map, Value};

/// The type's name as CPython's messages print it (`tp_name`).
pub(super) use model::pyrepr::py_type_name as type_name;

/// `bool(value)`, `None` for a value that is not there.
pub(super) fn truthy(value: Option<&Value>) -> bool {
    value.is_some_and(crate::python::truthy)
}

/// `len(value)`: a `str` counts code points.
pub(super) fn len(value: &Value) -> Result<i128, String> {
    let count = match value {
        Value::List(items) => items.len(),
        Value::Map(map) => map.len(),
        Value::Blob(bytes) => bytes.len(),
        Value::String(text) => text.chars().count(),
        other => {
            return Err(format!(
                "object of type '{}' has no len()",
                type_name(other)
            ));
        }
    };
    Ok(i128::try_from(count).unwrap_or(i128::MAX))
}

/// An `int`'s value; one beyond `i128` saturates.
fn int_of(value: &Value) -> Option<i128> {
    match value {
        Value::Integer(number) => Some(i128::from(*number)),
        Value::Bool(flag) => Some(i128::from(*flag)),
        Value::BigInteger(text) => Some(text.parse().unwrap_or(if text.starts_with('-') {
            i128::MIN
        } else {
            i128::MAX
        })),
        _ => None,
    }
}

/// An `int` of `number`, beyond `i64` as its decimal text.
pub(super) fn int_value(number: i128) -> Value {
    i64::try_from(number).map_or_else(
        |_| Value::BigInteger(number.to_string().into()),
        Value::Integer,
    )
}

/// The index a slice bound `index` stands for in a sequence of `len`
/// (`PySlice_AdjustIndices`).
fn bound(index: i128, len: usize) -> usize {
    let len = i128::try_from(len).unwrap_or(i128::MAX);
    let index = if index < 0 { index + len } else { index };
    usize::try_from(index.clamp(0, len)).unwrap_or(0)
}

/// Where `data[start:]` of a sequence of `len` starts.
pub(super) fn slice_start(start: &Value, len: usize) -> Result<usize, String> {
    let start = match start {
        Value::Null => 0,
        other => int_of(other).ok_or_else(|| {
            "slice indices must be integers or None or have an __index__ method".to_owned()
        })?,
    };
    Ok(bound(start, len))
}

/// Where `data[:stop]` of a sequence of `len` ends.
pub(super) fn slice_stop(stop: i128, len: usize) -> usize {
    bound(stop, len)
}

/// `data[start:]` of a `str`; anything else is itself.
pub(super) fn slice_from(data: &Value, start: &Value) -> Result<Value, String> {
    Ok(match data {
        Value::String(text) => {
            let start = slice_start(start, text.chars().count())?;
            Value::String(text.chars().skip(start).collect())
        }
        other => other.clone(),
    })
}

/// `data[:stop]` of a `str` or `bytes`.
pub(super) fn slice_to(data: &Value, stop: i128) -> Result<Value, String> {
    Ok(match data {
        Value::String(text) => {
            let end = slice_stop(stop, text.chars().count());
            Value::String(text.chars().take(end).collect())
        }
        Value::Blob(bytes) => Value::Blob(bytes[..slice_stop(stop, bytes.len())].to_vec()),
        other => {
            return Err(format!(
                "'{}' object is not subscriptable",
                type_name(other)
            ));
        }
    })
}

/// `left + right` for an `int` `left`.
pub(super) fn int_add(left: i128, right: &Value) -> Result<Value, String> {
    add(&int_value(left), right)
}

fn unsupported(left: &Value, right: &Value) -> String {
    format!(
        "unsupported operand type(s) for +: '{}' and '{}'",
        type_name(left),
        type_name(right)
    )
}

/// A number as a `float`, for `+` and `==` across `int` and `float`.
#[allow(
    clippy::cast_precision_loss,
    reason = "Python converts the int to a float"
)]
fn float_of(value: &Value) -> Option<f64> {
    match value {
        Value::Float(number) => Some(*number),
        other => int_of(other).map(|number| number as f64),
    }
}

/// `left + right`, as `build_full_result` adds a scalar result key's values.
pub(super) fn add(left: &Value, right: &Value) -> Result<Value, String> {
    if let (Some(left), Some(right)) = (int_of(left), int_of(right)) {
        return Ok(int_value(left.saturating_add(right)));
    }
    if let (Some(left), Some(right)) = (float_of(left), float_of(right)) {
        return Ok(Value::Float(left + right));
    }
    match (left, right) {
        (Value::String(left), Value::String(right)) => Ok(Value::String(format!("{left}{right}"))),
        (Value::String(_) | Value::List(_), other) => Err(format!(
            "can only concatenate {} (not \"{}\") to {}",
            type_name(left),
            type_name(other),
            type_name(left)
        )),
        _ => Err(unsupported(left, right)),
    }
}

/// `left == right`: numbers compare by value across `bool`, `int` and
/// `float`, and a `dict` regardless of its order.
pub(super) fn eq(left: &Value, right: &Value) -> bool {
    if let (Some(left), Some(right)) = (int_of(left), int_of(right)) {
        return left == right;
    }
    if let (Some(left), Some(right)) = (float_of(left), float_of(right)) {
        return left == right;
    }
    match (left, right) {
        (Value::List(left), Value::List(right)) => {
            left.len() == right.len() && left.iter().zip(right).all(|(l, r)| eq(l, r))
        }
        (Value::Map(left), Value::Map(right)) => map_eq(left, right),
        _ => left == right,
    }
}

/// `left == right` of two `dict`s.
pub(super) fn map_eq(left: &Map, right: &Map) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .all(|(key, value)| right.get_key(key).is_some_and(|other| eq(value, other)))
}

/// `int(value)`, as `_extract_paging_params` reads `MaxItems` and
/// `PageSize`.
///
/// # Errors
/// What `int()` raises.
pub(super) fn int(value: &Value) -> Result<Value, PaginationError> {
    match value {
        Value::Integer(_) | Value::BigInteger(_) => Ok(value.clone()),
        Value::Bool(flag) => Ok(Value::Integer(i64::from(*flag))),
        Value::String(text) => model::pyrepr::int_from_text(text).map_err(PaginationError::Value),
        Value::Float(number) => crate::python::int_of_float(*number).map_err(|error| match error {
            FloatToInt::NaN => PaginationError::Value(error.to_string()),
            FloatToInt::Infinite => PaginationError::Other(error.to_string()),
        }),
        other => Err(PaginationError::Other(format!(
            "int() argument must be a string, a bytes-like object or a real number, not '{}'",
            type_name(other)
        ))),
    }
}

/// The value of an `int` from [`int`], beyond `i128` saturated.
pub(super) fn int_number(value: &Value) -> i128 {
    int_of(value).unwrap_or(0)
}
