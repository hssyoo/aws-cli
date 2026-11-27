//! `BaseRestParser`: what a REST response holds outside its body (the
//! status code, header and header-map members, the request ids), and which
//! of the payload member or the body's members the body fills. What the
//! body's format decides is a [`Body`].
use crate::error::ParseError;
use crate::http::{Headers, HttpResponse};
use crate::protocol::common;
use model::schema::{Flag, Schema, ShapeType};
use model::value::{Map, Value};

/// What `BaseRestParser` leaves to its subclass: `_initial_body_parse`,
/// and `_parse_shape` with the subclass's handlers.
pub(in crate::protocol) trait Body {
    /// The body as `_initial_body_parse` reads it.
    type Parsed;
    /// `_initial_body_parse`.
    fn initial(&self, body: &[u8]) -> Result<Self::Parsed, ParseError>;
    /// `_parse_shape` of what [`Body::initial`] read.
    fn parse(&self, shape: Schema<'_>, parsed: &Self::Parsed) -> Result<Value, ParseError>;
    /// `_parse_shape` of a header's text or the status code.
    fn value(&self, shape: Schema<'_>, value: &Value) -> Result<Value, ParseError>;
}

/// `BaseRestParser._do_parse`: `ResponseMetadata` first, then the
/// output's members.
pub(in crate::protocol) fn do_parse(
    body: &impl Body,
    shape: Option<Schema<'_>>,
    response: &HttpResponse,
) -> Result<Map, ParseError> {
    let mut parsed = Map::new();
    parsed.insert(
        "ResponseMetadata".into(),
        Value::Map(response_metadata(&response.headers)),
    );
    if let Some(shape) = shape {
        add_modeled_parse(body, shape, response, &mut parsed, Payload::Streamed)?;
    }
    Ok(parsed)
}

/// `BaseRestParser._do_modeled_error_parse`: the error shape's members,
/// those outside the body included.
pub(in crate::protocol) fn do_modeled_error_parse(
    body: &impl Body,
    shape: Schema<'_>,
    response: &HttpResponse,
) -> Result<Map, ParseError> {
    let mut parsed = Map::new();
    add_modeled_parse(body, shape, response, &mut parsed, Payload::Read)?;
    Ok(parsed)
}

/// How a blob payload is held: as the `StreamingBody` botocore makes of a
/// successful response's body (`convert_to_response_dict` with
/// `has_streaming_output`), its bytes as they arrived; or, in an error,
/// read whole and decoded as text.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Payload {
    Streamed,
    Read,
}

/// `_add_modeled_parse`: the members outside the body, then the payload or
/// the body's members.
fn add_modeled_parse(
    body: &impl Body,
    shape: Schema<'_>,
    response: &HttpResponse,
    parsed: &mut Map,
    blob: Payload,
) -> Result<(), ParseError> {
    non_payload_attrs(body, shape, response, parsed)?;
    payload(body, shape, response, parsed, blob)
}

/// `_populate_response_metadata`: `x-amzn-requestid` as `RequestId`, else
/// `x-amz-request-id` with `x-amz-id-2` as `HostId`.
pub(in crate::protocol) fn response_metadata(headers: &Headers) -> Map {
    let mut metadata = Map::new();
    if let Some(id) = headers.joined("x-amzn-requestid") {
        metadata.insert("RequestId".into(), Value::String(id));
    } else if let Some(id) = headers.joined("x-amz-request-id") {
        metadata.insert("RequestId".into(), Value::String(id));
        let host = headers.joined("x-amz-id-2").unwrap_or_default();
        metadata.insert("HostId".into(), Value::String(host));
    }
    metadata
}

/// `_parse_non_payload_attrs`: in model order, a `statusCode` member from
/// the status, a `headers` member from the headers its prefix starts, and
/// a `header` member from its header when the response has it.
fn non_payload_attrs(
    body: &impl Body,
    shape: Schema<'_>,
    response: &HttpResponse,
    parsed: &mut Map,
) -> Result<(), ParseError> {
    for member in shape.members() {
        let value = match member.location() {
            Some("statusCode") => {
                let status = Value::Integer(i64::from(response.status));
                body.value(member, &status)?
            }
            Some("headers") => {
                let prefix = member.location_name().unwrap_or("");
                Value::Map(header_map(prefix, &response.headers))
            }
            Some("header") => {
                let Some(text) = response.headers.joined(common::wire_name(member)) else {
                    continue;
                };
                body.value(member, &Value::String(text))?
            }
            _ => continue,
        };
        parsed.insert(common::member_name(member).to_owned(), value);
    }
    Ok(())
}

/// `_parse_header_map`: each header whose name starts with `prefix`, in any
/// case, by its name without the prefix, its value as it is.
fn header_map(prefix: &str, headers: &Headers) -> Map {
    let prefix = prefix.to_lowercase();
    let skip = prefix.chars().count();
    let mut parsed = Map::new();
    for (name, value) in common::distinct(headers) {
        if name.to_lowercase().starts_with(&prefix) {
            parsed.insert(name.chars().skip(skip).collect(), Value::String(value));
        }
    }
    parsed
}

/// `_parse_payload`: with a `payload`, that member alone from the body (a
/// string as the decoded text, a blob as `blob` holds it); without one, the
/// body's members.
fn payload(
    body: &impl Body,
    shape: Schema<'_>,
    response: &HttpResponse,
    parsed: &mut Map,
    blob: Payload,
) -> Result<(), ParseError> {
    let Some(name) = shape.payload() else {
        let members = body.parse(shape, &body.initial(&response.body)?)?;
        return update(parsed, members);
    };
    let member = shape
        .member(name)
        .ok_or_else(|| ParseError::new(format!("'{name}'")))?;
    let value = if member.flag(Flag::EventStream) {
        return Err(ParseError::new("event stream responses are not supported"));
    } else if member.shape_type() == ShapeType::Blob && blob == Payload::Streamed {
        Value::Blob(response.body.clone())
    } else if matches!(member.shape_type(), ShapeType::String | ShapeType::Blob) {
        Value::String(common::decoded(&response.body)?.to_owned())
    } else {
        body.parse(member, &body.initial(&response.body)?)?
    };
    parsed.insert(name.to_owned(), value);
    Ok(())
}

/// `final_parsed.update(body_parsed)`: a `null` body's `None` is not a
/// mapping.
fn update(parsed: &mut Map, members: Value) -> Result<(), ParseError> {
    match members {
        Value::Map(members) => {
            for (key, value) in members {
                parsed.insert_key(key, value);
            }
            Ok(())
        }
        other => Err(ParseError::new(format!(
            "'{}' object is not iterable",
            common::short_type(&other)
        ))),
    }
}
