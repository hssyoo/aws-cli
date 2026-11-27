//! What botocore's serializers and parsers share (`Serializer`,
//! `ResponseParser`): timestamps and blobs in both directions, host
//! prefixes, member and wire names, and body decoding. The parse drivers
//! are [`super::compat`]'s.
use crate::error::{ParseError, SerializeError};
use crate::http::Headers;
use model::pyrepr::{float_from_text, py_str};
use model::schema::{Flag, OperationSchema, Schema, ShapeType, traits};
use model::value::{Key, Value};
use timestamp::Timestamp;

/// botocore's default `timestamp_parser`, `parse_timestamp`, as an instant:
/// a number or text `float()` reads is seconds since the epoch, other text
/// ISO 8601 or RFC 822. A zone-less time is taken as UTC.
pub(super) fn parse_timestamp(value: &Value) -> Result<Value, ParseError> {
    instant(value)
        .map(Value::Timestamp)
        .ok_or_else(|| ParseError::new(format!("Invalid timestamp \"{}\"", py_str(value))))
}

/// The instant `parse_to_aware_datetime` reads `value` as, a zone-less time
/// being UTC.
pub(super) fn instant(value: &Value) -> Option<Timestamp> {
    match value {
        Value::Timestamp(at) => Some(*at),
        Value::Integer(seconds) => Timestamp::from_unix_seconds(*seconds),
        Value::Bool(flag) => Timestamp::from_unix_seconds(i64::from(*flag)),
        Value::Float(seconds) => Timestamp::from_epoch_float(*seconds),
        Value::String(text) => match float_from_text(text) {
            Some(seconds) => Timestamp::from_epoch_float(seconds),
            None => written(text),
        },
        // Neither a number nor a date: a surrogate is no part of either.
        Value::PyString(_) => None,
        _ => None,
    }
}

fn written(text: &str) -> Option<Timestamp> {
    let zoned =
        Timestamp::parse_iso8601_zoned(text).or_else(|| Timestamp::parse_rfc822_zoned(text))?;
    let offset = i64::from(zoned.offset_seconds.unwrap_or(0)) * 1_000_000;
    let local = Timestamp::from_civil(zoned.civil)?;
    Timestamp::from_unix_micros(local.unix_micros().checked_sub(offset)?)
}

/// botocore's default `blob_parser`, `base64.b64decode` of a `str`, which
/// must be ASCII (`_bytes_from_decode_data`): a lone surrogate is not.
pub(super) fn decode_blob(value: &Value) -> Result<Value, ParseError> {
    match value {
        Value::String(text) if text.is_ascii() => pytext::base64::decode(text)
            .map(Value::Blob)
            .map_err(ParseError::new),
        Value::String(_) | Value::PyString(_) => Err(ParseError::new(
            "string argument should contain only ASCII characters",
        )),
        other => Err(ParseError::new(format!(
            "argument should be a bytes-like object or ASCII string, not '{}'",
            short_type(other)
        ))),
    }
}

/// `Serializer._convert_timestamp_to_str`: the value as an instant, then in
/// `format` (`iso8601`, `unixtimestamp` or `rfc822`, any case).
pub(crate) fn timestamp_out(value: &Value, format: &str) -> Result<Value, SerializeError> {
    let at = instant(value)
        .ok_or_else(|| SerializeError::new(format!("Invalid timestamp \"{}\"", py_str(value))))?;
    match format.to_ascii_lowercase().as_str() {
        "iso8601" => Ok(Value::String(at.iso8601())),
        "rfc822" => Ok(Value::String(at.rfc822())),
        "unixtimestamp" if at.subsec_micros() == 0 => Ok(Value::Integer(at.unix_seconds())),
        "unixtimestamp" => Ok(Value::Float(at.unix_micros() as f64 / 1e6)),
        other => Err(SerializeError::new(format!(
            "'Serializer' object has no attribute '_timestamp_{other}'"
        ))),
    }
}

/// `Serializer._get_base64`: text is encoded as UTF-8 first.
pub(crate) fn base64_in(value: &Value) -> Result<String, SerializeError> {
    match value {
        Value::String(text) => Ok(pytext::base64::encode(text.as_bytes())),
        Value::PyString(text) => {
            pytext::codec::encode_utf8(text.as_bytes(), pytext::codec::Errors::Strict)
                .map(|bytes| pytext::base64::encode(&bytes))
                .map_err(|error| SerializeError::new(error.to_string()))
        }
        Value::Blob(bytes) => Ok(pytext::base64::encode(bytes)),
        other => Err(SerializeError::new(format!(
            "a bytes-like object is required, not '{}'",
            short_type(other)
        ))),
    }
}

/// `Serializer._handle_float`: the three special values as text.
pub(crate) fn special_float(value: &Value) -> Value {
    match value {
        Value::Float(number) if number.is_nan() => Value::String("NaN".into()),
        Value::Float(number) if *number == f64::INFINITY => Value::String("Infinity".into()),
        Value::Float(number) if *number == f64::NEG_INFINITY => Value::String("-Infinity".into()),
        other => other.clone(),
    }
}

/// `type(value).__name__`.
pub(super) use model::pyrepr::py_short_type_name as short_type;

/// `Serializer._expand_host_prefix`: `endpoint.hostPrefix` with each
/// `hostLabel` member's value put in its `{name}`.
pub(super) fn host_prefix(
    operation: OperationSchema<'_>,
    input: &Value,
) -> Result<Option<String>, SerializeError> {
    let Some(template) = operation.host_prefix() else {
        return Ok(None);
    };
    let Some(shape) = operation.input() else {
        return Ok(Some(template.to_owned()));
    };
    let mut prefix = template.to_owned();
    for member in shape.members() {
        if !member.flag(Flag::HostLabel) {
            continue;
        }
        let name = member_name(member);
        let value = match input {
            Value::Map(map) => map.get(name),
            _ => None,
        };
        let value = value.ok_or_else(|| SerializeError::new(format!("'{name}'")))?;
        prefix = prefix.replace(&format!("{{{name}}}"), &py_str(value));
    }
    Ok(Some(prefix))
}

/// The member of `shape` named `key`, as `shape.members[key]` finds it.
/// `shape.members[key]`: the member's shape, else the `KeyError`'s text. An
/// input's keys are the CLI's, so a key with a lone surrogate is only ever
/// such a `KeyError`, as it is in Python.
pub(super) fn member<'s>(shape: Schema<'s>, key: &Key) -> Result<Schema<'s>, SerializeError> {
    key.as_str()
        .and_then(|name| shape.member(name))
        .ok_or_else(|| SerializeError::new(format!("'{}'", key.text())))
}

/// The model name of a shape a reference resolves to (`Shape.name`): a
/// member's is its target's.
pub(super) fn name(shape: Schema<'_>) -> &str {
    shape.target().id().name()
}

/// A member's name in its structure.
pub(super) fn member_name(member: Schema<'_>) -> &str {
    member.member_name().unwrap_or_default()
}

/// A member's `locationName`, else its name: the key the JSON and XML
/// bodies hold it under.
pub(crate) fn wire_name(member: Schema<'_>) -> &str {
    member
        .location_name()
        .unwrap_or_else(|| member_name(member))
}

/// A list's member, which every list the front-end builds has.
pub(super) fn list_member(shape: Schema<'_>) -> Result<Schema<'_>, String> {
    shape
        .list_member()
        .ok_or_else(|| format!("the list {} has no member", name(shape)))
}

/// A map's key and value, which every map the front-end builds has.
pub(crate) fn map_entry(shape: Schema<'_>) -> Result<(Schema<'_>, Schema<'_>), String> {
    shape
        .map_key()
        .zip(shape.map_value())
        .ok_or_else(|| format!("the map {} has no key or value", name(shape)))
}

/// botocore's `structure` type, a document or union included.
pub(super) fn is_structure(shape: Schema<'_>) -> bool {
    matches!(
        shape.shape_type(),
        ShapeType::Structure | ShapeType::Document | ShapeType::Union
    )
}

/// A truthy `document`: the type says it, unless a cold `c2j#document`
/// says the reference's truthiness differs from the type's.
pub(super) fn is_document(shape: Schema<'_>) -> bool {
    (shape.shape_type() == ShapeType::Document) != shape.has_trait(traits::DOCUMENT)
}

/// The headers as botocore's `HeadersDict` iterates them: each name once,
/// spelled as it first came, with [`header`]'s value.
pub(super) fn distinct(headers: &Headers) -> Vec<(String, String)> {
    let mut names: Vec<&str> = Vec::new();
    for (name, _) in headers.iter() {
        if !names.iter().any(|seen| seen.eq_ignore_ascii_case(name)) {
            names.push(name);
        }
    }
    names
        .into_iter()
        .map(|name| (name.to_owned(), headers.joined(name).unwrap_or_default()))
        .collect()
}

/// `bytes.decode('utf-8')`, whose `UnicodeDecodeError` the parsers let
/// through with CPython's text.
pub(crate) fn decoded(bytes: &[u8]) -> Result<&str, ParseError> {
    pytext::text::decode_utf8(bytes).map_err(ParseError::new)
}

/// The failure of a call whose response is valid JSON nested deeper than
/// the native client's reader holds ([`model::pyjson::MAX_DEPTH`]): its own
/// limit, named, where Python would read it. Invalid text that deep is not
/// the limit: it is `{'message': body}`, as in Python.
pub(crate) fn beyond_limit(limit: model::pyjson::Limit) -> ParseError {
    ParseError::new(format!(
        "native client limit: the response holds {limit}, which the native client does not \
         represent"
    ))
}

#[cfg(test)]
mod tests;
