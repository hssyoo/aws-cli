//! `JSONFormatter`: `json.dump(response, indent=4, default=json_encoder,
//! ensure_ascii=False)` and a newline, nothing for an empty response.
use crate::stream::Encoded;
use crate::text::PythonError;
use model::pyjson::{DumpError, Options, dumps_wtf8};
use model::value::Value;

/// What `JSONFormatter._format_response` writes for `response`, to `out`.
///
/// # Errors
/// The `UnicodeEncodeError` of a lone surrogate `out` cannot write, after
/// the writes before it.
pub fn json(response: &Value, out: &mut Encoded) -> Result<(), PythonError> {
    if matches!(response, Value::Map(map) if map.is_empty()) {
        return Ok(());
    }
    out.write(&dump(response))?;
    out.write(b"\n")
}

/// `json.dumps(value, indent=4, default=json_encoder, ensure_ascii=False)`,
/// in `pytext::wtf8`'s encoding.
pub fn dump(value: &Value) -> Vec<u8> {
    let options = Options {
        ensure_ascii: false,
        ..Options::indented(4)
    };
    // json_encoder converts the only two values JSON cannot hold, so the
    // dump cannot fail.
    dumps_wtf8(value, &options, Some(&json_encoder)).unwrap_or_default()
}

/// awscli's `utils.json_encoder`: a datetime as its `isoformat()` (an
/// aware UTC one, so with `+00:00`), bytes as their base64 text.
fn json_encoder(value: &Value) -> Result<Value, DumpError> {
    match value {
        Value::Timestamp(at) => Ok(Value::String(
            at.zoned(0)
                .map(|zoned| zoned.isoformat())
                .unwrap_or_default(),
        )),
        Value::Blob(bytes) => Ok(Value::String(pytext::base64::encode(bytes))),
        other => Err(DumpError::unserializable(other)),
    }
}

#[cfg(test)]
mod tests;
