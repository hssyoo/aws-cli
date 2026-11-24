//! The tokens the paginator hands out and takes back: `TokenEncoder`,
//! `TokenDecoder`, `PageIterator._parse_starting_token` and the
//! `resume_token` setter.
use super::loads::{LoadError, loads};
use super::py::{self, type_name};
use crate::admission::Unsupported;
use crate::error::CallError;
use crate::error::PaginationError;
use model::pyjson::{self, Options};
use model::pyrepr::py_repr;
use model::value::{Map, Value};

/// The key a truncated page's token carries its offset in.
pub(super) const TRUNCATE_AMOUNT: &str = "boto_truncate_amount";
const ENCODED_KEYS: &str = "boto_encoded_keys";

/// `TokenEncoder.encode`: `json.dumps(token)`, base64. A token `json`
/// cannot hold has its bytes base64 encoded in place and their paths
/// listed under `boto_encoded_keys`.
///
/// # Errors
/// The second `json.dumps`' `TypeError`, for a token holding a datetime.
pub(super) fn encode(token: &Map) -> Result<String, String> {
    let options = Options::default();
    let json = match pyjson::dumps(&Value::Map(token.clone()), &options, None) {
        Ok(json) => json,
        Err(_) => {
            let mut keys = Vec::new();
            let mut encoded = encode_bytes(&Value::Map(token.clone()), &mut Vec::new(), &mut keys);
            if let Value::Map(map) = &mut encoded {
                map.insert(ENCODED_KEYS.to_owned(), Value::List(keys));
            }
            pyjson::dumps(&encoded, &options, None).map_err(|error| error.to_string())?
        }
    };
    Ok(model::base64::encode(json.as_bytes()))
}

/// `TokenEncoder._encode`: `data` with each `bytes` base64 encoded, the path
/// to each pushed onto `encoded`.
fn encode_bytes(data: &Value, path: &mut Vec<Value>, encoded: &mut Vec<Value>) -> Value {
    match data {
        Value::Map(map) => Value::Map(
            map.iter()
                .map(|(key, value)| {
                    path.push(Value::String(key.to_owned()));
                    let value = encode_bytes(value, path, encoded);
                    path.pop();
                    (key.to_owned(), value)
                })
                .collect(),
        ),
        Value::List(items) => Value::List(
            items
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    path.push(Value::Integer(i64::try_from(index).unwrap_or(i64::MAX)));
                    let value = encode_bytes(value, path, encoded);
                    path.pop();
                    value
                })
                .collect(),
        ),
        Value::Blob(bytes) => {
            encoded.push(Value::List(path.clone()));
            Value::String(model::base64::encode(bytes))
        }
        other => other.clone(),
    }
}

/// How `TokenDecoder.decode` fails.
#[derive(Debug, PartialEq, Eq)]
enum DecodeError {
    /// A `ValueError` or `TypeError`, which `_parse_starting_token` takes
    /// to mean a token of the old form.
    Deprecated,
    /// Any other exception, which `_parse_starting_token` lets through.
    Raised(String),
    /// JSON the native values cannot hold as Python does: the Python
    /// route's to read.
    Unsupported(&'static str),
}

/// `TokenDecoder.decode`: base64, then a JSON object, then each path
/// `boto_encoded_keys` lists decoded from base64 to bytes.
fn decode(token: &str) -> Result<Map, DecodeError> {
    let bytes = model::base64::decode(token).map_err(|_| DecodeError::Deprecated)?;
    let text = String::from_utf8(bytes).map_err(|_| DecodeError::Deprecated)?;
    let mut decoded = match loads(&text) {
        Ok(Value::Map(map)) => map,
        // `list.pop('boto_encoded_keys', None)` is a `TypeError`.
        Ok(Value::List(_)) | Err(LoadError::Invalid) => return Err(DecodeError::Deprecated),
        Err(LoadError::Unsupported(why)) => return Err(DecodeError::Unsupported(why)),
        Ok(other) => {
            return Err(DecodeError::Raised(format!(
                "'{}' object has no attribute 'pop'",
                type_name(&other)
            )));
        }
    };
    match decoded.remove(ENCODED_KEYS) {
        None | Some(Value::Null) => Ok(decoded),
        Some(keys) => {
            let mut token = Value::Map(decoded);
            for path in iterate(&keys)? {
                decode_path(&mut token, &path)?;
            }
            match token {
                Value::Map(map) => Ok(map),
                _ => Err(DecodeError::Deprecated),
            }
        }
    }
}

/// `for item in value`: a list's items, a dict's keys, a str's characters.
fn iterate(value: &Value) -> Result<Vec<Value>, DecodeError> {
    match value {
        Value::List(items) => Ok(items.clone()),
        Value::Map(map) => Ok(map.keys().map(|key| Value::String(key.into())).collect()),
        Value::String(text) => Ok(text.chars().map(|c| Value::String(c.into())).collect()),
        _ => Err(DecodeError::Deprecated),
    }
}

/// `TokenDecoder._decode` of one path: the str there, base64 decoded to
/// bytes, set in its place (`_path_get`, `_path_set`).
fn decode_path(token: &mut Value, path: &Value) -> Result<(), DecodeError> {
    let steps = iterate(path)?;
    let encoded = steps
        .iter()
        .try_fold(token.clone(), |data, step| item(&data, step))?;
    let Value::String(text) = &encoded else {
        return Err(DecodeError::Raised(format!(
            "'{}' object has no attribute 'encode'",
            type_name(&encoded)
        )));
    };
    let bytes = model::base64::decode(text).map_err(|_| DecodeError::Deprecated)?;
    // `path[:-1]` of a dict looks the slice up, a `KeyError`.
    if matches!(path, Value::Map(_)) {
        return Err(DecodeError::Raised("slice(None, -1, None)".into()));
    }
    let Some((last, parents)) = steps.split_last() else {
        return Err(DecodeError::Raised("list index out of range".into()));
    };
    let container = parents
        .iter()
        .try_fold(token, |data, step| item_mut(data, step))?;
    match container {
        Value::Map(map) => {
            if let Value::String(key) = last {
                map.insert(key.clone(), Value::Blob(bytes));
            }
        }
        Value::List(items) => {
            let index = list_index(items.len(), last)?;
            items[index] = Value::Blob(bytes);
        }
        _ => return Err(DecodeError::Deprecated),
    }
    Ok(())
}

/// `data[step]`: a str's item is a str of its one character.
fn item(data: &Value, step: &Value) -> Result<Value, DecodeError> {
    match data {
        Value::Map(map) => {
            let key = map_key(step)?;
            map.get(key)
                .cloned()
                .ok_or_else(|| DecodeError::Raised(py_repr(step)))
        }
        Value::List(items) => Ok(items[list_index(items.len(), step)?].clone()),
        Value::String(text) => {
            let index = sequence_index(text.chars().count(), step, "string")?;
            let character = text.chars().nth(index).map(String::from);
            Ok(Value::String(character.unwrap_or_default()))
        }
        _ => Err(DecodeError::Deprecated),
    }
}

/// `data[step]`, to set in place.
fn item_mut<'v>(data: &'v mut Value, step: &Value) -> Result<&'v mut Value, DecodeError> {
    match data {
        Value::Map(map) => map_key(step).and_then(|key| {
            map.get_mut(key)
                .ok_or_else(|| DecodeError::Raised(py_repr(step)))
        }),
        Value::List(items) => {
            let index = list_index(items.len(), step)?;
            Ok(&mut items[index])
        }
        _ => Err(DecodeError::Deprecated),
    }
}

/// The key `step` looks up in a dict of JSON's string keys: any other
/// hashable one is a `KeyError`, an unhashable one a `TypeError`.
fn map_key(step: &Value) -> Result<&str, DecodeError> {
    match step {
        Value::String(key) => Ok(key),
        Value::List(_) | Value::Map(_) => Err(DecodeError::Deprecated),
        other => Err(DecodeError::Raised(py_repr(other))),
    }
}

fn list_index(len: usize, step: &Value) -> Result<usize, DecodeError> {
    sequence_index(len, step, "list")
}

/// The index `step` is in a sequence of `len`: an `IndexError` out of
/// range, a `TypeError` for a step that is no `int`.
fn sequence_index(len: usize, step: &Value, kind: &str) -> Result<usize, DecodeError> {
    let index = match step {
        Value::Integer(_) | Value::Bool(_) | Value::BigInteger(_) => py::int_number(step),
        _ => return Err(DecodeError::Deprecated),
    };
    let len = i128::try_from(len).unwrap_or(i128::MAX);
    let index = if index < 0 { index + len } else { index };
    if (0..len).contains(&index) {
        return usize::try_from(index).map_err(|_| DecodeError::Deprecated);
    }
    Err(DecodeError::Raised(format!("{kind} index out of range")))
}

/// A starting token as `_parse_starting_token` reads it: the input tokens
/// to send, and how far into the first page's primary result key to start.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct StartingToken {
    pub(super) token: Map,
    /// `boto_truncate_amount`, as the token gave it (0 when absent).
    pub(super) index: Value,
}

/// `_parse_starting_token`: the token [`decode`]s, or else it is of the old
/// form (`_parse_starting_token_deprecated`).
///
/// # Errors
/// `ValueError("Bad starting token: ...")`, an exception the decoder
/// raises that is neither a `ValueError` nor a `TypeError`, or
/// [`Unsupported::StartingToken`] for a token only Python can read.
pub(super) fn parse_starting_token(
    raw: &str,
    input_token: &[String],
) -> Result<StartingToken, CallError> {
    match decode(raw) {
        Ok(mut token) => {
            let index = token.remove(TRUNCATE_AMOUNT).unwrap_or(Value::Integer(0));
            Ok(StartingToken { token, index })
        }
        Err(DecodeError::Raised(text)) => Err(PaginationError::Other(text).into()),
        Err(DecodeError::Unsupported(why)) => {
            Err(CallError::Unsupported(Unsupported::StartingToken(why)))
        }
        Err(DecodeError::Deprecated) => Ok(deprecated(raw, input_token)?),
    }
}

/// `_parse_starting_token_deprecated`: the input tokens joined by `___`, an
/// `int` after the last of them the offset, `None` for a token not sent
/// (`_convert_deprecated_starting_token`).
fn deprecated(raw: &str, input_token: &[String]) -> Result<StartingToken, PaginationError> {
    let mut parts: Vec<&str> = raw.split("___").collect();
    let mut index = Value::Integer(0);
    if parts.len() == input_token.len() + 1 {
        match parts.pop().map(py::int_of_str) {
            Some(Ok(number)) => index = number,
            _ => parts = vec![raw],
        }
    }
    if parts.len() > input_token.len() {
        return Err(PaginationError::Value(format!("Bad starting token: {raw}")));
    }
    let values = parts
        .into_iter()
        .map(|part| match part {
            "None" => Value::Null,
            part => Value::String(part.to_owned()),
        })
        .chain(std::iter::repeat(Value::Null));
    let token = input_token.iter().cloned().zip(values).collect();
    Ok(StartingToken { token, index })
}

/// The `resume_token` setter: `value` encoded, once its keys are exactly the
/// input tokens and, if it has one, `boto_truncate_amount`.
///
/// # Errors
/// `ValueError("Bad starting token: ...")` for other keys, and
/// [`encode`]'s error.
pub(super) fn resume_token(value: &Map, input_token: &[String]) -> Result<String, PaginationError> {
    let mut expected: Vec<&str> = input_token.iter().map(String::as_str).collect();
    if value.contains_key(TRUNCATE_AMOUNT) {
        expected.push(TRUNCATE_AMOUNT);
    }
    expected.sort_unstable();
    let mut keys: Vec<&str> = value.keys().collect();
    keys.sort_unstable();
    if expected != keys {
        return Err(PaginationError::Value(format!(
            "Bad starting token: {}",
            py_repr(&Value::Map(value.clone()))
        )));
    }
    encode(value).map_err(PaginationError::Other)
}

#[cfg(test)]
mod tests;
