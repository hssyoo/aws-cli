//! `RestJSONParser`: [`rest`]'s `BaseRestParser` over a JSON body, read by
//! the codec with `RestJSONParser`'s handlers. Its error code is
//! [`crate::protocol::json::error_metadata`]'s.
use crate::codec::json::{self, JsonCodec};
use crate::codec::value::build_value;
use crate::error::ParseError;
use crate::http::HttpResponse;
use crate::protocol::ParseOptions;
use model::schema::Schema;
use model::value::{Map, Value};

mod rest;

/// `BaseRestParser._do_parse` with `codec`.
pub(super) fn do_parse(
    codec: &JsonCodec,
    shape: Option<Schema<'_>>,
    response: &HttpResponse,
    options: &ParseOptions<'_>,
) -> Result<Map, ParseError> {
    rest::do_parse(&reader(codec, options), shape, response)
}

/// `BaseRestParser._do_modeled_error_parse` with `codec`.
pub(super) fn do_modeled_error_parse(
    codec: &JsonCodec,
    shape: Schema<'_>,
    response: &HttpResponse,
    options: &ParseOptions<'_>,
) -> Result<Map, ParseError> {
    rest::do_modeled_error_parse(&reader(codec, options), shape, response)
}

/// The body as `RestJSONParser` reads it: through the codec with its
/// handlers, which a header's text and the status code are read with too.
fn reader<'o>(codec: &'o JsonCodec, options: &'o ParseOptions<'o>) -> Reader<'o> {
    Reader { codec, options }
}

struct Reader<'o> {
    codec: &'o JsonCodec,
    options: &'o ParseOptions<'o>,
}

impl rest::Body for Reader<'_> {
    type Parsed = Value;

    fn initial(&self, body: &[u8]) -> Result<Value, ParseError> {
        initial_body_parse(body)
    }

    fn parse(&self, shape: Schema<'_>, parsed: &Value) -> Result<Value, ParseError> {
        build_value(shape, &mut self.codec.cursor(parsed), self.options)
    }

    fn value(&self, shape: Schema<'_>, value: &Value) -> Result<Value, ParseError> {
        build_value(shape, &mut self.codec.cursor(value), self.options)
    }
}

/// `RestJSONParser._initial_body_parse`: `_parse_body_as_json`, as the json
/// protocol reads a body.
fn initial_body_parse(body: &[u8]) -> Result<Value, ParseError> {
    json::load(body)
}

#[cfg(test)]
mod tests;
