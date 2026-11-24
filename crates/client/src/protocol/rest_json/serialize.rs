//! `RestJSONSerializer`: the operation's `requestUri` with its labels, the
//! query string, the headers, and the payload or JSON body.
//!
//! The first half is `BaseRestSerializer`'s, which every REST protocol
//! shares: the parameters partitioned by `location`, the URI template, the
//! query string and the header values. It knows nothing of JSON. The second
//! half is `RestJSONSerializer`'s: the payload or body as JSON, when an
//! empty body is `{}`, and `Content-Type`.
use crate::error::SerializeError;
use crate::http::Headers;
use crate::protocol::query::method;
use crate::protocol::{Serialized, common, json};
use model::pyjson::{Options, dumps};
use model::pyrepr::py_str;
use model::shapes::{Flag, Model, Operation, ShapeId, ShapeKind};
use model::value::{Map, Value};

/// `BaseRestSerializer.serialize_to_request` with `RestJSONSerializer`'s
/// body: `requestUri` with its labels filled in, the query string after
/// any query `requestUri` carries, the header members, then the payload
/// or the JSON body and its `Content-Type`.
///
/// # Errors
/// An input the protocol cannot put on the wire.
pub(super) fn request(
    model: &Model,
    operation: &Operation,
    input: &Value,
) -> Result<Serialized, SerializeError> {
    let method = method(operation)?;
    let host_prefix = common::host_prefix(model, operation, input)?;
    let template = || {
        operation
            .http
            .request_uri
            .as_deref()
            .ok_or_else(|| SerializeError::new("'requestUri'"))
    };
    // `if shape is None`: the template as it is, labels and all.
    let Some(shape) = operation.input else {
        let (path, query) = split_uri(template()?);
        return Ok(Serialized {
            method,
            path: path.to_owned(),
            query: query.unwrap_or_default().to_owned(),
            headers: Headers::new(),
            body: Vec::new(),
            host_prefix,
        });
    };
    let Value::Map(params) = input else {
        return Err(no_items(input));
    };
    let partitioned = partition(model, shape, params)?;
    let url_path = render_uri_template(template()?, &partitioned.uri)?;
    let (path, uri_query) = split_uri(&url_path);
    let query = joined_query(uri_query, &partitioned.query);
    let mut headers = partitioned.headers;
    let payload = Payload::of(model, shape)?;
    let body = body(model, shape, payload, params, &partitioned.body)?;
    serialize_content_type(payload, &body, &mut headers);
    Ok(Serialized {
        method,
        path: path.to_owned(),
        query,
        headers,
        body,
        host_prefix,
    })
}

/// The `AttributeError` of `.items()` on what is not a dict.
fn no_items(value: &Value) -> SerializeError {
    SerializeError::new(format!(
        "'{}' object has no attribute 'items'",
        common::short_type(value)
    ))
}

/// The `TypeError` of iterating what is not a list.
fn not_iterable(value: &Value) -> SerializeError {
    SerializeError::new(format!(
        "'{}' object is not iterable",
        common::short_type(value)
    ))
}

/// `_partition_parameters`' four dicts.
#[derive(Default)]
struct Partitioned {
    /// `uri_path_kwargs`: each label's value, converted.
    uri: Map,
    /// `query_string_kwargs`: each name's value, converted, a list's items
    /// each their own pair. A map member's entries are merged in as given.
    query: Map,
    headers: Headers,
    /// `body_kwargs`: the members with no known location, by member name.
    body: Map,
}

/// `BaseRestSerializer._partition_parameters` of every parameter that is
/// not `None`, in the caller's order: each member by its `location`
/// (`uri`, `querystring`, `header`, `headers`) under its `locationName`,
/// the rest into the body.
fn partition(model: &Model, shape: ShapeId, params: &Map) -> Result<Partitioned, SerializeError> {
    let mut parts = Partitioned::default();
    for (name, value) in params.iter() {
        if *value == Value::Null {
            continue;
        }
        let member = common::member(model, shape, name)?;
        let resolved = model.shape(member);
        let key = resolved.traits.name.as_deref().unwrap_or(name);
        match resolved.traits.location.as_deref() {
            Some("uri") => {
                parts
                    .uri
                    .insert(key.to_owned(), uri_and_query_value(model, member, value)?);
            }
            Some("querystring") => query_param(model, member, key, value, &mut parts.query)?,
            Some("header") => {
                // `if not param_value and shape.type_name == 'list'`.
                let empty = matches!(value, Value::List(items) if items.is_empty());
                if !(empty && matches!(resolved.kind, ShapeKind::List { .. })) {
                    let converted = convert_header_value(model, member, value)?;
                    parts.headers.set(key, converted);
                }
            }
            Some("headers") => serialize_header_map(key, value, &mut parts.headers)?,
            _ => {
                parts.body.insert(name.to_owned(), value.clone());
            }
        }
    }
    Ok(parts)
}

/// A `querystring` member: a dict's entries merged in as they are (over a
/// name another member set, which keeps its place), a list's items each
/// converted, anything else converted. A list member's value that is not a
/// list is refused, even text, which Python would iterate.
fn query_param(
    model: &Model,
    member: ShapeId,
    key: &str,
    value: &Value,
    query: &mut Map,
) -> Result<(), SerializeError> {
    match (value, &model.shape(member).kind) {
        (Value::Map(map), _) => {
            for (name, item) in map.iter() {
                query.insert(name.to_owned(), item.clone());
            }
        }
        (_, ShapeKind::List { member: item }) => {
            let Value::List(items) = value else {
                return Err(not_iterable(value));
            };
            let converted = items
                .iter()
                .map(|value| uri_and_query_value(model, *item, value))
                .collect::<Result<_, _>>()?;
            query.insert(key.to_owned(), Value::List(converted));
        }
        _ => {
            query.insert(key.to_owned(), uri_and_query_value(model, member, value)?);
        }
    }
    Ok(())
}

/// `_get_uri_and_query_string_value`: a boolean as `true` or `false`, a
/// timestamp in its `timestampFormat` (`iso8601` by default), a float as
/// its `str()` with the special values spelled out, anything else as it
/// is.
fn uri_and_query_value(
    model: &Model,
    shape: ShapeId,
    value: &Value,
) -> Result<Value, SerializeError> {
    let resolved = model.shape(shape);
    Ok(match resolved.kind {
        ShapeKind::Boolean => Value::String(py_str(value).to_lowercase()),
        ShapeKind::Timestamp => {
            let format = resolved.traits.timestamp_format.as_deref();
            common::timestamp_out(value, format.unwrap_or("iso8601"))?
        }
        ShapeKind::Float | ShapeKind::Double => {
            Value::String(py_str(&common::special_float(value)))
        }
        _ => value.clone(),
    })
}

/// `_do_serialize_header_map`: each entry as a header named the prefix
/// and its key, its value as given.
fn serialize_header_map(
    prefix: &str,
    value: &Value,
    headers: &mut Headers,
) -> Result<(), SerializeError> {
    let Value::Map(map) = value else {
        return Err(no_items(value));
    };
    for (key, item) in map.iter() {
        headers.set(&format!("{prefix}{key}"), py_str(item));
    }
    Ok(())
}

/// `_convert_header_value`: a timestamp to the second in its
/// `timestampFormat` (`rfc822` by default); a list's items that are not
/// `None` joined with `,`, strings quoted when they hold `"` or `,`; a
/// `jsonvalue` string as base64 of compact JSON; a boolean as `true` or
/// `false`; anything else as its `str()`. A list value that is not a list
/// is refused, even text or a dict, which Python would iterate; validation
/// lets none through.
fn convert_header_value(
    model: &Model,
    shape: ShapeId,
    value: &Value,
) -> Result<String, SerializeError> {
    let resolved = model.shape(shape);
    match &resolved.kind {
        ShapeKind::Timestamp => {
            // `calendar.timegm(datetime_obj.utctimetuple())`.
            let seconds = common::instant(value)
                .map(|at| Value::Integer(at.unix_seconds()))
                .ok_or_else(|| {
                    SerializeError::new(format!("Invalid timestamp \"{}\"", py_str(value)))
                })?;
            let format = resolved.traits.timestamp_format.as_deref();
            common::timestamp_out(&seconds, format.unwrap_or("rfc822")).map(|out| py_str(&out))
        }
        ShapeKind::List { member } => {
            let Value::List(items) = value else {
                return Err(not_iterable(value));
            };
            let strings = matches!(model.shape(*member).kind, ShapeKind::String);
            let converted = items
                .iter()
                .filter(|item| **item != Value::Null)
                .map(|item| match item {
                    Value::String(text) if strings => Ok(escape_header_list_string(text)),
                    other if strings => Err(SerializeError::new(format!(
                        "argument of type '{}' is not iterable",
                        common::short_type(other)
                    ))),
                    other => convert_header_value(model, *member, other),
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(converted.join(","))
        }
        ShapeKind::String
            if resolved.traits.is(Flag::JsonValue)
                && resolved.traits.location.as_deref() == Some("header") =>
        {
            let text = dumps(value, &Options::compact(), None)
                .map_err(|error| SerializeError::new(error.to_string()))?;
            Ok(pytext::base64::encode(text.as_bytes()))
        }
        ShapeKind::Boolean => Ok(py_str(value).to_lowercase()),
        ShapeKind::Float | ShapeKind::Double => Ok(py_str(&common::special_float(value))),
        _ => Ok(py_str(value)),
    }
}

/// `_escape_header_list_string`: a string holding `"` or `,` in double
/// quotes, its `"` escaped with `\`.
fn escape_header_list_string(text: &str) -> String {
    if text.contains(['"', ',']) {
        format!("\"{}\"", text.replace('"', "\\\""))
    } else {
        text.to_owned()
    }
}

/// `_render_uri_template`: each `{Label}` the label's value
/// percent-encoded, `/` included; a greedy `{Label+}` keeps its `/`.
///
/// # Errors
/// A label with no value (`KeyError`).
fn render_uri_template(template: &str, labels: &Map) -> Result<String, SerializeError> {
    let mut rendered = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{')
        && let Some(length) = rest[start..].find('}')
    {
        rendered.push_str(&rest[..start]);
        let label = &rest[start + 1..start + length];
        let (name, safe): (&str, &[u8]) = match label.strip_suffix('+') {
            Some(name) => (name, b"/~"),
            None => (label, b""),
        };
        let value = labels
            .get(name)
            .ok_or_else(|| SerializeError::new(format!("'{name}'")))?;
        rendered.push_str(&percent_encode(value, safe));
        rest = &rest[start + length + 1..];
    }
    rendered.push_str(rest);
    Ok(rendered)
}

/// The rendered `url_path` as the request's URL splits it: the path, and
/// the query after the first `?`, if any.
fn split_uri(url_path: &str) -> (&str, Option<&str>) {
    match url_path.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (url_path, None),
    }
}

/// `prepare_request_dict`'s query: `query_string`, when it has any name,
/// [`percent_encode_sequence`]d after the `requestUri`'s own query and a
/// `&`. A query string with names but no pairs (an empty list) still adds
/// the `&`.
fn joined_query(uri_query: Option<&str>, query: &Map) -> String {
    match (uri_query, query.is_empty()) {
        (Some(own), true) => own.to_owned(),
        (Some(own), false) => format!("{own}&{}", percent_encode_sequence(query)),
        (None, true) => String::new(),
        (None, false) => percent_encode_sequence(query),
    }
}

/// `percent_encode_sequence`: `name=value` pairs joined by `&`, both
/// percent-encoded, a list giving one pair per item.
fn percent_encode_sequence(query: &Map) -> String {
    let mut pairs = Vec::new();
    for (name, value) in query.iter() {
        let name = encode_bytes(name.as_bytes(), b"");
        match value {
            Value::List(items) => {
                for item in items {
                    pairs.push(format!("{name}={}", percent_encode(item, b"")));
                }
            }
            other => pairs.push(format!("{name}={}", percent_encode(other, b""))),
        }
    }
    pairs.join("&")
}

/// `percent_encode`: bytes as they are, text as UTF-8, anything else as
/// its `str()`; then `urllib.parse.quote` with `safe`.
fn percent_encode(value: &Value, safe: &[u8]) -> String {
    match value {
        Value::Blob(bytes) => encode_bytes(bytes, safe),
        Value::String(text) => encode_bytes(text.as_bytes(), safe),
        other => encode_bytes(py_str(other).as_bytes(), safe),
    }
}

/// `urllib.parse.quote(bytes, safe)`: every byte but letters, digits,
/// `_.-~` and those in `safe` as `%XX`.
fn encode_bytes(bytes: &[u8], safe: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len());
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || b"_.-~".contains(&byte) || safe.contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// The input's `payload` member, if it names one.
#[derive(Clone, Copy)]
struct Payload<'a> {
    name: &'a str,
    shape: ShapeId,
    /// `_has_streaming_payload`: a blob or a string, sent as it is.
    raw: bool,
}

impl<'a> Payload<'a> {
    fn of(model: &'a Model, shape: ShapeId) -> Result<Option<Self>, SerializeError> {
        let Some(name) = model.shape(shape).traits.payload.as_deref() else {
            return Ok(None);
        };
        let member = common::member(model, shape, name)?;
        let raw = matches!(
            model.shape(member).kind,
            ShapeKind::Blob | ShapeKind::String
        );
        Ok(Some(Self {
            name,
            shape: member,
            raw,
        }))
    }
}

/// `_serialize_payload` with `RestJSONSerializer`'s rules: a blob or string
/// payload as its bytes (none when unset); another payload as JSON, `{}`
/// when unset; without a payload, the body members as a JSON object, or
/// `{}` when none is set but the input has a member with no location
/// (`_requires_empty_body`), else nothing.
fn body(
    model: &Model,
    shape: ShapeId,
    payload: Option<Payload<'_>>,
    params: &Map,
    body_members: &Map,
) -> Result<Vec<u8>, SerializeError> {
    if let Some(payload) = payload {
        let value = params
            .get(payload.name)
            .filter(|value| **value != Value::Null);
        return match (value, payload.raw) {
            (None, true) => Ok(Vec::new()),
            (Some(value), true) => encode_payload(value),
            (None, false) => Ok(b"{}".to_vec()),
            (Some(value), false) => serialize_body_params(model, value, payload.shape),
        };
    }
    if !body_members.is_empty() {
        return serialize_body_params(model, &Value::Map(body_members.clone()), shape);
    }
    let requires_empty_body = model
        .members(shape)
        .iter()
        .any(|member| model.shape(member.shape).traits.location.is_none());
    Ok(if requires_empty_body {
        b"{}".to_vec()
    } else {
        Vec::new()
    })
}

/// `_encode_payload`: text as UTF-8, bytes as they are. Anything else,
/// which botocore leaves in the body for the HTTP layer to fail on, is
/// refused; validation lets none through.
fn encode_payload(value: &Value) -> Result<Vec<u8>, SerializeError> {
    match value {
        Value::Blob(bytes) => Ok(bytes.clone()),
        Value::String(text) => Ok(text.as_bytes().to_vec()),
        other => Err(SerializeError::new(format!(
            "a bytes-like object is required, not '{}'",
            common::short_type(other)
        ))),
    }
}

/// `RestJSONSerializer._serialize_body_params`: `json.dumps` of a new dict
/// the value is serialized into. A structure's members are the dict's;
/// anything else (a document, say) is set under the key `None`, which
/// `json.dumps` writes as `"null"`.
fn serialize_body_params(
    model: &Model,
    value: &Value,
    shape: ShapeId,
) -> Result<Vec<u8>, SerializeError> {
    let resolved = model.shape(shape);
    let serialized = json::serialized(model, value, shape)?;
    let body = if matches!(resolved.kind, ShapeKind::Structure { .. })
        && !resolved.traits.is(Flag::Document)
    {
        serialized
    } else {
        let mut wrapper = Map::new();
        wrapper.insert("null".into(), serialized);
        Value::Map(wrapper)
    };
    dumps(&body, &Options::default(), None)
        .map(String::into_bytes)
        .map_err(|error| SerializeError::new(error.to_string()))
}

/// `RestJSONSerializer._serialize_content_type`: `application/json` for a
/// body that is not empty, unless the payload is a blob or a string or a
/// member set `Content-Type`.
fn serialize_content_type(payload: Option<Payload<'_>>, body: &[u8], headers: &mut Headers) {
    if payload.is_some_and(|payload| payload.raw) {
        return;
    }
    if !body.is_empty() && !headers.contains("Content-Type") {
        headers.set("Content-Type", "application/json");
    }
}

#[cfg(test)]
mod tests;
