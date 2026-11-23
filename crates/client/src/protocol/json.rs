//! The `json` protocol, 1.0 and 1.1 by the model's `jsonVersion`:
//! botocore's `JSONSerializer` and `JSONParser`, with the query-compatible
//! mode (`awsQueryCompatible`).
use super::common::{self, Family};
use super::query::method;
use super::{ParseOptions, Protocol, Serialized};
use crate::error::{ParseError, SerializeError, ServiceError};
use crate::http::{Headers, HttpResponse};
use model::pyjson::{Options, dumps};
use model::pyrepr::py_str;
use model::shapes::{Flag, Model, Operation, ShapeId, ShapeKind};
use model::value::{Map, Value};

/// The `json` protocol.
#[derive(Debug)]
pub struct Json;

impl Protocol for Json {
    fn name(&self) -> &'static str {
        "json"
    }

    /// `JSONSerializer.serialize_to_request`.
    fn serialize(
        &self,
        model: &Model,
        operation: &Operation,
        input: &Value,
    ) -> Result<Serialized, SerializeError> {
        let metadata = model.metadata();
        let missing = |key: &str| SerializeError::new(format!("'{key}'"));
        let target = metadata
            .target_prefix
            .as_deref()
            .ok_or_else(|| missing("targetPrefix"))?;
        let version = metadata
            .json_version
            .as_deref()
            .ok_or_else(|| missing("jsonVersion"))?;
        let mut headers = Headers::new();
        headers.set("X-Amz-Target", format!("{target}.{}", operation.name));
        headers.set("Content-Type", format!("application/x-amz-json-{version}"));
        if metadata.aws_query_compatible {
            headers.set("x-amzn-query-mode", "true");
        }
        let body = match operation.input {
            Some(shape) => serialized(model, input, shape)?,
            None => Value::Map(Map::new()),
        };
        let body = dumps(&body, &Options::default(), None)
            .map_err(|error| SerializeError::new(error.to_string()))?;
        Ok(Serialized {
            method: method(operation)?,
            path: "/".into(),
            query: String::new(),
            headers,
            body: body.into_bytes(),
            host_prefix: common::host_prefix(model, operation, input)?,
        })
    }

    fn parse(
        &self,
        model: &Model,
        operation: &Operation,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Value, ParseError> {
        common::parse(self, model, operation.output, response, options)
    }

    fn parse_error(
        &self,
        model: &Model,
        operation: &Operation,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<ServiceError, ParseError> {
        common::parse_error(self, model, operation, response, options)
    }
}

/// `JSONSerializer._serialize`: the value as the body holds it.
pub(super) fn serialized(
    model: &Model,
    value: &Value,
    shape: ShapeId,
) -> Result<Value, SerializeError> {
    let resolved = model.shape(shape);
    match &resolved.kind {
        ShapeKind::Structure { .. } if resolved.traits.is(Flag::Document) => Ok(value.clone()),
        ShapeKind::Structure { .. } => structure(model, value, shape),
        ShapeKind::Map { value: item, .. } => {
            let Value::Map(map) = value else {
                return Err(SerializeError::new("a dict is required"));
            };
            let mut out = Map::with_capacity(map.len());
            for (key, entry) in map.iter() {
                let entry = if *entry == Value::Null {
                    Value::Null
                } else {
                    serialized(model, entry, *item)?
                };
                out.insert(key.to_owned(), entry);
            }
            Ok(Value::Map(out))
        }
        ShapeKind::List { member } => {
            let Value::List(items) = value else {
                return Err(SerializeError::new("a list is required"));
            };
            let out = items
                .iter()
                .map(|item| {
                    if *item == Value::Null {
                        Ok(Value::Null)
                    } else {
                        serialized(model, item, *member)
                    }
                })
                .collect::<Result<_, _>>()?;
            Ok(Value::List(out))
        }
        ShapeKind::Timestamp => {
            let format = resolved
                .traits
                .timestamp_format
                .as_deref()
                .unwrap_or("unixtimestamp");
            common::timestamp_out(value, format)
        }
        ShapeKind::Blob => common::base64_in(value).map(Value::String),
        ShapeKind::Float | ShapeKind::Double => Ok(common::special_float(value)),
        _ => Ok(value.clone()),
    }
}

/// `_serialize_type_structure`: members in the caller's order, under their
/// `locationName`.
fn structure(model: &Model, value: &Value, shape: ShapeId) -> Result<Value, SerializeError> {
    let Value::Map(map) = value else {
        return Err(SerializeError::new(format!(
            "'{}' object has no attribute 'items'",
            common::short_type(value)
        )));
    };
    let mut out = Map::with_capacity(map.len());
    for (key, item) in map.iter() {
        let member = common::member(model, shape, key)?;
        let name = model.shape(member).traits.name.as_deref().unwrap_or(key);
        out.insert(name.to_owned(), serialized(model, item, member)?);
    }
    Ok(Value::Map(out))
}

/// `BaseJSONParser._parse_body_as_json`: nothing is `{}`, and a body that
/// is not JSON is `{'message': body}`.
pub(super) fn body_json(body: &[u8]) -> Result<Value, ParseError> {
    if body.is_empty() {
        return Ok(Value::Map(Map::new()));
    }
    let text = std::str::from_utf8(body)
        .map_err(|error| ParseError::new(format!("'utf-8' codec can't decode bytes: {error}")))?;
    Ok(Value::from_json(body).unwrap_or_else(|_| {
        let mut map = Map::new();
        map.insert("message".into(), Value::String(text.to_owned()));
        Value::Map(map)
    }))
}

/// A subclass's own `_handle_<type>` for `shape`, ahead of
/// `BaseJSONParser`'s: the parsed value, or `None` to leave the shape to
/// the base.
pub(super) type Handler = fn(&Reader<'_, '_>, ShapeId, &Value) -> Option<Result<Value, ParseError>>;

pub(super) struct Reader<'m, 'o> {
    pub(super) model: &'m Model,
    pub(super) options: &'o ParseOptions<'o>,
    /// `None` for `JSONParser`, which overrides no handler.
    pub(super) handler: Option<Handler>,
}

impl Reader<'_, '_> {
    /// `_parse_shape`.
    pub(super) fn shape(&self, shape: ShapeId, value: &Value) -> Result<Value, ParseError> {
        if let Some(parsed) = self.handler.and_then(|handler| handler(self, shape, value)) {
            return parsed;
        }
        let resolved = self.model.shape(shape);
        match &resolved.kind {
            ShapeKind::Structure { .. } if resolved.traits.is(Flag::Document) => Ok(value.clone()),
            ShapeKind::Structure { .. } => self.structure(shape, value),
            ShapeKind::Map { key, value: item } => self.map((*key, *item), value),
            ShapeKind::List { member } => self.list(*member, value),
            ShapeKind::Blob => match value {
                Value::String(text) => (self.options.blob)(text),
                other => Err(ParseError::new(format!(
                    "argument should be a bytes-like object or ASCII string, not '{}'",
                    common::short_type(other)
                ))),
            },
            ShapeKind::Timestamp => (self.options.timestamp)(value),
            _ => Ok(value.clone()),
        }
    }

    /// `_handle_structure`: `null` stays `None`; members by their JSON
    /// names, `null` members left out.
    fn structure(&self, shape: ShapeId, value: &Value) -> Result<Value, ParseError> {
        let map = match value {
            Value::Null => return Ok(Value::Null),
            Value::Map(map) => map,
            other => {
                return Err(ParseError::new(format!(
                    "'{}' object has no attribute 'get'",
                    common::short_type(other)
                )));
            }
        };
        let present: Vec<&str> = map
            .iter()
            .filter(|(_, v)| **v != Value::Null)
            .map(|(k, _)| k)
            .collect();
        if let Some(tag) = common::unknown_union_member(self.model, shape, &present)? {
            return Ok(Value::Map(common::unknown_member_value(tag)));
        }
        let mut parsed = Map::new();
        for member in self.model.members(shape) {
            let name = self
                .model
                .shape(member.shape)
                .traits
                .name
                .as_deref()
                .unwrap_or(&member.name);
            match map.get(name) {
                None | Some(Value::Null) => {}
                Some(raw) => {
                    parsed.insert(member.name.to_string(), self.shape(member.shape, raw)?);
                }
            }
        }
        Ok(Value::Map(parsed))
    }

    /// `_handle_map`: keys through the key shape, `null` values kept.
    fn map(
        &self,
        (key_shape, value_shape): (ShapeId, ShapeId),
        value: &Value,
    ) -> Result<Value, ParseError> {
        let Value::Map(map) = value else {
            return Err(ParseError::new(format!(
                "'{}' object has no attribute 'items'",
                common::short_type(value)
            )));
        };
        let mut parsed = Map::with_capacity(map.len());
        for (key, item) in map.iter() {
            let key = match self.shape(key_shape, &Value::String(key.to_owned()))? {
                Value::String(key) => key,
                other => py_str(&other),
            };
            let item = if *item == Value::Null {
                Value::Null
            } else {
                self.shape(value_shape, item)?
            };
            parsed.insert(key, item);
        }
        Ok(Value::Map(parsed))
    }

    /// `ResponseParser._handle_list`: `null` items kept.
    pub(super) fn list(&self, member: ShapeId, value: &Value) -> Result<Value, ParseError> {
        let Value::List(items) = value else {
            return Err(ParseError::new(format!(
                "'{}' object is not iterable",
                common::short_type(value)
            )));
        };
        let parsed = items
            .iter()
            .map(|item| {
                if *item == Value::Null {
                    Ok(Value::Null)
                } else {
                    self.shape(member, item)
                }
            })
            .collect::<Result<_, _>>()?;
        Ok(Value::List(parsed))
    }

    /// `_handle_json_body`: a structure, or `None` for a `null` body.
    fn body(&self, shape: ShapeId, body: &[u8]) -> Result<Value, ParseError> {
        match self.shape(shape, &body_json(body)?)? {
            parsed @ (Value::Map(_) | Value::Null) => Ok(parsed),
            other => Err(ParseError::new(format!(
                "expected a structure, parsed {}",
                py_str(&other)
            ))),
        }
    }
}

/// `BaseJSONParser._inject_response_metadata`.
fn request_id(parsed: &mut Map, headers: &Headers) {
    if let Some(id) = headers.get("x-amzn-requestid") {
        let mut metadata = match parsed.remove("ResponseMetadata") {
            Some(Value::Map(metadata)) => metadata,
            _ => Map::new(),
        };
        metadata.insert("RequestId".into(), Value::String(id.to_owned()));
        parsed.insert("ResponseMetadata".into(), Value::Map(metadata));
    }
}

/// The text of a JSON value botocore reads with `.get`, `''` when absent.
fn text(body: &Value, key: &str) -> Option<Value> {
    match body {
        Value::Map(map) => map.get(key).cloned(),
        _ => None,
    }
}

impl Family for Json {
    /// `JSONParser._do_parse`.
    fn do_parse(
        &self,
        model: &Model,
        shape: Option<ShapeId>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Value, ParseError> {
        let parsed = match shape {
            Some(shape)
                if model.shape(shape).traits.is(Flag::EventStream)
                    || has_event_stream(model, shape) =>
            {
                return Err(ParseError::new("event stream responses are not supported"));
            }
            Some(shape) => Reader {
                model,
                options,
                handler: None,
            }
            .body(shape, &response.body)?,
            None => Value::Map(Map::new()),
        };
        match parsed {
            Value::Map(mut map) => {
                request_id(&mut map, &response.headers);
                Ok(Value::Map(map))
            }
            // `parsed.setdefault` on the `None` a `null` body parses to.
            _ if response.headers.contains("x-amzn-requestid") => Err(ParseError::new(
                "'NoneType' object has no attribute 'setdefault'",
            )),
            other => Ok(other),
        }
    }

    /// `BaseJSONParser._do_error_parse`: the message from `message` or
    /// `Message`; the code from `x-amzn-query-error` (with its `Type`), else
    /// `__type`, else the status, without a `:` suffix or a `#` prefix.
    fn do_error_parse(&self, response: &HttpResponse) -> Result<Map, ParseError> {
        let body = body_json(&response.body)?;
        let mut error = Map::new();
        let message = text(&body, "message").or_else(|| text(&body, "Message"));
        error.insert(
            "Message".into(),
            message.unwrap_or_else(|| Value::String(String::new())),
        );
        error.insert("Code".into(), Value::String(String::new()));
        let query_error = response.headers.get("x-amzn-query-error").unwrap_or("");
        let mut code = None;
        if let [first, kind] = query_error.split(';').collect::<Vec<_>>()[..]
            && !first.is_empty()
        {
            code = Some(Value::String(first.to_owned()));
            error.insert("Type".into(), Value::String(kind.to_owned()));
        }
        let code = code
            .or_else(|| text(&body, "__type"))
            .unwrap_or_else(|| Value::String(response.status.to_string()));
        error.insert("Code".into(), Value::String(error_code(&py_str(&code))));
        let mut parsed = Map::new();
        parsed.insert("Error".into(), Value::Map(error));
        parsed.insert("ResponseMetadata".into(), Value::Map(Map::new()));
        request_id(&mut parsed, &response.headers);
        Ok(parsed)
    }

    /// `JSONParser._do_modeled_error_parse`.
    fn do_modeled_error_parse(
        &self,
        model: &Model,
        shape: ShapeId,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Map, ParseError> {
        let reader = Reader {
            model,
            options,
            handler: None,
        };
        match reader.body(shape, &response.body)? {
            Value::Map(map) => Ok(map),
            _ => Err(ParseError::new("'NoneType' object is not iterable")),
        }
    }
}

/// A code without what follows a `:` and what precedes the last `#`.
pub(super) fn error_code(code: &str) -> String {
    let code = code.split_once(':').map_or(code, |(head, _)| head);
    code.rsplit_once('#')
        .map_or(code, |(_, tail)| tail)
        .to_owned()
}

/// `shape.event_stream_name`: a member marked `eventstream`.
fn has_event_stream(model: &Model, shape: ShapeId) -> bool {
    model
        .members(shape)
        .iter()
        .any(|member| model.shape(member.shape).traits.is(Flag::EventStream))
}

#[cfg(test)]
mod tests;
