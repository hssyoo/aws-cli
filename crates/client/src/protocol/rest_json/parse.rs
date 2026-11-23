//! `RestJSONParser`: the status code, header and payload members, the JSON
//! body, and the rest-json error code.
use super::RestJson;
use crate::error::ParseError;
use crate::http::HttpResponse;
use crate::protocol::ParseOptions;
use crate::protocol::common::Family;
use model::shapes::{Model, ShapeId};
use model::value::{Map, Value};

impl Family for RestJson {
    /// `BaseRestParser._do_parse`.
    fn do_parse(
        &self,
        _model: &Model,
        _shape: Option<ShapeId>,
        _response: &HttpResponse,
        _options: &ParseOptions<'_>,
    ) -> Result<Value, ParseError> {
        Err(ParseError::new("rest-json responses are not parsed yet"))
    }

    /// `RestJSONParser._do_error_parse`.
    fn do_error_parse(&self, _response: &HttpResponse) -> Result<Map, ParseError> {
        Err(ParseError::new("rest-json errors are not parsed yet"))
    }

    /// `BaseRestParser._do_modeled_error_parse`.
    fn do_modeled_error_parse(
        &self,
        _model: &Model,
        _shape: ShapeId,
        _response: &HttpResponse,
        _options: &ParseOptions<'_>,
    ) -> Result<Map, ParseError> {
        Err(ParseError::new("rest-json errors are not parsed yet"))
    }
}
