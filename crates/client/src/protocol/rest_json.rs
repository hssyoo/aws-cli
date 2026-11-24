//! The `rest-json` protocol: botocore's `RestJSONSerializer` and
//! `RestJSONParser`. The request side is in [`serialize`], the response
//! side in [`parse`]; both reuse the `json` protocol's body rules.
use super::{ParseOptions, Protocol, Serialized, common};
use crate::error::{ParseError, SerializeError, ServiceError};
use crate::http::HttpResponse;
use model::shapes::{Model, Operation};
use model::value::Value;

mod parse;
mod serialize;

/// The `rest-json` protocol.
#[derive(Debug)]
pub struct RestJson;

impl Protocol for RestJson {
    fn name(&self) -> &'static str {
        "rest-json"
    }

    /// `RestJSONSerializer` keeps `JSONSerializer`'s body format; a header
    /// timestamp is RFC 822 text, which carries its own offset.
    fn epoch_timestamps(&self) -> bool {
        true
    }

    /// `RestJSONSerializer.serialize_to_request`.
    fn serialize(
        &self,
        model: &Model,
        operation: &Operation,
        input: &Value,
    ) -> Result<Serialized, SerializeError> {
        serialize::request(model, operation, input)
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
