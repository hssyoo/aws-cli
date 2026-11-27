//! The `rest-json` protocol: botocore's `RestJSONSerializer` and
//! `RestJSONParser`. The request side is in [`serialize`], the response
//! side in [`parse`]; both reuse the `json` protocol's body rules, as an
//! [`HttpBindingProtocol`] over the JSON codec.
use super::{BotocoreCompat, ClientProtocol, ParseOptions, Serialized, json};
use crate::codec::json::{JsonCodec, REST_JSON};
use crate::error::{ParseError, SerializeError};
use crate::http::HttpResponse;
use model::schema::{OperationSchema, Schema, TraitId, traits};
use model::serde::Codec;
use model::shapes::Model;
use model::value::{Map, Value};

pub(super) mod parse;
pub(super) mod serialize;

/// A protocol whose members bind to the HTTP message (Smithy's restJson1,
/// later restXml): the URI, query, headers and status, with the rest in a
/// body of the codec's format.
#[derive(Debug)]
pub struct HttpBindingProtocol<C: Codec + 'static> {
    pub(super) id: TraitId,
    pub(super) codec: &'static C,
    pub(super) compat: BotocoreCompat,
}

/// restJson1, botocore's `rest-json`.
pub static REST_JSON_1: HttpBindingProtocol<JsonCodec> = HttpBindingProtocol {
    id: traits::REST_JSON_1,
    codec: &REST_JSON,
    compat: BotocoreCompat::REST,
};

impl ClientProtocol for HttpBindingProtocol<JsonCodec> {
    fn protocol_id(&self) -> TraitId {
        self.id
    }
    fn payload_codec(&self) -> Option<&dyn Codec> {
        Some(self.codec)
    }
    fn compat(&self) -> &BotocoreCompat {
        &self.compat
    }

    /// `RestJSONSerializer.serialize_to_request`.
    fn serialize_request(
        &self,
        operation: OperationSchema<'_>,
        input: &Value,
        _model: &Model,
    ) -> Result<Serialized, SerializeError> {
        serialize::request(operation, input)
    }

    /// `BaseRestParser._do_parse`.
    fn deserialize_response(
        &self,
        _operation: OperationSchema<'_>,
        shape: Option<Schema<'_>>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Value, ParseError> {
        parse::do_parse(self.codec, shape, response, options).map(Value::Map)
    }

    /// `RestJSONParser._do_error_parse`: `BaseJSONParser`'s message, code
    /// and request id, then the code `_inject_error_code` finds.
    fn parse_error_metadata(&self, response: &HttpResponse) -> Result<Map, ParseError> {
        json::error_metadata(&self.compat, response)
    }

    /// `BaseRestParser._do_modeled_error_parse`.
    fn deserialize_modeled_error(
        &self,
        shape: Schema<'_>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Map, ParseError> {
        parse::do_modeled_error_parse(self.codec, shape, response, options)
    }
}
