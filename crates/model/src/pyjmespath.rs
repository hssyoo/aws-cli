//! [`Value`]s as the Python objects JMESPath evaluates: the
//! [`jmespath::Runtime`] that reads and builds them, and borrows Python's
//! own texts from this crate's `json`, `str()` and `int()`.
use crate::pyjson::{self, LoadError, Options};
use crate::pyrepr::{float_from_py_text, int_from_py_text, py_str, py_type_name};
use crate::value::{Key, Map, Value, Wtf8Buf};
use jmespath::{Error, Number, View};

/// The [`jmespath::Runtime`] of [`Value`]s.
#[derive(Clone, Copy, Debug, Default)]
pub struct Python;

impl jmespath::Runtime for Python {
    type Value = Value;

    fn view<'a>(&self, value: &'a Value) -> View<'a, Value> {
        match value {
            Value::Null => View::Null,
            Value::Bool(flag) => View::Bool(*flag),
            Value::Integer(int) => View::Number(Number::Int(*int)),
            Value::BigInteger(text) => {
                Number::from_int_text(text).map_or(View::Other, View::Number)
            }
            Value::Float(float) => View::Number(Number::Float(*float)),
            Value::String(text) => View::String(text.as_bytes()),
            Value::PyString(text) => View::String(text.as_bytes()),
            Value::List(items) => View::Array(items),
            Value::Map(_) => View::Object,
            Value::Blob(_) | Value::Timestamp(_) => View::Other,
        }
    }

    fn get<'a>(&self, object: &'a Value, key: &[u8]) -> Option<&'a Value> {
        let mut entries = object.as_map()?.iter();
        entries
            .find(|(name, _)| name.as_bytes() == key)
            .map(|(_, value)| value)
    }

    fn entries<'a>(&self, object: &'a Value) -> impl Iterator<Item = (&'a [u8], &'a Value)> + 'a {
        let entries = object.as_map().into_iter().flat_map(Map::iter);
        entries.map(|(key, value)| (key.as_bytes(), value))
    }

    fn null(&self) -> Value {
        Value::Null
    }

    fn boolean(&self, value: bool) -> Value {
        Value::Bool(value)
    }

    fn number(&self, value: Number) -> Value {
        match value {
            Number::Int(int) => Value::Integer(int),
            Number::Big(int) => Value::BigInteger(int.to_string().into()),
            Number::Float(float) => Value::Float(float),
        }
    }

    fn string(&self, text: Vec<u8>) -> Value {
        Value::from_text(Wtf8Buf::from_bytes(text))
    }

    fn array(&self, items: Vec<Value>) -> Value {
        Value::List(items)
    }

    fn object(&self, entries: Vec<(Vec<u8>, Value)>) -> Value {
        let entries = entries.into_iter();
        Value::Map(
            entries
                .map(|(key, value)| (Key::from(Wtf8Buf::from_bytes(key)), value))
                .collect(),
        )
    }

    fn loads(&self, text: &str) -> Result<Value, String> {
        pyjson::loads(text).map_err(|error| match error {
            LoadError::Invalid(message) => message,
            LoadError::Limit(limit) => limit.to_string(),
        })
    }

    fn type_name(&self, value: &Value) -> &'static str {
        py_type_name(value)
    }

    fn str(&self, value: &Value) -> String {
        py_str(value)
    }

    fn dumps(&self, value: &Value) -> String {
        let fallback = |value: &Value| Ok(Value::String(py_str(value)));
        pyjson::dumps(value, &Options::compact(), Some(&fallback)).unwrap_or_default()
    }

    fn other_eq(&self, a: &Value, b: &Value) -> bool {
        a == b
    }

    fn other_truthy(&self, value: &Value) -> bool {
        !matches!(value, Value::Blob(bytes) if bytes.is_empty())
    }

    fn to_number(&self, value: &Value) -> Result<Option<Number>, Error> {
        let text = match value {
            Value::String(text) => text.as_bytes(),
            Value::PyString(text) => text.as_bytes(),
            // `int(bytes)` reads ASCII only.
            Value::Blob(bytes) if bytes.is_ascii() => bytes,
            Value::Blob(_) => return Ok(None),
            other => {
                return Err(Error::python(
                    "TypeError",
                    &format!(
                        "int() argument must be a string, a bytes-like object or a real number, \
                         not '{}'",
                        py_type_name(other)
                    ),
                ));
            }
        };
        Ok(match int_from_py_text(text) {
            Ok(Value::Integer(int)) => Some(Number::Int(int)),
            Ok(Value::BigInteger(int)) => Number::from_int_text(&int),
            _ => float_from_py_text(text).map(Number::Float),
        })
    }
}

#[cfg(test)]
mod tests;
