//! `RestJSONParser`: [`rest`]'s `BaseRestParser` over a JSON body, read by
//! the `json` protocol's reader with `RestJSONParser`'s handlers, and the
//! rest-json error code.
use super::RestJson;
use crate::codec::json::{self, REST_JSON};
use crate::codec::value::build_value;
use crate::error::ParseError;
use crate::http::{Headers, HttpResponse};
use crate::protocol::ParseOptions;
use crate::protocol::common::Family;
use crate::protocol::json::{Json, error_code};
use model::schema::Schema;
use model::value::{Map, Value};

mod rest;

impl Family for RestJson {
    /// `BaseRestParser._do_parse`.
    fn do_parse(
        &self,
        shape: Option<Schema<'_>>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Value, ParseError> {
        rest::do_parse(&reader(options), shape, response).map(Value::Map)
    }

    /// `RestJSONParser._do_error_parse`: `BaseJSONParser`'s message, code
    /// and request id, then the code `_inject_error_code` finds.
    fn do_error_parse(&self, response: &HttpResponse) -> Result<Map, ParseError> {
        let body = initial_body_parse(&response.body)?;
        if !matches!(body, Value::Map(_)) {
            // `body.get('message', ...)` raises before any code is read.
            return Err(rest::no_attribute(&body, "get"));
        }
        let mut parsed = Json.do_error_parse(response)?;
        if let Some(code) = injected_code(&body, &response.headers)
            && let Some(Value::Map(error)) = parsed.get_mut("Error")
        {
            error.insert("Code".into(), code);
        }
        Ok(parsed)
    }

    /// `BaseRestParser._do_modeled_error_parse`.
    fn do_modeled_error_parse(
        &self,
        shape: Schema<'_>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Map, ParseError> {
        rest::do_modeled_error_parse(&reader(options), shape, response)
    }
}

/// The body as `RestJSONParser` reads it: through the codec with its
/// handlers, which a header's text and the status code are read with too.
fn reader<'o>(options: &'o ParseOptions<'o>) -> Reader<'o> {
    Reader { options }
}

struct Reader<'o> {
    options: &'o ParseOptions<'o>,
}

impl rest::Body for Reader<'_> {
    type Parsed = Value;

    fn initial(&self, body: &[u8]) -> Result<Value, ParseError> {
        initial_body_parse(body)
    }

    fn parse(&self, shape: Schema<'_>, parsed: &Value) -> Result<Value, ParseError> {
        build_value(shape, &mut REST_JSON.cursor(parsed), self.options)
    }

    fn value(&self, shape: Schema<'_>, value: &Value) -> Result<Value, ParseError> {
        build_value(shape, &mut REST_JSON.cursor(value), self.options)
    }
}

/// `RestJSONParser._initial_body_parse`: `_parse_body_as_json`, as the json
/// protocol reads a body.
fn initial_body_parse(body: &[u8]) -> Result<Value, ParseError> {
    json::load(body)
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

#[cfg(test)]
mod tests;
