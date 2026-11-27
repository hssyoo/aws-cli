//! The `smithy-rpc-v2-cbor` protocol: botocore's `RpcV2CBORSerializer` and
//! `RpcV2CBORParser` over the CBOR codec.
//!
//! Every operation is a `POST` to `/service/{targetPrefix}/operation/{name}`
//! whose input, when it has one, is a CBOR body. The output is the body's
//! data item read as `BaseRpcV2Parser` reads it: a structure's members by
//! member name, a list's items, and everything else as the decoder made it
//! (a map whole, a timestamp as the timestamp parser made its tag, a blob
//! as bytes, whatever the blob parser is). Botocore runs the timestamp
//! parser while it decodes, error bodies included, so the parse drivers run
//! over [`WithHooks`], which carries the call's hooks to the error step.
//!
//! Switched off: [`RPC_V2_CBOR`] is in [`super::SWITCHED_OFF`].
use super::{BotocoreCompat, ClientProtocol, ParseOptions, Serialized, common, compat};
use crate::codec::cbor::{CBOR, CborCodec, CborCursor, Item, decode};
use crate::codec::value::write_value;
use crate::error::{ParseError, SerializeError, ServiceError};
use crate::http::{Headers, HttpResponse, Method};
use model::schema::{Flag, OperationSchema, Schema, ShapeType, TraitId, TypeRegistry, traits};
use model::serde::{Codec, ShapeDeserializer};
use model::shapes::Model;
use model::value::{Map, Value};

/// Smithy's rpcv2Cbor, botocore's `smithy-rpc-v2-cbor`.
#[derive(Debug)]
pub struct RpcV2Protocol<C: Codec + 'static> {
    id: TraitId,
    codec: &'static C,
    compat: BotocoreCompat,
}

/// rpcv2Cbor. Its compat constants are the RPC parsers': the generic 5xx
/// error, and only `__type` and `x-amzn-query-error` name the code.
pub static RPC_V2_CBOR: RpcV2Protocol<CborCodec> = RpcV2Protocol {
    id: traits::RPC_V2_CBOR,
    codec: &CBOR,
    compat: BotocoreCompat::RPC,
};

impl RpcV2Protocol<CborCodec> {
    const fn with<'a>(&'a self, options: &'a ParseOptions<'a>) -> WithHooks<'a> {
        WithHooks {
            protocol: self,
            options,
        }
    }

    /// `_parse_payload`: the body's data item read through `shape`. An
    /// empty body is `b''`, which a structure with members cannot `get`
    /// from.
    fn payload(
        &self,
        shape: Schema<'_>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Map, ParseError> {
        let tree = body(&response.body, options)?;
        match parse_shape(shape, &mut CborCursor::new(&tree))? {
            Value::Map(map) => Ok(map),
            other => Err(ParseError::new(format!(
                "'{}' object is not iterable",
                model::pyrepr::py_short_type_name(&other)
            ))),
        }
    }

    /// `RpcV2CBORParser._do_error_parse`, tag 1 values through `options`.
    fn error_metadata(
        &self,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Map, ParseError> {
        let body = body(&response.body, options)?;
        if !matches!(body, Item::Map(_)) {
            return Err(ParseError::new(format!(
                "'{}' object has no attribute 'get'",
                body.type_name()
            )));
        }
        let message = match body.get("message").or_else(|| body.get("Message")) {
            Some(message) => message.to_value()?,
            None => Value::String(String::new()),
        };
        let mut error = Map::new();
        error.insert("Message".into(), message);
        error.insert("Code".into(), Value::String(String::new()));
        let code = match body.get("__type") {
            Some(code) if *code != Item::Scalar(Value::Null) => error_code(code)?,
            _ => Value::String(response.status.to_string()),
        };
        let code = match response.headers.joined("x-amzn-query-error") {
            Some(query_error) => query_compatible(code, &query_error, &mut error),
            None => code,
        };
        error.insert("Code".into(), code);
        let mut metadata = Map::new();
        if let Some(id) = response.headers.joined("x-amzn-requestid") {
            metadata.insert("RequestId".into(), Value::String(id));
        }
        let mut parsed = Map::new();
        parsed.insert("Error".into(), Value::Map(error));
        parsed.insert("ResponseMetadata".into(), Value::Map(metadata));
        Ok(parsed)
    }
}

/// `_initial_body_parse`: `b''` as it is, else the data item at its front.
fn body(body: &[u8], options: &ParseOptions<'_>) -> Result<Item, ParseError> {
    if body.is_empty() {
        return Ok(Item::Scalar(Value::Blob(Vec::new())));
    }
    decode(body, options.timestamp)
}

/// `ResponseParser._parse_shape` with `BaseRpcV2Parser`'s handlers: a
/// structure (a union or document among them) by its members, `None` ones
/// left out; a list's items, `None` ones kept; any other value as it was
/// decoded.
fn parse_shape(schema: Schema<'_>, at: &mut dyn ShapeDeserializer) -> Result<Value, ParseError> {
    match schema.shape_type() {
        ShapeType::Structure | ShapeType::Union | ShapeType::Document => {
            let mut parsed = Map::new();
            at.read_struct(schema, &mut |member, at| {
                if !at.is_null() {
                    let name = member.member_name().unwrap_or_default();
                    parsed.insert(name.to_owned(), parse_shape(member, at)?);
                }
                Ok(())
            })?;
            Ok(Value::Map(parsed))
        }
        ShapeType::List => {
            let item = common::list_member(schema).map_err(ParseError::new)?;
            let mut items = Vec::new();
            at.read_list(schema, &mut |at| {
                items.push(match at.is_null() {
                    true => Value::Null,
                    false => parse_shape(item, at)?,
                });
                Ok(())
            })?;
            Ok(Value::List(items))
        }
        _ => Ok(at.read_document(schema)?),
    }
}

/// The `__type` code without what follows a `:` and what precedes the
/// last `#`, Python's `in` and `split` of whatever the value is.
fn error_code(code: &Item) -> Result<Value, ParseError> {
    let contains = |sign: &str| match code {
        Item::Scalar(Value::String(text)) => Ok(text.contains(sign)),
        Item::Scalar(Value::Blob(_)) => Err(ParseError::new(
            "a bytes-like object is required, not 'str'",
        )),
        Item::List(items) => Ok(items.contains(&Item::Scalar(Value::String(sign.into())))),
        Item::Map(_) => Ok(code.get(sign).is_some()),
        Item::Scalar(_) => Err(ParseError::new(format!(
            "argument of type '{}' is not iterable",
            code.type_name()
        ))),
    };
    let (colon, hash) = (contains(":")?, contains("#")?);
    let Item::Scalar(Value::String(text)) = code else {
        if colon || hash {
            let method = if colon { "split" } else { "rsplit" };
            return Err(ParseError::new(format!(
                "'{}' object has no attribute '{method}'",
                code.type_name()
            )));
        }
        return code.to_value();
    };
    let text = text.split(':').next().unwrap_or(text);
    let text = text.rsplit('#').next().unwrap_or(text);
    Ok(Value::String(text.to_owned()))
}

/// `_do_query_compatible_error_parse`: a header of two parts, the first
/// set, is the code, with the body's code kept as `QueryErrorCode` and the
/// second part as `Type`.
fn query_compatible(code: Value, query_error: &str, error: &mut Map) -> Value {
    match query_error.split(';').collect::<Vec<_>>()[..] {
        [first, kind] if !first.is_empty() => {
            error.insert("QueryErrorCode".into(), code);
            error.insert("Type".into(), Value::String(kind.to_owned()));
            Value::String(first.to_owned())
        }
        _ => code,
    }
}

/// `operation_model.has_event_stream_output`.
fn has_event_stream(shape: Schema<'_>) -> bool {
    shape.members().any(|member| member.flag(Flag::EventStream))
}

impl ClientProtocol for RpcV2Protocol<CborCodec> {
    fn protocol_id(&self) -> TraitId {
        self.id
    }
    fn payload_codec(&self) -> Option<&dyn Codec> {
        Some(self.codec)
    }
    fn compat(&self) -> &BotocoreCompat {
        &self.compat
    }

    /// `BaseRpcV2Serializer.serialize_to_request` with
    /// `RpcV2CBORSerializer`'s body and headers.
    fn serialize_request(
        &self,
        operation: OperationSchema<'_>,
        input: &Value,
        model: &Model,
    ) -> Result<Serialized, SerializeError> {
        let target = (model.metadata().target_prefix.as_deref())
            .ok_or_else(|| SerializeError::new("'targetPrefix'"))?;
        let body = match operation.input() {
            Some(shape) => {
                let mut serializer = self.codec.serializer();
                write_value(shape, input, serializer.as_mut())?;
                serializer.finish()?
            }
            None => Vec::new(),
        };
        let host_prefix = common::host_prefix(operation, input)?;
        let mut headers = Headers::new();
        headers.set("smithy-protocol", "rpc-v2-cbor");
        if model.schema().service().query_compatible() {
            headers.set("x-amzn-query-mode", "true");
        }
        let media_type = match operation.output() {
            Some(output) if has_event_stream(output) => "application/vnd.amazon.eventstream",
            _ => self.codec.media_type(),
        };
        headers.set("Accept", media_type);
        if !body.is_empty() {
            headers.set("Content-Type", media_type);
        }
        Ok(Serialized {
            method: Method::Post,
            path: format!("/service/{target}/operation/{}", operation.name()),
            query: String::new(),
            headers,
            body,
            host_prefix,
        })
    }

    /// `BaseRpcV2Parser._do_parse`: the payload, then `ResponseMetadata`
    /// with the request id.
    fn deserialize_response(
        &self,
        _operation: OperationSchema<'_>,
        shape: Option<Schema<'_>>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Value, ParseError> {
        let Some(shape) = shape else {
            return Ok(Value::Map(Map::new()));
        };
        if shape.flag(Flag::EventStream) || has_event_stream(shape) {
            return Err(ParseError::new("event stream responses are not supported"));
        }
        let mut parsed = self.payload(shape, response, options)?;
        let mut metadata = Map::new();
        if let Some(id) = response.headers.joined("x-amzn-requestid") {
            metadata.insert("RequestId".into(), Value::String(id));
        }
        parsed.insert("ResponseMetadata".into(), Value::Map(metadata));
        Ok(Value::Map(parsed))
    }

    /// The error step with botocore's own timestamp parser, which
    /// `RpcV2CBORParser()` has; a call's parse runs it with its own hooks
    /// ([`WithHooks`]).
    fn parse_error_metadata(&self, response: &HttpResponse) -> Result<Map, ParseError> {
        self.error_metadata(response, &ParseOptions::BOTOCORE)
    }

    /// `BaseRpcV2Parser._do_modeled_error_parse`.
    fn deserialize_modeled_error(
        &self,
        shape: Schema<'_>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Map, ParseError> {
        self.payload(shape, response, options)
    }

    fn parse(
        &self,
        operation: OperationSchema<'_>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Value, ParseError> {
        compat::parse(
            &self.with(options),
            operation,
            operation.output(),
            response,
            options,
        )
    }

    fn parse_error(
        &self,
        operation: OperationSchema<'_>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
        registry: TypeRegistry<'_>,
    ) -> Result<ServiceError, ParseError> {
        compat::parse_error(&self.with(options), operation, response, options, registry)
    }
}

/// The protocol with one call's parse hooks, for the error step, which the
/// interface gives none: botocore's decoder runs the timestamp parser on
/// an error body's tags too.
#[derive(Debug)]
struct WithHooks<'a> {
    protocol: &'a RpcV2Protocol<CborCodec>,
    options: &'a ParseOptions<'a>,
}

impl ClientProtocol for WithHooks<'_> {
    fn protocol_id(&self) -> TraitId {
        self.protocol.protocol_id()
    }
    fn payload_codec(&self) -> Option<&dyn Codec> {
        self.protocol.payload_codec()
    }
    fn compat(&self) -> &BotocoreCompat {
        self.protocol.compat()
    }
    fn serialize_request(
        &self,
        operation: OperationSchema<'_>,
        input: &Value,
        model: &Model,
    ) -> Result<Serialized, SerializeError> {
        self.protocol.serialize_request(operation, input, model)
    }
    fn deserialize_response(
        &self,
        operation: OperationSchema<'_>,
        shape: Option<Schema<'_>>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Value, ParseError> {
        (self.protocol).deserialize_response(operation, shape, response, options)
    }
    fn parse_error_metadata(&self, response: &HttpResponse) -> Result<Map, ParseError> {
        self.protocol.error_metadata(response, self.options)
    }
    fn deserialize_modeled_error(
        &self,
        shape: Schema<'_>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Map, ParseError> {
        self.protocol
            .deserialize_modeled_error(shape, response, options)
    }
}

#[cfg(test)]
mod tests;
