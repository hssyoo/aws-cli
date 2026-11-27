//! The `json` protocol, 1.0 and 1.1 by the model's `jsonVersion`:
//! botocore's `JSONSerializer` and `JSONParser`, with the query-compatible
//! mode (`awsQueryCompatible`).
use super::common::{self, Family};
use super::query::method;
use super::{ParseOptions, Protocol, Serialized};
use crate::error::{ParseError, SerializeError, ServiceError};
use crate::http::{Headers, HttpResponse};
use model::pyjson::{LoadError, Options, dumps};
use model::pyrepr::py_str;
use model::schema::{Flag, OperationSchema, Schema, ShapeType};
use model::shapes::Model;
use model::value::{Key, Map, Value};

/// The `json` protocol.
#[derive(Debug)]
pub struct Json;

impl Protocol for Json {
    fn name(&self) -> &'static str {
        "json"
    }
    fn epoch_timestamps(&self) -> bool {
        true
    }

    /// `JSONSerializer.serialize_to_request`.
    fn serialize(
        &self,
        model: &Model,
        operation: OperationSchema<'_>,
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
        headers.set("X-Amz-Target", format!("{target}.{}", operation.name()));
        headers.set("Content-Type", format!("application/x-amz-json-{version}"));
        if model.schema().service().query_compatible() {
            headers.set("x-amzn-query-mode", "true");
        }
        let body = match operation.input() {
            Some(shape) => serialized(input, shape)?,
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
            host_prefix: common::host_prefix(operation, input)?,
        })
    }

    fn parse(
        &self,
        _model: &Model,
        operation: OperationSchema<'_>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Value, ParseError> {
        common::parse(self, operation.output(), response, options)
    }

    fn parse_error(
        &self,
        model: &Model,
        operation: OperationSchema<'_>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<ServiceError, ParseError> {
        common::parse_error(self, model, operation, response, options)
    }
}

/// `JSONSerializer._serialize`: the value as the body holds it.
pub(super) fn serialized(value: &Value, shape: Schema<'_>) -> Result<Value, SerializeError> {
    match shape.shape_type() {
        _ if common::is_structure(shape) && common::is_document(shape) => Ok(value.clone()),
        _ if common::is_structure(shape) => structure(value, shape),
        ShapeType::Map => {
            let (_, item) = common::map_entry(shape).map_err(SerializeError::new)?;
            let Value::Map(map) = value else {
                return Err(SerializeError::new("a dict is required"));
            };
            let mut out = Map::with_capacity(map.len());
            for (key, entry) in map.iter() {
                let entry = if *entry == Value::Null {
                    Value::Null
                } else {
                    serialized(entry, item)?
                };
                out.insert_key(key.clone(), entry);
            }
            Ok(Value::Map(out))
        }
        ShapeType::List => {
            let member = common::list_member(shape).map_err(SerializeError::new)?;
            let Value::List(items) = value else {
                return Err(SerializeError::new("a list is required"));
            };
            let out = items
                .iter()
                .map(|item| {
                    if *item == Value::Null {
                        Ok(Value::Null)
                    } else {
                        serialized(item, member)
                    }
                })
                .collect::<Result<_, _>>()?;
            Ok(Value::List(out))
        }
        ShapeType::Timestamp => {
            let format = shape.timestamp_format_text().unwrap_or("unixtimestamp");
            common::timestamp_out(value, format)
        }
        ShapeType::Blob => common::base64_in(value).map(Value::String),
        ShapeType::Float | ShapeType::Double => Ok(common::special_float(value)),
        _ => Ok(value.clone()),
    }
}

/// `_serialize_type_structure`: members in the caller's order, under their
/// `locationName`.
fn structure(value: &Value, shape: Schema<'_>) -> Result<Value, SerializeError> {
    let Value::Map(map) = value else {
        return Err(SerializeError::new(format!(
            "'{}' object has no attribute 'items'",
            common::short_type(value)
        )));
    };
    let mut out = Map::with_capacity(map.len());
    for (key, item) in map.iter() {
        let member = common::member(shape, key)?;
        // A member's key is its name, which `member` found.
        let name = member.location_name();
        let name = name.map_or_else(|| key.clone(), Key::from);
        out.insert_key(name, serialized(item, member)?);
    }
    Ok(Value::Map(out))
}

/// `BaseJSONParser._parse_body_as_json`: nothing is `{}`, the body decoded
/// as UTF-8 (whose `UnicodeDecodeError` botocore does not catch), and text
/// `json.loads` refuses is `{'message': body}`. A lone surrogate is kept.
/// Valid text nested past the reader's depth limit fails the call, naming
/// the limit: the request was sent, so the Python route cannot take it
/// over without sending it again.
pub(super) fn body_json(body: &[u8]) -> Result<Value, ParseError> {
    if body.is_empty() {
        return Ok(Value::Map(Map::new()));
    }
    let text = common::decoded(body)?;
    match model::pyjson::loads(text) {
        Ok(value) => Ok(value),
        Err(LoadError::Invalid(_)) => {
            let mut map = Map::new();
            map.insert("message".into(), Value::String(text.to_owned()));
            Ok(Value::Map(map))
        }
        Err(LoadError::Limit(limit)) => Err(common::beyond_limit(limit)),
    }
}

/// A subclass's own `_handle_<type>` for `shape`, ahead of
/// `BaseJSONParser`'s: the parsed value, or `None` to leave the shape to
/// the base.
pub(super) type Handler = fn(&Reader<'_>, Schema<'_>, &Value) -> Option<Result<Value, ParseError>>;

pub(super) struct Reader<'o> {
    pub(super) options: &'o ParseOptions<'o>,
    /// `None` for `JSONParser`, which overrides no handler.
    pub(super) handler: Option<Handler>,
}

impl Reader<'_> {
    /// `_parse_shape`.
    pub(super) fn shape(&self, shape: Schema<'_>, value: &Value) -> Result<Value, ParseError> {
        if let Some(parsed) = self.handler.and_then(|handler| handler(self, shape, value)) {
            return parsed;
        }
        match shape.shape_type() {
            _ if common::is_structure(shape) && common::is_document(shape) => Ok(value.clone()),
            _ if common::is_structure(shape) => self.structure(shape, value),
            ShapeType::Map => self.map(common::map_entry(shape).map_err(ParseError::new)?, value),
            ShapeType::List => {
                self.list(common::list_member(shape).map_err(ParseError::new)?, value)
            }
            ShapeType::Blob => match value {
                Value::String(_) | Value::PyString(_) => (self.options.blob)(value),
                other => Err(ParseError::new(format!(
                    "argument should be a bytes-like object or ASCII string, not '{}'",
                    common::short_type(other)
                ))),
            },
            ShapeType::Timestamp => (self.options.timestamp)(value),
            _ => Ok(value.clone()),
        }
    }

    /// `_handle_structure`: `null` stays `None`; members by their JSON
    /// names, `null` members left out.
    fn structure(&self, shape: Schema<'_>, value: &Value) -> Result<Value, ParseError> {
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
        let present: Vec<&Key> = map
            .iter()
            .filter(|(_, v)| **v != Value::Null)
            .map(|(k, _)| k)
            .collect();
        if let Some(tag) = common::unknown_union_member(shape, &present)? {
            return Ok(Value::Map(common::unknown_member_value(tag)));
        }
        let mut parsed = Map::new();
        for member in shape.members() {
            match map.get(common::wire_name(member)) {
                None | Some(Value::Null) => {}
                Some(raw) => {
                    let name = common::member_name(member);
                    parsed.insert(name.to_owned(), self.shape(member, raw)?);
                }
            }
        }
        Ok(Value::Map(parsed))
    }

    /// `_handle_map`: keys through the key shape, `null` values kept.
    fn map(
        &self,
        (key_shape, value_shape): (Schema<'_>, Schema<'_>),
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
            let key = match self.shape(key_shape, &key.to_value())? {
                Value::String(key) => Key::Str(key),
                Value::PyString(key) => Key::Py(key),
                other => Key::Str(py_str(&other)),
            };
            let item = if *item == Value::Null {
                Value::Null
            } else {
                self.shape(value_shape, item)?
            };
            parsed.insert_key(key, item);
        }
        Ok(Value::Map(parsed))
    }

    /// `ResponseParser._handle_list`: `null` items kept.
    pub(super) fn list(&self, member: Schema<'_>, value: &Value) -> Result<Value, ParseError> {
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
    fn body(&self, shape: Schema<'_>, body: &[u8]) -> Result<Value, ParseError> {
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
    if let Some(id) = headers.joined("x-amzn-requestid") {
        let mut metadata = match parsed.remove("ResponseMetadata") {
            Some(Value::Map(metadata)) => metadata,
            _ => Map::new(),
        };
        metadata.insert("RequestId".into(), Value::String(id));
        parsed.insert("ResponseMetadata".into(), Value::Map(metadata));
    }
}

impl Family for Json {
    /// `JSONParser._do_parse`.
    fn do_parse(
        &self,
        shape: Option<Schema<'_>>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Value, ParseError> {
        let parsed = match shape {
            Some(shape) if shape.flag(Flag::EventStream) || has_event_stream(shape) => {
                return Err(ParseError::new("event stream responses are not supported"));
            }
            Some(shape) => Reader {
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
        let message = body
            .get("message")
            .cloned()
            .or_else(|| body.get("Message").cloned());
        error.insert(
            "Message".into(),
            message.unwrap_or_else(|| Value::String(String::new())),
        );
        error.insert("Code".into(), Value::String(String::new()));
        let query_error = response
            .headers
            .joined("x-amzn-query-error")
            .unwrap_or_default();
        let mut code = None;
        if let [first, kind] = query_error.split(';').collect::<Vec<_>>()[..]
            && !first.is_empty()
        {
            code = Some(Value::String(first.to_owned()));
            error.insert("Type".into(), Value::String(kind.to_owned()));
        }
        let code = code
            .or_else(|| body.get("__type").cloned())
            .unwrap_or_else(|| Value::String(response.status.to_string()));
        error.insert(
            "Code".into(),
            error_code(&model::pyrepr::py_str_text(&code)),
        );
        let mut parsed = Map::new();
        parsed.insert("Error".into(), Value::Map(error));
        parsed.insert("ResponseMetadata".into(), Value::Map(Map::new()));
        request_id(&mut parsed, &response.headers);
        Ok(parsed)
    }

    /// `JSONParser._do_modeled_error_parse`.
    fn do_modeled_error_parse(
        &self,
        shape: Schema<'_>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Map, ParseError> {
        let reader = Reader {
            options,
            handler: None,
        };
        match reader.body(shape, &response.body)? {
            Value::Map(map) => Ok(map),
            _ => Err(ParseError::new("'NoneType' object is not iterable")),
        }
    }
}

/// A code without what follows a `:` and what precedes the last `#`, of
/// a `str` in `pytext::wtf8`'s encoding, the `str` it leaves: a lone
/// surrogate kept. `:` and `#` are ASCII, so no cut splits a code point.
pub(super) fn error_code(code: &[u8]) -> Value {
    let code = code.split(|byte| *byte == b':').next().unwrap_or(code);
    let code = code
        .iter()
        .rposition(|byte| *byte == b'#')
        .map_or(code, |hash| &code[hash + 1..]);
    Value::from_text(model::value::Wtf8Buf::from_bytes(code.to_vec()))
}

/// `shape.event_stream_name`: a member marked `eventstream`.
fn has_event_stream(shape: Schema<'_>) -> bool {
    shape.members().any(|member| member.flag(Flag::EventStream))
}

#[cfg(test)]
mod tests;
