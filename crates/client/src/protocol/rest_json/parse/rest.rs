//! `BaseRestParser`: what a REST response holds outside its body (the
//! status code, header and header-map members, the request ids), and which
//! of the payload member or the body's members the body fills. What the
//! body's format decides is a [`Body`].
use crate::error::ParseError;
use crate::http::{Headers, HttpResponse};
use crate::protocol::common;
use model::shapes::{Flag, Model, Shape, ShapeId, ShapeKind};
use model::value::{Map, Value};

/// What `BaseRestParser` leaves to its subclass: `_initial_body_parse`,
/// and `_parse_shape` with the subclass's handlers.
pub(super) trait Body {
    /// The body as `_initial_body_parse` reads it.
    type Parsed;
    /// `_initial_body_parse`.
    fn initial(&self, body: &[u8]) -> Result<Self::Parsed, ParseError>;
    /// `_parse_shape` of what [`Body::initial`] read.
    fn parse(&self, shape: ShapeId, parsed: &Self::Parsed) -> Result<Value, ParseError>;
    /// `_parse_shape` of a header's text or the status code.
    fn value(&self, shape: ShapeId, value: &Value) -> Result<Value, ParseError>;
}

/// `BaseRestParser._do_parse`: `ResponseMetadata` first, then the
/// output's members.
pub(super) fn do_parse(
    body: &impl Body,
    model: &Model,
    shape: Option<ShapeId>,
    response: &HttpResponse,
) -> Result<Map, ParseError> {
    let mut parsed = Map::new();
    parsed.insert(
        "ResponseMetadata".into(),
        Value::Map(response_metadata(&response.headers)),
    );
    if let Some(shape) = shape {
        add_modeled_parse(body, model, shape, response, &mut parsed)?;
    }
    Ok(parsed)
}

/// `BaseRestParser._do_modeled_error_parse`: the error shape's members,
/// those outside the body included.
pub(super) fn do_modeled_error_parse(
    body: &impl Body,
    model: &Model,
    shape: ShapeId,
    response: &HttpResponse,
) -> Result<Map, ParseError> {
    let mut parsed = Map::new();
    add_modeled_parse(body, model, shape, response, &mut parsed)?;
    Ok(parsed)
}

/// `_add_modeled_parse`: the members outside the body, then the payload or
/// the body's members.
fn add_modeled_parse(
    body: &impl Body,
    model: &Model,
    shape: ShapeId,
    response: &HttpResponse,
    parsed: &mut Map,
) -> Result<(), ParseError> {
    non_payload_attrs(body, model, shape, response, parsed)?;
    payload(body, model, shape, response, parsed)
}

/// `_populate_response_metadata`: `x-amzn-requestid` as `RequestId`, else
/// `x-amz-request-id` with `x-amz-id-2` as `HostId`.
fn response_metadata(headers: &Headers) -> Map {
    let mut metadata = Map::new();
    if let Some(id) = common::header(headers, "x-amzn-requestid") {
        metadata.insert("RequestId".into(), Value::String(id));
    } else if let Some(id) = common::header(headers, "x-amz-request-id") {
        metadata.insert("RequestId".into(), Value::String(id));
        let host = common::header(headers, "x-amz-id-2").unwrap_or_default();
        metadata.insert("HostId".into(), Value::String(host));
    }
    metadata
}

/// `_parse_non_payload_attrs`: in model order, a `statusCode` member from
/// the status, a `headers` member from the headers its prefix starts, and
/// a `header` member from its header when the response has it.
fn non_payload_attrs(
    body: &impl Body,
    model: &Model,
    shape: ShapeId,
    response: &HttpResponse,
    parsed: &mut Map,
) -> Result<(), ParseError> {
    for member in model.members(shape) {
        let traits = &model.shape(member.shape).traits;
        let value = match traits.location.as_deref() {
            Some("statusCode") => {
                let status = Value::Integer(i64::from(response.status));
                body.value(member.shape, &status)?
            }
            Some("headers") => {
                let prefix = traits.name.as_deref().unwrap_or("");
                Value::Map(header_map(prefix, &response.headers))
            }
            Some("header") => {
                let name = traits.name.as_deref().unwrap_or(&member.name);
                let Some(text) = common::header(&response.headers, name) else {
                    continue;
                };
                body.value(member.shape, &Value::String(text))?
            }
            _ => continue,
        };
        parsed.insert(member.name.to_string(), value);
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
/// string or blob as the decoded text); without one, the body's members.
fn payload(
    body: &impl Body,
    model: &Model,
    shape: ShapeId,
    response: &HttpResponse,
    parsed: &mut Map,
) -> Result<(), ParseError> {
    let Some(name) = model.shape(shape).traits.payload.as_deref() else {
        let members = body.parse(shape, &body.initial(&response.body)?)?;
        return update(parsed, members);
    };
    let member = model
        .members(shape)
        .iter()
        .find(|member| &*member.name == name)
        .ok_or_else(|| ParseError::new(format!("'{name}'")))?;
    let resolved = model.shape(member.shape);
    let value = if resolved.traits.is(Flag::EventStream) {
        return Err(ParseError::new("event stream responses are not supported"));
    } else if matches!(resolved.kind, ShapeKind::String | ShapeKind::Blob) {
        Value::String(common::decoded(&response.body)?.to_owned())
    } else {
        body.parse(member.shape, &body.initial(&response.body)?)?
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

/// `BaseRestParser._handle_string` for a `jsonvalue` header
/// (`is_json_value_header`): the text base64-decoded and read as JSON.
/// `None` for any other string, which stays as it is.
pub(super) fn json_value(shape: &Shape, value: &Value) -> Option<Result<Value, ParseError>> {
    let header = shape.traits.location.as_deref() == Some("header");
    (header && shape.traits.is(Flag::JsonValue)).then(|| json_value_of(value))
}

/// `json.loads(base64.b64decode(value).decode('utf-8'))`.
fn json_value_of(value: &Value) -> Result<Value, ParseError> {
    let Value::String(text) = value else {
        return Err(ParseError::new(format!(
            "argument should be a bytes-like object or ASCII string, not '{}'",
            common::short_type(value)
        )));
    };
    if !text.is_ascii() {
        return Err(ParseError::new(
            "string argument should contain only ASCII characters",
        ));
    }
    let bytes = pytext::base64::decode(text).map_err(ParseError::new)?;
    match model::pyjson::loads(common::decoded(&bytes)?) {
        Ok(value) => Ok(value),
        Err(model::pyjson::LoadError::Invalid(message)) => Err(ParseError::new(message)),
        Err(model::pyjson::LoadError::Limit(limit)) => Err(common::beyond_limit(limit)),
    }
}

/// `BaseRestParser._handle_list`'s split: a header list's text as its
/// comma-separated parts, each stripped, as RFC 7230 lists them. `None`
/// for a list that is no header or is a list already.
pub(super) fn header_items(shape: &Shape, value: &Value) -> Option<Result<Value, ParseError>> {
    if shape.traits.location.as_deref() != Some("header") {
        return None;
    }
    match value {
        Value::List(_) => None,
        Value::String(text) => {
            let parts = text
                .split(',')
                .map(|part| Value::String(pytext::text::strip(part).to_owned()));
            Some(Ok(Value::List(parts.collect())))
        }
        other => Some(Err(no_attribute(other, "split"))),
    }
}

/// The `AttributeError` of `value.name` on a value without that method.
pub(super) fn no_attribute(value: &Value, name: &str) -> ParseError {
    ParseError::new(format!(
        "'{}' object has no attribute '{name}'",
        common::short_type(value)
    ))
}
