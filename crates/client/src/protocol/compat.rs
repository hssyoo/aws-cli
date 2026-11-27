//! botocore's response-parser drivers over a [`ClientProtocol`]'s steps
//! (`ResponseParser.parse`, `Endpoint._add_modeled_error_fields`), in one
//! place for every protocol, and the [`BotocoreCompat`] constants where a
//! protocol departs from Smithy as botocore does.
use super::{ClientProtocol, ParseOptions};
use crate::error::{ParseError, ServiceError};
use crate::http::{Headers, HttpResponse};
use model::schema::{Flag, OperationSchema, Schema, TypeRegistry, traits};
use model::value::{Map, Value};

/// Where a protocol instance departs from Smithy's protocol tests as
/// botocore does, fixed where the instance is built. Each field names the
/// cases of botocore's protocol-test ignore list
/// (`tests/unit/botocore/protocols/protocol-tests-ignore-list.json`) it
/// accounts for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BotocoreCompat {
    /// `ResponseParser._is_generic_error_response`: a 5xx with an empty or
    /// HTML body is the status and its reason phrase, whatever its headers
    /// say (`AwsJson10FooErrorUsingXAmznErrorType`,
    /// `AwsJson11FooErrorUsingXAmznErrorType`,
    /// `RestJsonFooErrorUsingXAmznErrorType` and their `WithUri` and
    /// `WithUriAndNamespace` cases).
    pub generic_server_error: bool,
    /// `RestJSONParser._inject_error_code`: the error code from
    /// `x-amzn-errortype`, else the body's `code`, else its `Code`. Off,
    /// only `x-amzn-query-error` and `__type` name the code, as
    /// `JSONParser` reads them (`AwsJson10FooErrorUsingCode`,
    /// `AwsJson11FooErrorUsingCode` and their `AndNamespace` and
    /// `UriAndNamespace` cases).
    pub rest_error_code: bool,
}

impl BotocoreCompat {
    /// botocore's RPC parsers (`JSONParser`, `QueryParser`).
    pub const RPC: Self = Self {
        generic_server_error: true,
        rest_error_code: false,
    };
    /// botocore's REST parsers (`RestJSONParser`).
    pub const REST: Self = Self {
        generic_server_error: true,
        rest_error_code: true,
    };
}

/// Whether `protocol` sends a timestamp member with no `timestampFormat`
/// as an epoch number (`JSONSerializer.TIMESTAMP_FORMAT`, `unixtimestamp`)
/// rather than ISO 8601 text (`QuerySerializer`'s and the XML protocols'
/// `iso8601`). `RestJSONSerializer` keeps `JSONSerializer`'s body format; a
/// header timestamp is RFC 822 text, which carries its own offset.
pub fn timestamps_epoch(protocol: &dyn ClientProtocol) -> bool {
    matches!(
        protocol.protocol_id(),
        traits::AWS_JSON_1_0 | traits::AWS_JSON_1_1 | traits::REST_JSON_1
    )
}

/// `ResponseParser.parse`: the error paths from 301, a modelled error
/// shape's members alone, and `ResponseMetadata` added to the rest.
///
/// # Errors
/// `ResponseParserError`: a body the protocol cannot read.
pub fn parse<P: ClientProtocol + ?Sized>(
    protocol: &P,
    operation: OperationSchema<'_>,
    shape: Option<Schema<'_>>,
    response: &HttpResponse,
    options: &ParseOptions<'_>,
) -> Result<Value, ParseError> {
    let exception = shape.filter(|shape| shape.flag(Flag::Error));
    let parsed = if response.status < 301 {
        protocol.deserialize_response(operation, shape, response, options)?
    } else if protocol.compat().generic_server_error && is_generic_error(response) {
        Value::Map(generic_error(response.status))
    } else if let Some(shape) = exception {
        return protocol
            .deserialize_modeled_error(shape, response, options)
            .map(Value::Map);
    } else {
        Value::Map(protocol.parse_error_metadata(response)?)
    };
    // `if isinstance(parsed, dict)`: a `None` output gets no metadata.
    let Value::Map(mut parsed) = parsed else {
        return Ok(parsed);
    };
    if shape.is_some_and(|shape| shape.flag(Flag::EventStream)) {
        return Ok(Value::Map(parsed));
    }
    // `parsed.get('ResponseMetadata', {})`, updated and set back: a
    // metadata dict the parser made keeps its place (the REST parsers put
    // it first), and a new one goes last.
    let mut metadata = match parsed.get("ResponseMetadata") {
        Some(Value::Map(metadata)) => metadata.clone(),
        _ => Map::new(),
    };
    metadata.insert(
        "HTTPStatusCode".into(),
        Value::Integer(i64::from(response.status)),
    );
    metadata.insert(
        "HTTPHeaders".into(),
        Value::Map(lowercase_headers(&response.headers)),
    );
    parsed.insert("ResponseMetadata".into(), Value::Map(metadata));
    Ok(Value::Map(parsed))
}

/// `parse` then, when the error's code names a modelled error,
/// `Endpoint._add_modeled_error_fields`' second parse with that shape,
/// merged into the first.
///
/// # Errors
/// `ResponseParserError`: a body the protocol cannot read.
pub fn parse_error<P: ClientProtocol + ?Sized>(
    protocol: &P,
    operation: OperationSchema<'_>,
    response: &HttpResponse,
    options: &ParseOptions<'_>,
    registry: TypeRegistry<'_>,
) -> Result<ServiceError, ParseError> {
    let output = operation.output();
    let Value::Map(mut parsed) = parse(protocol, operation, output, response, options)? else {
        return Err(ParseError::new("'NoneType' object has no attribute 'get'"));
    };
    let code = match parsed.get("Error") {
        Some(Value::Map(error)) => error.get("Code").and_then(Value::as_str).map(str::to_owned),
        _ => None,
    };
    if let Some(shape) = code
        .as_deref()
        .and_then(|code| registry.entry_for_error_code(code))
    {
        let Value::Map(modelled) = parse(protocol, operation, Some(shape), response, options)?
        else {
            return Err(ParseError::new("'NoneType' object is not iterable"));
        };
        for (key, value) in modelled {
            parsed.insert_key(key, value);
        }
    }
    Ok(service_error(operation.name(), response, parsed))
}

/// The parsed error response as `ClientError` holds it.
fn service_error(operation: &str, response: &HttpResponse, parsed: Map) -> ServiceError {
    let mut error = ServiceError::new(operation, response.status);
    error.headers = response.headers.clone();
    for (key, value) in parsed {
        match value {
            Value::Map(map) if key == *"Error" => error.error = map,
            Value::Map(metadata) if key == *"ResponseMetadata" => {
                error.request_id = metadata
                    .get("RequestId")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            value => {
                error.members.insert_key(key, value);
            }
        }
    }
    error
}

/// `ResponseParser._is_generic_error_response`: a 5xx without a body, or
/// one that is HTML.
fn is_generic_error(response: &HttpResponse) -> bool {
    if response.status < 500 {
        return false;
    }
    let body = response.body.trim_ascii();
    body.is_empty() || body.starts_with(b"<html>")
}

/// `ResponseParser._do_generic_error_parse`.
fn generic_error(status: u16) -> Map {
    let mut error = Map::new();
    error.insert("Code".into(), Value::String(status.to_string()));
    error.insert(
        "Message".into(),
        Value::String(reason_phrase(status).into()),
    );
    let mut parsed = Map::new();
    parsed.insert("Error".into(), Value::Map(error));
    parsed.insert("ResponseMetadata".into(), Value::Map(Map::new()));
    parsed
}

/// `http.client.responses` for the statuses the generic path reads (5xx),
/// in CPython 3.14.
fn reason_phrase(status: u16) -> &'static str {
    match status {
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        506 => "Variant Also Negotiates",
        507 => "Insufficient Storage",
        508 => "Loop Detected",
        510 => "Not Extended",
        511 => "Network Authentication Required",
        _ => "",
    }
}

/// `botocore.utils.lowercase_dict` of the response headers: names
/// lowercased, a repeated header's values joined with `, ` as urllib3
/// joins them.
fn lowercase_headers(headers: &Headers) -> Map {
    let mut map = Map::new();
    for (name, value) in headers.iter() {
        let name = name.to_ascii_lowercase();
        let joined = match map.get(&name) {
            Some(Value::String(old)) => format!("{old}, {value}"),
            _ => value.to_owned(),
        };
        map.insert(name, Value::String(joined));
    }
    map
}

#[cfg(test)]
mod tests;
