//! `JSONFormatter`: `json.dump(response, indent=4, default=json_encoder,
//! ensure_ascii=False)` and a newline, nothing for an empty response.
use model::pyjson::{DumpError, Options, dumps};
use model::value::Value;

/// What `JSONFormatter._format_response` writes for `response`.
pub fn json(response: &Value) -> String {
    if matches!(response, Value::Map(map) if map.is_empty()) {
        return String::new();
    }
    let mut text = dump(response);
    text.push('\n');
    text
}

/// `json.dumps(value, indent=4, default=json_encoder, ensure_ascii=False)`.
pub fn dump(value: &Value) -> String {
    let options = Options {
        ensure_ascii: false,
        ..Options::indented(4)
    };
    // json_encoder converts the only two values JSON cannot hold, so the
    // dump cannot fail.
    dumps(value, &options, Some(&json_encoder)).unwrap_or_default()
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
        Value::Blob(bytes) => Ok(Value::String(model::base64::encode(bytes))),
        other => Err(DumpError::unserializable(other)),
    }
}

#[cfg(test)]
mod tests;
