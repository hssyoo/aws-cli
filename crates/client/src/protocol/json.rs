//! The `json` protocol, awsJson 1.0 and 1.1 by the model's `jsonVersion`:
//! botocore's `JSONSerializer` and `JSONParser`, with the query-compatible
//! mode (`awsQueryCompatible`), as an [`HttpRpcProtocol`] over the JSON
//! codec. The JSON error metadata, `RestJSONParser`'s included, is here.
use super::common;
use super::query::method;
use super::{BotocoreCompat, ClientProtocol, ParseOptions, Serialized};
use crate::codec::json::{self, JSON, JsonCodec};
use crate::codec::value::{build_value, write_value};
use crate::error::{ParseError, SerializeError};
use crate::http::{Headers, HttpResponse};
use model::pyrepr::py_str;
use model::schema::{Flag, OperationSchema, Schema, TraitId, traits};
use model::serde::Codec;
use model::shapes::Model;
use model::value::{Map, Value};

/// An RPC protocol over HTTP (Smithy's awsJson 1.0 and 1.1): every
/// operation a `POST` to `/` naming it in `X-Amz-Target`, its input and
/// output whole bodies in the codec's format.
#[derive(Debug)]
pub struct HttpRpcProtocol<C: Codec + 'static> {
    id: TraitId,
    codec: &'static C,
    compat: BotocoreCompat,
}

/// awsJson 1.0, botocore's `json` with `jsonVersion` 1.0.
pub static AWS_JSON_1_0: HttpRpcProtocol<JsonCodec> = HttpRpcProtocol {
    id: traits::AWS_JSON_1_0,
    codec: &JSON,
    compat: BotocoreCompat::RPC,
};

/// awsJson 1.1, botocore's `json` with `jsonVersion` 1.1.
pub static AWS_JSON_1_1: HttpRpcProtocol<JsonCodec> = HttpRpcProtocol {
    id: traits::AWS_JSON_1_1,
    codec: &JSON,
    compat: BotocoreCompat::RPC,
};

impl ClientProtocol for HttpRpcProtocol<JsonCodec> {
    fn protocol_id(&self) -> TraitId {
        self.id
    }
    fn payload_codec(&self) -> Option<&dyn Codec> {
        Some(self.codec)
    }
    fn compat(&self) -> &BotocoreCompat {
        &self.compat
    }

    /// `JSONSerializer.serialize_to_request`.
    fn serialize_request(
        &self,
        operation: OperationSchema<'_>,
        input: &Value,
        model: &Model,
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
                let mut serializer = self.codec.serializer();
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

    /// `JSONParser._do_parse`.
    fn deserialize_response(
        &self,
        _operation: OperationSchema<'_>,
        shape: Option<Schema<'_>>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Value, ParseError> {
        let parsed = match shape {
            Some(shape) if shape.flag(Flag::EventStream) || has_event_stream(shape) => {
                return Err(ParseError::new("event stream responses are not supported"));
            }
            Some(shape) => body(self.codec, shape, &response.body, options)?,
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

    fn parse_error_metadata(&self, response: &HttpResponse) -> Result<Map, ParseError> {
        error_metadata(&self.compat, response)
    }

    /// `JSONParser._do_modeled_error_parse`.
    fn deserialize_modeled_error(
        &self,
        shape: Schema<'_>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Map, ParseError> {
        match body(self.codec, shape, &response.body, options)? {
            Value::Map(map) => Ok(map),
            _ => Err(ParseError::new("'NoneType' object is not iterable")),
        }
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

/// A JSON protocol's `_do_error_parse`: `BaseJSONParser`'s, then, with
/// [`BotocoreCompat::rest_error_code`], the code `RestJSONParser.
/// _inject_error_code` finds.
pub(super) fn error_metadata(
    compat: &BotocoreCompat,
    response: &HttpResponse,
) -> Result<Map, ParseError> {
    if !compat.rest_error_code {
        return base_error_metadata(response);
    }
    let body = json::load(&response.body)?;
    if !matches!(body, Value::Map(_)) {
        // `body.get('message', ...)` raises before any code is read.
        let kind = common::short_type(&body);
        return Err(ParseError::new(format!(
            "'{kind}' object has no attribute 'get'"
        )));
    }
    let mut parsed = base_error_metadata(response)?;
    if let Some(code) = injected_code(&body, &response.headers)
        && let Some(Value::Map(error)) = parsed.get_mut("Error")
    {
        error.insert("Code".into(), code);
    }
    Ok(parsed)
}

/// `BaseJSONParser._do_error_parse`: the message from `message` or
/// `Message`; the code from `x-amzn-query-error` (with its `Type`), else
/// `__type`, else the status, without a `:` suffix or a `#` prefix.
fn base_error_metadata(response: &HttpResponse) -> Result<Map, ParseError> {
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

/// `RestJSONParser._inject_error_code`'s code: `x-amzn-errortype`, else the
/// body's `code`, else its `Code`, a text one without what follows a `:` and
/// what precedes the last `#`. `None`, which leaves `BaseJSONParser`'s code,
/// when there is none or it is `null`.
fn injected_code(body: &Value, headers: &Headers) -> Option<Value> {
    let code = match headers.joined("x-amzn-errortype") {
        Some(code) => Value::String(code),
        None => match body {
            Value::Map(body) => body.get("code").or_else(|| body.get("Code"))?.clone(),
            _ => return None,
        },
    };
    match code {
        Value::Null => None,
        Value::String(_) | Value::PyString(_) => {
            Some(error_code(&model::pyrepr::py_str_text(&code)))
        }
        other => Some(other),
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
