//! `JSONFormatter`: `json.dump(response, indent=4, default=json_encoder,
//! ensure_ascii=False)` and a newline, nothing for an empty response.
use crate::stream::Encoded;
use crate::text::PythonError;
use model::pyjson::{ChunkError, DumpError, Options, dump_chunks};
use model::value::Value;

/// What `JSONFormatter._format_response` writes for `response`, to `out`:
/// `json.dump`'s chunks, each its own write, then the newline.
///
/// # Errors
/// The `UnicodeEncodeError` of a lone surrogate `out` cannot write, after
/// the writes before it.
pub fn json(response: &Value, out: &mut Encoded) -> Result<(), PythonError> {
    if matches!(response, Value::Map(map) if map.is_empty()) {
        return Ok(());
    }
    let options = Options {
        ensure_ascii: false,
        ..Options::indented(4)
    };
    match dump_chunks(response, &options, Some(&json_encoder), &mut |chunk| {
        out.write(chunk)
    }) {
        Ok(()) => {}
        Err(ChunkError::Write(error)) => return Err(error),
        // json_encoder converts the only two values JSON cannot hold.
        Err(ChunkError::Dump(error)) => {
            return Err(PythonError {
                message: error.to_string(),
            });
        }
    }
    out.write(b"\n")
}

/// awscli's `utils.json_encoder`: a datetime as its `isoformat()` (an
/// aware UTC one, so with `+00:00`), bytes as their base64 text.
///
/// # Errors
/// `JSONEncoder.default`'s `TypeError` for any other value.
pub fn json_encoder(value: &Value) -> Result<Value, DumpError> {
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
