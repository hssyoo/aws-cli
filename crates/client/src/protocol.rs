//! A wire protocol: how an operation's input becomes a request and a
//! response becomes its output or a service error. [`ClientProtocol`] is
//! the interface, [`compat`] botocore's drivers over it, and [`registry`]
//! botocore's choice of one. The implementations: [`query`] (awsQuery),
//! [`json`] (awsJson 1.0 and 1.1, with the query-compatible mode) and
//! [`rest_json`] (restJson1).
use crate::admission::Unsupported;
use crate::error::{CallError, ParseError, SerializeError, ServiceError};
use crate::http::{Headers, HttpRequest, HttpResponse, Method, Url};
use model::schema::{OperationSchema, Schema, TraitId, TypeRegistry};
use model::serde::Codec;
use model::shapes::Model;
use model::value::{Map, Value};

/// A request before it has an endpoint: botocore's request dict, as a
/// serializer returns it (`Serializer.serialize_to_request`). The call
/// joins it to the resolved endpoint (`prepare_request_dict`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Serialized {
    pub method: Method,
    /// `url_path`: appended to the endpoint's path; percent-encoded.
    pub path: String,
    /// `query_string`, encoded, without the `?`.
    pub query: String,
    pub headers: Headers,
    pub body: Vec<u8>,
    /// `host_prefix`: the operation's `endpoint.hostPrefix` with its host
    /// labels filled in (`Serializer._expand_host_prefix`), for the call to
    /// put before the endpoint's host.
    pub host_prefix: Option<String>,
}

/// botocore's `timestamp_parser`: a timestamp member's wire value (XML text
/// or a JSON number or string) to the parsed value.
pub type TimestampHandler<'a> = &'a (dyn Fn(&Value) -> Result<Value, ParseError> + Sync);
/// botocore's `blob_parser`: a blob member's `str` (a [`Value::String`], or
/// a [`Value::PyString`] from a JSON body) to the parsed value.
pub type BlobHandler<'a> = &'a (dyn Fn(&Value) -> Result<Value, ParseError> + Sync);

/// The parser defaults `ResponseParserFactory.set_parser_defaults` sets.
#[derive(Clone, Copy)]
pub struct ParseOptions<'a> {
    pub timestamp: TimestampHandler<'a>,
    pub blob: BlobHandler<'a>,
}

impl std::fmt::Debug for ParseOptions<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParseOptions").finish_non_exhaustive()
    }
}

/// The response parsers' handlers, owned by the client that uses them, so
/// each client can carry its own (the CLI's depend on the invocation's
/// `cli_timestamp_format`). Each call borrows them as [`ParseOptions`].
#[derive(Clone)]
pub struct ParseHooks {
    pub timestamp: OwnedTimestampHandler,
    pub blob: OwnedBlobHandler,
}

/// A [`TimestampHandler`] a client owns.
pub type OwnedTimestampHandler =
    std::sync::Arc<dyn Fn(&Value) -> Result<Value, ParseError> + Send + Sync>;
/// A [`BlobHandler`] a client owns.
pub type OwnedBlobHandler =
    std::sync::Arc<dyn Fn(&Value) -> Result<Value, ParseError> + Send + Sync>;

impl ParseHooks {
    /// botocore's own handlers ([`ParseOptions::BOTOCORE`], the one place
    /// they are named), for tests: the CLI's clients carry its own.
    #[cfg(any(test, feature = "testing"))]
    pub fn botocore() -> Self {
        Self {
            timestamp: std::sync::Arc::new(ParseOptions::BOTOCORE.timestamp),
            blob: std::sync::Arc::new(ParseOptions::BOTOCORE.blob),
        }
    }
    /// The handlers as one call's parse borrows them.
    pub fn options(&self) -> ParseOptions<'_> {
        ParseOptions {
            timestamp: &*self.timestamp,
            blob: &*self.blob,
        }
    }
}

impl std::fmt::Debug for ParseHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParseHooks").finish_non_exhaustive()
    }
}

impl ParseOptions<'static> {
    /// botocore's own: `parse_timestamp` to a [`Value::Timestamp`] and
    /// `base64.b64decode` to a [`Value::Blob`]. The CLI replaces both
    /// (`add_timestamp_parser`, its identity blob parser).
    pub const BOTOCORE: Self = Self {
        timestamp: &common::parse_timestamp,
        blob: &common::decode_blob,
    };
}

/// A wire protocol (smithy-rs's `ClientProtocol`): the request for an
/// input, and botocore's three parse steps (`_do_parse`, `_do_error_parse`,
/// `_do_modeled_error_parse`), which [`compat`]'s drivers combine as
/// `ResponseParser.parse` does. A protocol implements only the steps; the
/// botocore name it answers to lives in the [`registry`] alone.
pub trait ClientProtocol: Sync + std::fmt::Debug {
    /// The Smithy protocol trait the protocol implements.
    fn protocol_id(&self) -> TraitId;
    /// The codec of its bodies, both ways; `None` when the two directions
    /// differ (awsQuery writes a form and reads XML).
    fn payload_codec(&self) -> Option<&dyn Codec>;
    /// Where it departs from Smithy as botocore does.
    fn compat(&self) -> &BotocoreCompat;
    /// The request for `operation` with `input`, which validation passed
    /// (`Serializer.serialize_to_request`).
    ///
    /// # Errors
    /// An input the protocol cannot put on the wire.
    fn serialize_request(
        &self,
        operation: OperationSchema<'_>,
        input: &Value,
        model: &Model,
    ) -> Result<Serialized, SerializeError>;
    /// The serialized request joined to the endpoint with the call's
    /// `User-Agent` (`prepare_request_dict`, the SRA's
    /// `updateServiceEndpoint`).
    ///
    /// # Errors
    /// An endpoint that is not a URL.
    fn update_endpoint(
        &self,
        request: &Serialized,
        endpoint_url: &str,
        user_agent: &str,
    ) -> Result<HttpRequest, CallError> {
        join(request.clone(), endpoint_url, user_agent)
    }
    /// `_do_parse`: the output of a response the drivers did not take for
    /// an error.
    ///
    /// # Errors
    /// `ResponseParserError`: a body the protocol cannot read.
    fn deserialize_response(
        &self,
        operation: OperationSchema<'_>,
        shape: Option<Schema<'_>>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Value, ParseError>;
    /// `_do_error_parse`: the `Error` and `ResponseMetadata` botocore
    /// reads from an error response.
    ///
    /// # Errors
    /// `ResponseParserError`: a body the protocol cannot read.
    fn parse_error_metadata(&self, response: &HttpResponse) -> Result<Map, ParseError>;
    /// `_do_modeled_error_parse`: a modelled error shape's members.
    ///
    /// # Errors
    /// `ResponseParserError`: a body the protocol cannot read.
    fn deserialize_modeled_error(
        &self,
        shape: Schema<'_>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Map, ParseError>;
    /// A streamed output, read from the response as it arrives. Reserved:
    /// no protocol streams yet.
    fn deserialize_streaming(
        &self,
        _operation: OperationSchema<'_>,
        _response: &mut HttpResponse,
    ) -> Option<Result<Value, ParseError>> {
        None
    }
    /// The output of a successful response: [`compat::parse`]. Protocols
    /// keep this; a test double may answer for the whole driver.
    ///
    /// # Errors
    /// `ResponseParserError`: a body the protocol cannot read.
    fn parse(
        &self,
        operation: OperationSchema<'_>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Value, ParseError> {
        compat::parse(self, operation, operation.output(), response, options)
    }
    /// The service error a response with a status of 300 or more carries:
    /// [`compat::parse_error`], with `registry` naming the modelled errors.
    /// Protocols keep this; a test double may answer for the whole driver.
    ///
    /// # Errors
    /// `ResponseParserError`: a body the protocol cannot read.
    fn parse_error(
        &self,
        operation: OperationSchema<'_>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
        registry: TypeRegistry<'_>,
    ) -> Result<ServiceError, ParseError> {
        compat::parse_error(self, operation, response, options, registry)
    }
}

mod common;
pub mod compat;
pub mod json;
pub mod query;
pub mod registry;
pub mod rest_json;

pub use compat::BotocoreCompat;
pub use json::{AWS_JSON_1_0, AWS_JSON_1_1, HttpRpcProtocol};
pub use query::{AWS_QUERY, AwsQueryProtocol};
pub use registry::{ProtocolRegistry, Unresolved, resolved_protocol};
pub use rest_json::{HttpBindingProtocol, REST_JSON_1};

/// What the codecs share with the protocols: botocore's serializer
/// conversions, wire names and body decoding.
pub(crate) use common::{
    base64_in, beyond_limit, decoded, map_entry, special_float, timestamp_out, wire_name,
};

/// botocore's priority order (`PRIORITY_ORDERED_SUPPORTED_PROTOCOLS`,
/// botocore/utils.py), which [`registry::resolved_protocol`] reads.
pub(crate) const PRIORITY: [&str; 6] = [
    "json",
    "rest-json",
    "rest-xml",
    "smithy-rpc-v2-cbor",
    "query",
    "ec2",
];

/// The protocols implemented, under the botocore names that select them,
/// which admission asks about without building a client.
pub static IMPLEMENTED: ProtocolRegistry = ProtocolRegistry::new(&[
    ("query", &AWS_QUERY),
    ("json", &AWS_JSON_1_1),
    ("json", &AWS_JSON_1_0),
    ("rest-json", &REST_JSON_1),
]);

/// Whether `model`'s service sends a timestamp member with no
/// `timestampFormat` as an epoch number; `None` when its protocol is not
/// implemented.
pub fn epoch_timestamps(model: &Model) -> Option<bool> {
    for_model(model).map(compat::timestamps_epoch)
}

/// The implementation of the protocol botocore speaks to `model`'s service;
/// `None` when that protocol is not implemented, even if another the model
/// lists is, or when the model has none.
pub fn for_model(model: &Model) -> Option<&'static dyn ClientProtocol> {
    select(model).ok()
}

/// [`for_model`], refused as [`Unsupported::Protocol`] with the name
/// botocore resolves (empty when it resolves none).
///
/// # Errors
/// See above.
pub fn select(model: &Model) -> Result<&'static dyn ClientProtocol, Unsupported> {
    IMPLEMENTED.resolve(model.metadata())
}

/// `prepare_request_dict`: the serialized request joined to the endpoint
/// (`_urljoin`), its host prefix before the endpoint's host, its query
/// after the endpoint's, and `User-Agent` set after the serializer's
/// headers. Host prefix injection is always on: the router sends
/// `disable_host_prefix_injection` to Python.
fn join(
    serialized: Serialized,
    endpoint: &str,
    user_agent: &str,
) -> Result<HttpRequest, CallError> {
    let mut url = Url::parse(endpoint)
        .ok_or_else(|| CallError::Endpoint(format!("Invalid endpoint: {endpoint}")))?;
    if let Some(prefix) = &serialized.host_prefix {
        url.authority.insert_str(0, prefix);
    }
    let path = &serialized.path;
    if !(path.is_empty() || path == "/") {
        match (url.path.strip_suffix('/'), path.starts_with('/')) {
            (Some(base), true) => url.path = format!("{base}{path}"),
            _ => url.path.push_str(path),
        }
    }
    if !serialized.query.is_empty() {
        if !url.query.is_empty() {
            url.query.push('&');
        }
        url.query.push_str(&serialized.query);
    }
    let mut headers = serialized.headers;
    headers.set("User-Agent", user_agent);
    Ok(HttpRequest {
        method: serialized.method,
        url,
        headers,
        body: serialized.body,
    })
}
