//! `RestXMLParser`: `BaseRestParser` over an XML body, read by the XML
//! codec; a header's text and the status code read with
//! `BaseXMLResponseParser`'s scalar handlers ([`HeaderValue`]); and the
//! error envelope `_do_error_parse` reads, S3's `<Error>` or the
//! `<ErrorResponse>` of the other rest-xml services, else the status.
use crate::codec::json::json_value;
use crate::codec::value::build_value;
use crate::codec::xml::{self as codec, Node, XmlDeserializer, by_name, replaced};
use crate::error::ParseError;
use crate::http::HttpResponse;
use crate::protocol::ParseOptions;
use crate::protocol::rest_json::parse::rest;
use crate::xml::Element;
use model::pyrepr::{float_from_text, int_from_text, py_repr, py_short_type_name};
use model::schema::{Schema, ShapeType};
use model::serde::{Read, SerdeError, SerdeResult, ShapeDeserializer};
use model::value::{Key, Map, Value};

/// `BaseRestParser._do_parse`.
pub(super) fn do_parse(
    shape: Option<Schema<'_>>,
    response: &HttpResponse,
    options: &ParseOptions<'_>,
) -> Result<Map, ParseError> {
    rest::do_parse(&Reader { options }, shape, response)
}

/// `BaseRestParser._do_modeled_error_parse`.
pub(super) fn do_modeled_error_parse(
    shape: Schema<'_>,
    response: &HttpResponse,
    options: &ParseOptions<'_>,
) -> Result<Map, ParseError> {
    rest::do_modeled_error_parse(&Reader { options }, shape, response)
}

/// The body as `RestXMLParser` reads it.
struct Reader<'o> {
    options: &'o ParseOptions<'o>,
}

impl rest::Body for Reader<'_> {
    type Parsed = Element;

    /// `_initial_body_parse`: no body is an element with no children.
    fn initial(&self, body: &[u8]) -> Result<Element, ParseError> {
        if body.is_empty() {
            return Ok(Element::default());
        }
        Ok(codec::parse(body)?)
    }

    fn parse(&self, shape: Schema<'_>, parsed: &Element) -> Result<Value, ParseError> {
        build_value(
            shape,
            &mut XmlDeserializer::new(Node::One(parsed)),
            self.options,
        )
    }

    fn value(&self, shape: Schema<'_>, value: &Value) -> Result<Value, ParseError> {
        build_value(shape, &mut HeaderValue(value), self.options)
    }
}

/// A header's text, or the status code, as `_parse_shape` reads it with
/// `RestXMLParser`'s handlers, which `_text_content` hands the value
/// itself: a boolean is whether the text is `true`, an integer `int()` of
/// it, a float `float()` of it, a `jsonvalue` string the JSON its base64
/// holds, and a list the comma-separated parts, each stripped. Timestamps
/// and blobs stay raw for the parse hooks.
#[derive(Clone, Copy, Debug)]
struct HeaderValue<'v>(&'v Value);

impl HeaderValue<'_> {
    /// The `TypeError` or `AttributeError` of iterating the value as an
    /// XML node: text is characters, which have no `tag`.
    fn not_a_node(&self) -> SerdeError {
        match self.0 {
            Value::String(_) => SerdeError::new("'str' object has no attribute 'tag'"),
            other => SerdeError::new(format!(
                "'{}' object is not iterable",
                py_short_type_name(other)
            )),
        }
    }
}

impl ShapeDeserializer for HeaderValue<'_> {
    fn read_struct(
        &mut self,
        _schema: Schema<'_>,
        _each: &mut dyn FnMut(Schema<'_>, &mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError> {
        Err(self.not_a_node())
    }

    /// `BaseRestParser._handle_list`: a header's text split at each `,`.
    fn read_list(
        &mut self,
        _schema: Schema<'_>,
        each: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError> {
        let Value::String(text) = self.0 else {
            return Err(self.not_a_node());
        };
        for part in text.split(',') {
            let part = Value::String(pytext::text::strip(part).to_owned());
            each(&mut HeaderValue(&part))?;
        }
        Ok(Read::Present)
    }

    /// `_handle_map` over text iterates its characters, which have no
    /// `tag`; an empty text is an empty dict.
    fn read_map(
        &mut self,
        _schema: Schema<'_>,
        _each: &mut dyn FnMut(Key, &mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError> {
        match self.0 {
            Value::String(text) if text.is_empty() => Ok(Read::Present),
            _ => Err(self.not_a_node()),
        }
    }

    fn read_scalar(&mut self, schema: Schema<'_>) -> Result<Value, SerdeError> {
        let value = self.0;
        match (schema.shape_type(), value) {
            (ShapeType::Boolean, _) => Ok(Value::Bool(
                matches!(value, Value::String(text) if text == "true"),
            )),
            (ShapeType::Integer | ShapeType::Long, Value::String(text)) => {
                int_from_text(text).map_err(SerdeError::new)
            }
            (ShapeType::Float | ShapeType::Double, Value::String(text)) => {
                float_from_text(text).map(Value::Float).ok_or_else(|| {
                    SerdeError::new(format!(
                        "could not convert string to float: {}",
                        py_repr(value)
                    ))
                })
            }
            (ShapeType::Float | ShapeType::Double, Value::Integer(number)) => {
                Ok(Value::Float(*number as f64))
            }
            (ShapeType::String, _) => {
                json_value(schema, value).unwrap_or_else(|| Ok(value.clone()))
            }
            _ => Ok(value.clone()),
        }
    }

    fn read_document(&mut self, _schema: Schema<'_>) -> Result<Value, SerdeError> {
        Ok(self.0.clone())
    }

    fn is_null(&self) -> bool {
        false
    }

    fn union_tags(&self) -> Option<Vec<Key>> {
        None
    }
}

/// `RestXMLParser._do_error_parse`: the body's envelope when the body is
/// XML, else the status.
///
/// # Errors
/// A body whose envelope `merge_dicts` cannot merge.
pub(super) fn error_metadata(response: &HttpResponse) -> Result<Map, ParseError> {
    if !response.body.is_empty()
        && let Ok(root) = codec::parse(&response.body)
    {
        return error_from_body(&root, response);
    }
    Ok(error_from_status(response))
}

/// `_parse_error_from_body`: S3's `<Error>` root as the `Error` with the
/// headers' request ids; otherwise the root's children merged over an empty
/// `Code` and `Message`, a `RequestId` child moved into
/// `ResponseMetadata`. The root's tag is compared as written, namespace
/// and all.
fn error_from_body(root: &Element, response: &HttpResponse) -> Result<Map, ParseError> {
    let mut parsed = replaced(by_name(root));
    if root.tag == "Error" {
        let metadata = rest::response_metadata(&response.headers);
        parsed.remove("RequestId");
        parsed.remove("HostId");
        let mut error = Map::new();
        error.insert("Error".into(), Value::Map(parsed));
        error.insert("ResponseMetadata".into(), Value::Map(metadata));
        return Ok(error);
    }
    if let Some(id) = parsed.remove("RequestId") {
        let mut metadata = Map::new();
        metadata.insert("RequestId".into(), id);
        parsed.insert("ResponseMetadata".into(), Value::Map(metadata));
    }
    let mut error = Map::new();
    error.insert("Message".into(), Value::String(String::new()));
    error.insert("Code".into(), Value::String(String::new()));
    let mut merged = Map::new();
    merged.insert("Error".into(), Value::Map(error));
    merge_dicts(&mut merged, parsed)?;
    Ok(merged)
}

/// `botocore.utils.merge_dicts`: a dict merged into the dict under the same
/// key, anything else set over it. Merging into what is no dict fails as
/// item assignment on it does, unless there is nothing to merge.
fn merge_dicts(into: &mut Map, from: Map) -> Result<(), ParseError> {
    for (key, value) in from {
        match (into.get_mut_key(&key), value) {
            (Some(Value::Map(existing)), Value::Map(inner)) => merge_dicts(existing, inner)?,
            (Some(other), Value::Map(inner)) => {
                if !inner.is_empty() {
                    return Err(ParseError::new(format!(
                        "'{}' object does not support item assignment",
                        py_short_type_name(other)
                    )));
                }
            }
            (_, value) => {
                into.insert_key(key, value);
            }
        }
    }
    Ok(())
}

/// `_parse_error_from_http_status`: the status and its reason phrase, the
/// request ids from the headers.
fn error_from_status(response: &HttpResponse) -> Map {
    let header = |name| Value::String(response.headers.joined(name).unwrap_or_default());
    let mut error = Map::new();
    error.insert("Code".into(), Value::String(response.status.to_string()));
    error.insert(
        "Message".into(),
        Value::String(reason_phrase(response.status).into()),
    );
    let mut metadata = Map::new();
    metadata.insert("RequestId".into(), header("x-amz-request-id"));
    metadata.insert("HostId".into(), header("x-amz-id-2"));
    let mut parsed = Map::new();
    parsed.insert("Error".into(), Value::Map(error));
    parsed.insert("ResponseMetadata".into(), Value::Map(metadata));
    parsed
}

/// `http.client.responses.get(status, '')`, CPython 3.14's.
fn reason_phrase(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        101 => "Switching Protocols",
        102 => "Processing",
        103 => "Early Hints",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        203 => "Non-Authoritative Information",
        204 => "No Content",
        205 => "Reset Content",
        206 => "Partial Content",
        207 => "Multi-Status",
        208 => "Already Reported",
        226 => "IM Used",
        300 => "Multiple Choices",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        305 => "Use Proxy",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        407 => "Proxy Authentication Required",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Content Too Large",
        414 => "URI Too Long",
        415 => "Unsupported Media Type",
        416 => "Range Not Satisfiable",
        417 => "Expectation Failed",
        418 => "I'm a Teapot",
        421 => "Misdirected Request",
        422 => "Unprocessable Content",
        423 => "Locked",
        424 => "Failed Dependency",
        425 => "Too Early",
        426 => "Upgrade Required",
        428 => "Precondition Required",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        451 => "Unavailable For Legal Reasons",
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

#[cfg(test)]
mod tests;
