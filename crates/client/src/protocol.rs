//! A wire protocol: how an operation's input becomes a request and a
//! response becomes its output or a service error. One implementation per
//! protocol family: [`query`], [`json`] (1.0 and 1.1, with the
//! query-compatible mode) and [`rest_json`].
use crate::error::{ParseError, SerializeError, ServiceError};
use crate::http::{Headers, HttpResponse, Method};
use model::schema::OperationSchema;
use model::shapes::{Metadata, Model};
use model::value::Value;

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

pub trait Protocol: Sync + std::fmt::Debug {
    /// The protocol's name, as a model's `protocol` and `protocols` spell
    /// it.
    fn name(&self) -> &'static str;
    /// Whether a timestamp member with no `timestampFormat` goes over the
    /// wire as an epoch number (`JSONSerializer.TIMESTAMP_FORMAT`,
    /// `unixtimestamp`) rather than ISO 8601 text (`QuerySerializer`'s and
    /// the XML protocols' `iso8601`).
    fn epoch_timestamps(&self) -> bool;
    /// The request for `operation` with `input`, which validation passed.
    ///
    /// # Errors
    /// An input the protocol cannot put on the wire.
    fn serialize(
        &self,
        model: &Model,
        operation: OperationSchema<'_>,
        input: &Value,
    ) -> Result<Serialized, SerializeError>;
    /// The output of a successful response.
    ///
    /// # Errors
    /// `ResponseParserError`: a body the protocol cannot read.
    fn parse(
        &self,
        model: &Model,
        operation: OperationSchema<'_>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Value, ParseError>;
    /// The service error a response with a status of 300 or more carries,
    /// as botocore parses it (`ResponseParser.parse`, whose error path
    /// starts at 301), its modelled members included
    /// (`Endpoint._add_modeled_error_fields`).
    ///
    /// # Errors
    /// `ResponseParserError`: a body the protocol cannot read.
    fn parse_error(
        &self,
        model: &Model,
        operation: OperationSchema<'_>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<ServiceError, ParseError>;
}

mod common;
pub mod json;
/// The serializers' shared conversions, for the codecs.
pub(crate) use common::{base64_in, special_float, timestamp_out};
pub mod query;
pub mod rest_json;

/// The protocols implemented, which admission asks about without building
/// a client.
static IMPLEMENTED: &[&'static dyn Protocol] = &[&query::Query, &json::Json, &rest_json::RestJson];

/// Why a model has no protocol botocore would speak.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unresolved {
    /// `UnsupportedServiceProtocolsError`: `protocols` lists none botocore
    /// supports.
    Unsupported(Vec<String>),
    /// `UndefinedModelAttributeError`: neither `protocols` nor `protocol`.
    Missing,
}

/// botocore's priority order (`PRIORITY_ORDERED_SUPPORTED_PROTOCOLS`,
/// botocore/utils.py).
const PRIORITY: [&str; 6] = [
    "json",
    "rest-json",
    "rest-xml",
    "smithy-rpc-v2-cbor",
    "query",
    "ec2",
];

/// The protocol botocore speaks to a service (`ServiceModel.
/// resolved_protocol`): with a nonempty `protocols`, the first of
/// [`PRIORITY`] it lists; otherwise `protocol`.
///
/// # Errors
/// See [`Unresolved`].
pub fn resolved_protocol(metadata: &Metadata) -> Result<&str, Unresolved> {
    match metadata.protocols.as_deref() {
        Some(listed) if !listed.is_empty() => PRIORITY
            .into_iter()
            .find(|protocol| listed.iter().any(|name| &**name == *protocol))
            .ok_or_else(|| Unresolved::Unsupported(listed.iter().map(|n| n.to_string()).collect())),
        _ => metadata.protocol.as_deref().ok_or(Unresolved::Missing),
    }
}

/// Whether `model`'s service sends a timestamp member with no
/// `timestampFormat` as an epoch number; `None` when its protocol is not
/// implemented.
pub fn epoch_timestamps(model: &Model) -> Option<bool> {
    for_model(model).map(Protocol::epoch_timestamps)
}

/// The implementation of the protocol botocore speaks to `model`'s service;
/// `None` when that protocol is not implemented, even if another the model
/// lists is, or when the model has none.
pub fn for_model(model: &Model) -> Option<&'static dyn Protocol> {
    select(model.metadata(), IMPLEMENTED)
}

fn select(
    metadata: &Metadata,
    implemented: &[&'static dyn Protocol],
) -> Option<&'static dyn Protocol> {
    let name = resolved_protocol(metadata).ok()?;
    implemented
        .iter()
        .copied()
        .find(|protocol| protocol.name() == name)
}

#[cfg(test)]
mod tests;
