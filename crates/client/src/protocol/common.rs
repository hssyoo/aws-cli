//! What botocore's serializers and parsers share (`Serializer`,
//! `ResponseParser`): timestamps and blobs in both directions, host
//! prefixes, the generic error path, `ResponseMetadata`, and the second,
//! modelled parse of an error.
use super::ParseOptions;
use crate::error::{ParseError, SerializeError, ServiceError};
use crate::http::{Headers, HttpResponse};
use model::pyrepr::{float_from_text, py_str};
use model::schema::{Flag, OperationSchema, Schema, ShapeType, traits};
use model::shapes::{Model, Operation};
use model::value::{Key, Map, Value};
use timestamp::Timestamp;

/// One protocol family's parse steps (`_do_parse`, `_do_error_parse`,
/// `_do_modeled_error_parse`).
pub(super) trait Family {
    fn do_parse(
        &self,
        shape: Option<Schema<'_>>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Value, ParseError>;
    fn do_error_parse(&self, response: &HttpResponse) -> Result<Map, ParseError>;
    fn do_modeled_error_parse(
        &self,
        shape: Schema<'_>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Map, ParseError>;
}

/// `ResponseParser.parse`: the error paths from 301, a modelled error
/// shape's members alone, and `ResponseMetadata` added to the rest.
pub(super) fn parse(
    family: &dyn Family,
    shape: Option<Schema<'_>>,
    response: &HttpResponse,
    options: &ParseOptions<'_>,
) -> Result<Value, ParseError> {
    let exception = shape.filter(|shape| shape.flag(Flag::Error));
    let parsed = if response.status < 301 {
        family.do_parse(shape, response, options)?
    } else if is_generic_error(response) {
        Value::Map(generic_error(response.status))
    } else if let Some(shape) = exception {
        return family
            .do_modeled_error_parse(shape, response, options)
            .map(Value::Map);
    } else {
        Value::Map(family.do_error_parse(response)?)
    };
    // `if isinstance(parsed, dict)`: a `None` output gets no metadata.
    let Value::Map(mut parsed) = parsed else {
        return Ok(parsed);
    };
    if shape.is_some_and(|shape| shape.flag(Flag::EventStream)) {
        return Ok(Value::Map(parsed));
    }
    // `parsed.get('ResponseMetadata', {})`, updated and set back: a
    // metadata dict the parser made keeps its place (the REST parsers put
    // it first), and a new one goes last.
    let mut metadata = match parsed.get("ResponseMetadata") {
        Some(Value::Map(metadata)) => metadata.clone(),
        _ => Map::new(),
    };
    metadata.insert(
        "HTTPStatusCode".into(),
        Value::Integer(i64::from(response.status)),
    );
    metadata.insert(
        "HTTPHeaders".into(),
        Value::Map(lowercase_headers(&response.headers)),
    );
    parsed.insert("ResponseMetadata".into(), Value::Map(metadata));
    Ok(Value::Map(parsed))
}

/// `parse` then, when the error's code names a modelled error,
/// `Endpoint._add_modeled_error_fields`' second parse with that shape,
/// merged into the first.
pub(super) fn parse_error(
    family: &dyn Family,
    model: &Model,
    operation: &Operation,
    response: &HttpResponse,
    options: &ParseOptions<'_>,
) -> Result<ServiceError, ParseError> {
    let operation = operation_schema(model, operation).map_err(ParseError::new)?;
    let Value::Map(mut parsed) = parse(family, operation.output(), response, options)? else {
        return Err(ParseError::new("'NoneType' object has no attribute 'get'"));
    };
    let code = match parsed.get("Error") {
        Some(Value::Map(error)) => error.get("Code").and_then(Value::as_str).map(str::to_owned),
        _ => None,
    };
    if let Some(shape) = code
        .as_deref()
        .and_then(|code| model.schema().registry().entry_for_error_code(code))
    {
        let Value::Map(modelled) = parse(family, Some(shape), response, options)? else {
            return Err(ParseError::new("'NoneType' object is not iterable"));
        };
        for (key, value) in modelled {
            parsed.insert_key(key, value);
        }
    }
    Ok(service_error(operation.name(), response, parsed))
}

/// The parsed error response as `ClientError` holds it.
fn service_error(operation: &str, response: &HttpResponse, parsed: Map) -> ServiceError {
    let mut error = ServiceError::new(operation, response.status);
    error.headers = response.headers.clone();
    for (key, value) in parsed {
        match value {
            Value::Map(map) if key == *"Error" => error.error = map,
            Value::Map(metadata) if key == *"ResponseMetadata" => {
                error.request_id = metadata
                    .get("RequestId")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            value => {
                error.members.insert_key(key, value);
            }
        }
    }
    error
}

/// `ResponseParser._is_generic_error_response`: a 5xx without a body, or
/// one that is HTML.
fn is_generic_error(response: &HttpResponse) -> bool {
    if response.status < 500 {
        return false;
    }
    let body = response.body.trim_ascii();
    body.is_empty() || body.starts_with(b"<html>")
}

/// `ResponseParser._do_generic_error_parse`.
fn generic_error(status: u16) -> Map {
    let mut error = Map::new();
    error.insert("Code".into(), Value::String(status.to_string()));
    error.insert(
        "Message".into(),
        Value::String(reason_phrase(status).into()),
    );
    let mut parsed = Map::new();
    parsed.insert("Error".into(), Value::Map(error));
    parsed.insert("ResponseMetadata".into(), Value::Map(Map::new()));
    parsed
}

/// `http.client.responses` for the statuses the generic path reads (5xx),
/// in CPython 3.14.
fn reason_phrase(status: u16) -> &'static str {
    match status {
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        506 => "Variant Also Negotiates",
        507 => "Insufficient Storage",
        508 => "Loop Detected",
        510 => "Not Extended",
        511 => "Network Authentication Required",
        _ => "",
    }
}

/// `botocore.utils.lowercase_dict` of the response headers: names
/// lowercased, a repeated header's values joined with `, ` as urllib3
/// joins them.
fn lowercase_headers(headers: &Headers) -> Map {
    let mut map = Map::new();
    for (name, value) in headers.iter() {
        let name = name.to_ascii_lowercase();
        let joined = match map.get(&name) {
            Some(Value::String(old)) => format!("{old}, {value}"),
            _ => value.to_owned(),
        };
        map.insert(name, Value::String(joined));
    }
    map
}

/// `ResponseParser._has_unknown_tagged_union_member`: for a union, the tag
/// of a member the model does not know, or `Err` when the response sets
/// other than exactly one member. `present` names the set members.
pub(super) fn unknown_union_member(
    shape: Schema<'_>,
    present: &[&Key],
) -> Result<Option<Key>, ParseError> {
    if !is_union(shape) {
        return Ok(None);
    }
    let present: Vec<&Key> = present
        .iter()
        .copied()
        .filter(|name| **name != *"__type")
        .collect();
    let [tag] = present[..] else {
        return Err(ParseError::new(format!(
            "Invalid service response: {} must have one and only one member set.",
            name(shape)
        )));
    };
    let known = shape.members().any(|member| *tag == *wire_name(member));
    Ok((!known).then(|| tag.clone()))
}

/// `_handle_unknown_tagged_union_member`: the tag as the `str` it is.
pub(super) fn unknown_member_value(tag: Key) -> Map {
    let mut name = Map::new();
    name.insert("name".into(), tag.to_value());
    let mut parsed = Map::new();
    parsed.insert("SDK_UNKNOWN_MEMBER".into(), Value::Map(name));
    parsed
}

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
pub(super) fn timestamp_out(value: &Value, format: &str) -> Result<Value, SerializeError> {
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
pub(super) fn base64_in(value: &Value) -> Result<String, SerializeError> {
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
pub(super) fn special_float(value: &Value) -> Value {
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

/// The schema of `operation`, which the model's front-end built from the
/// same description; `Err` only if the two readings disagree.
pub(super) fn operation_schema<'m>(
    model: &'m Model,
    operation: &Operation,
) -> Result<OperationSchema<'m>, String> {
    model
        .schema()
        .service()
        .operation(&operation.name)
        .ok_or_else(|| format!("the model has no schema for {}", operation.name))
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
pub(super) fn wire_name(member: Schema<'_>) -> &str {
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
pub(super) fn map_entry(shape: Schema<'_>) -> Result<(Schema<'_>, Schema<'_>), String> {
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

/// A truthy `union`, as [`is_document`] reads `document`.
pub(super) fn is_union(shape: Schema<'_>) -> bool {
    (shape.shape_type() == ShapeType::Union) != shape.has_trait(traits::UNION)
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
pub(super) fn decoded(bytes: &[u8]) -> Result<&str, ParseError> {
    pytext::text::decode_utf8(bytes).map_err(ParseError::new)
}

/// The failure of a call whose response is valid JSON nested deeper than
/// the native client's reader holds ([`model::pyjson::MAX_DEPTH`]): its own
/// limit, named, where Python would read it. Invalid text that deep is not
/// the limit: it is `{'message': body}`, as in Python.
pub(super) fn beyond_limit(limit: model::pyjson::Limit) -> ParseError {
    ParseError::new(format!(
        "native client limit: the response holds {limit}, which the native client does not \
         represent"
    ))
}

#[cfg(test)]
mod tests;
