//! The `json` protocol, 1.0 and 1.1 by the model's `jsonVersion`:
//! botocore's `JSONSerializer` and `JSONParser`, with the query-compatible
//! mode (`awsQueryCompatible`).
use super::common::{self, Family};
use super::query::method;
use super::{ParseOptions, Protocol, Serialized};
use crate::codec::json::{self, JSON, JsonCodec};
use crate::codec::value::{build_value, write_value};
use crate::error::{ParseError, SerializeError, ServiceError};
use crate::http::{Headers, HttpResponse};
use model::pyrepr::py_str;
use model::schema::{Flag, OperationSchema, Schema};
use model::serde::Codec;
use model::shapes::Model;
use model::value::{Map, Value};

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
        // `JSONSerializer._serialize` of the input, `{}` without one.
        let body = match operation.input() {
            Some(shape) => {
                let mut serializer = JSON.serializer();
                write_value(shape, input, serializer.as_mut())?;
                serializer.finish()?
            }
            None => json::dumps(&Value::Map(Map::new()))?,
        };
        Ok(Serialized {
            method: method(operation)?,
            path: "/".into(),
            query: String::new(),
            headers,
            body,
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

/// `_handle_json_body` through `codec`: a structure, or `None` for a
/// `null` body.
pub(super) fn body(
    codec: &JsonCodec,
    shape: Schema<'_>,
    body: &[u8],
    options: &ParseOptions<'_>,
) -> Result<Value, ParseError> {
    let mut deserializer = codec.deserializer(body)?;
    match build_value(shape, deserializer.as_mut(), options)? {
        parsed @ (Value::Map(_) | Value::Null) => Ok(parsed),
        other => Err(ParseError::new(format!(
            "expected a structure, parsed {}",
            py_str(&other)
        ))),
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
            Some(shape) => body(&JSON, shape, &response.body, options)?,
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
        let body = json::load(&response.body)?;
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
        match body(&JSON, shape, &response.body, options)? {
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
