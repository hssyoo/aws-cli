//! Why the model client leaves a call to the Python route. Each refusal
//! is decided before any request, and before any credential provider runs:
//! [`static_refusal`] from the model and the rule set alone, and
//! [`crate::call::Client::prepare`] from the configuration and the
//! endpoint resolution, which needs no credentials. The caller routes the
//! command to Python with the reason each variant names.
use crate::endpoint::EndpointData;
use crate::hooks::Customization;
use model::paginators::{Config, Refused};
use model::schema::{Flag, OperationSchema, Schema, ShapeType, traits};
use model::shapes::Model;
use model::validate::Undecided;

/// Why a call is left to the Python route; each variant is one reason the
/// caller can name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unsupported {
    /// A protocol not implemented, as botocore resolves it for the model.
    Protocol(String),
    /// An operation trait the client does not implement, by its model key:
    /// a request checksum, required endpoint discovery, the discovery
    /// operation, request compression, an event stream, a streaming member
    /// the client does not carry, or a key the model loader does not
    /// interpret.
    Trait(String),
    /// `operationContextParams`, which botocore evaluates with JMESPath.
    OperationContextParams,
    /// An endpoint built-in neither core nor a customization supplies,
    /// which the rule set requires without a default.
    Builtin(String),
    /// A client-context parameter the rule set requires without a default,
    /// which the CLI's empty client context cannot supply and the
    /// operation does not set.
    ClientContextParam(String),
    /// An auth type the client cannot sign with (anything but SigV4 and
    /// none), by the name botocore resolves.
    Auth(String),
    /// An operation whose payload SigV4 leaves unsigned (`unsignedPayload`).
    UnsignedPayload,
    /// An endpoint resolution that carries headers to send.
    EndpointHeaders,
    /// A partition `regionRegex` the native variant check cannot read.
    RegionPattern,
    /// An `endpoints.json` that is not the JSON its kind is, found when a
    /// call first needs it.
    EndpointData,
    /// A validation only the Python route can decide.
    Validation(Undecided),
    /// A customization's interceptor left the call to Python, for the
    /// reason it names.
    Hook(String),
    /// A starting token whose JSON Python reads into what the native
    /// values cannot hold, or follows deeper than they do.
    StartingToken(&'static str),
    /// The operation's paginator entry, one the native paginator does not
    /// run.
    Paginator(Refused),
}

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Protocol(name) => write!(f, "the {name} protocol"),
            Self::Trait(name) => write!(f, "the {name} trait"),
            Self::OperationContextParams => f.write_str("operation context parameters"),
            Self::Builtin(name) => write!(f, "the endpoint built-in {name}"),
            Self::ClientContextParam(name) => write!(f, "the client context parameter {name}"),
            Self::Auth(name) => write!(f, "the auth type {name}"),
            Self::UnsignedPayload => f.write_str("an unsigned payload"),
            Self::EndpointHeaders => f.write_str("endpoint headers"),
            Self::RegionPattern => f.write_str("a partition region pattern"),
            Self::EndpointData => f.write_str("an unreadable endpoints.json"),
            Self::Validation(undecided) => write!(f, "a validation of {undecided:?}"),
            Self::Hook(reason) => write!(f, "a customization: {reason}"),
            Self::StartingToken(why) => write!(f, "a starting token with {why}"),
            Self::Paginator(refused) => refused.fmt(f),
        }
    }
}

/// Every refusal the model, the rule set and the paginators file decide
/// alone, whatever the configuration, the input and the credentials, in the
/// order a caller meets them: the protocol, the operation's traits, its
/// context parameters, its auth, the rule set's required built-ins and
/// client context, then `paginator`, the operation's entry in
/// `paginators-1.json` as [`model::paginators::Paginators`] parsed it, if it
/// has one (whether or not this call pages, so
/// `--no-paginate` changes nothing here). A call this passes can still be
/// refused by [`crate::call::Client::prepare`] and by the paginator's
/// reading of a starting token, neither of which needs credentials.
pub fn static_refusal(
    model: &Model,
    operation: OperationSchema<'_>,
    endpoints: &EndpointData,
    service: &str,
    customizations: &[&'static Customization],
    paginator: Option<&Result<Config, Refused>>,
) -> Option<Unsupported> {
    refusal(
        model,
        operation,
        endpoints,
        service,
        customizations,
        paginator,
        false,
    )
}

/// [`static_refusal`], which with `checksums` admits the request checksum
/// traits ([`crate::checksum`], built but switched off: only a client a
/// test switches on passes `true`).
pub(crate) fn refusal(
    model: &Model,
    operation: OperationSchema<'_>,
    endpoints: &EndpointData,
    service: &str,
    customizations: &[&'static Customization],
    paginator: Option<&Result<Config, Refused>>,
    checksums: bool,
) -> Option<Unsupported> {
    if let Err(unsupported) = crate::protocol::select(model) {
        return Some(unsupported);
    }
    // A key the model loader does not interpret may change what botocore
    // does with the call; the first one the operation reaches refuses it.
    if let Some(uninterpreted) = model.schema().uninterpreted_reached(operation).first() {
        return Some(Unsupported::Trait(uninterpreted.key.to_string()));
    }
    let traits = if checksums {
        unimplemented_trait_but_checksums(operation)
    } else {
        unimplemented_trait(operation)
    };
    if let Some(unsupported) = traits.or_else(|| streams(model, operation)) {
        return Some(unsupported);
    }
    if operation.operation_context_params().next().is_some() {
        return Some(Unsupported::OperationContextParams);
    }
    if let Some(unsupported) = crate::auth::static_refusal(model, operation) {
        return Some(unsupported);
    }
    if let Some(name) = crate::endpoint::missing_builtin(endpoints, service, customizations) {
        return Some(Unsupported::Builtin(name));
    }
    if let Some(name) = crate::endpoint::missing_client_context(endpoints, model, operation) {
        return Some(Unsupported::ClientContextParam(name));
    }
    paginator?
        .as_ref()
        .err()
        .cloned()
        .map(Unsupported::Paginator)
}

/// An event stream at either end of the operation, which botocore reads
/// from the top-level members (`OperationModel.has_event_stream_input`,
/// `has_event_stream_output`); and a streaming end the client does not
/// carry: a `streaming` member that is not its end's blob payload, or a
/// blob payload under a protocol without payloads.
///
/// A blob payload is what botocore streams, marked `streaming` or not
/// (`_get_streaming_body`, `has_streaming_input`, `has_streaming_output`):
/// the client sends the input's bytes as the body, with the
/// `Content-Length` and payload hash botocore computes from a file it can
/// seek, and returns the output's raw bytes under the payload member
/// ([`crate::protocol::rest_json`]'s parse), which the CLI writes to its
/// `outfile` (`streamingoutputarg`). Only the REST protocols bind a payload.
fn streams(model: &Model, operation: OperationSchema<'_>) -> Option<Unsupported> {
    let reaches = |end: Option<Schema<'_>>, flag: Flag| {
        end.is_some_and(|end| end.flag(flag) || end.members().any(|member| member.flag(flag)))
    };
    let ends = [operation.input(), operation.output()];
    if ends.iter().any(|end| reaches(*end, Flag::EventStream)) {
        return Some(Unsupported::Trait("eventstream".to_owned()));
    }
    let rest = crate::protocol::select(model).is_ok_and(|protocol| {
        let id = protocol.protocol_id();
        id == traits::REST_JSON_1 || id == traits::REST_XML
    });
    let carried = |end: &Option<Schema<'_>>| {
        end.is_none_or(|end| {
            let payload = blob_payload(end);
            let streamed = |member: Schema<'_>| {
                !member.flag(Flag::Streaming) || payload.is_some_and(|blob| blob == member)
            };
            !end.flag(Flag::Streaming) && end.members().all(streamed) && (payload.is_none() || rest)
        })
    };
    if !ends.iter().all(carried) {
        return Some(Unsupported::Trait("streaming".to_owned()));
    }
    None
}

/// `OperationModel._get_streaming_body`: the `payload` member, when it is a
/// blob, which streams whether or not it is marked `streaming`.
fn blob_payload(end: Schema<'_>) -> Option<Schema<'_>> {
    end.payload()
        .and_then(|payload| end.member(payload))
        .filter(|member| member.shape_type() == ShapeType::Blob)
}

/// The first operation trait the client does not implement: request
/// checksums (`httpChecksum`, `httpChecksumRequired`), required endpoint
/// discovery, the discovery operation itself (which botocore sends with
/// `x-amz-api-version`, `inject_api_version_header_if_needed`), and request
/// compression. Optional discovery is no refusal: botocore's default
/// `auto` mode discovers only for required operations
/// (`_requires_endpoint_discovery`, `EndpointDiscoveryManager.
/// describe_endpoint`).
pub(crate) fn unimplemented_trait(operation: OperationSchema<'_>) -> Option<Unsupported> {
    let found = if operation.http_checksum().is_some() {
        "httpChecksum"
    } else if operation.http_checksum_required() {
        "httpChecksumRequired"
    } else {
        return unimplemented_trait_but_checksums(operation);
    };
    Some(Unsupported::Trait(found.to_owned()))
}

/// [`unimplemented_trait`] past the request checksum traits.
fn unimplemented_trait_but_checksums(operation: OperationSchema<'_>) -> Option<Unsupported> {
    let found = if operation
        .endpoint_discovery()
        .is_some_and(|discovery| discovery.required)
    {
        "endpointdiscovery"
    } else if operation.endpoint_operation() {
        "endpointoperation"
    } else if operation.request_compression().is_some() {
        "requestcompression"
    } else {
        return None;
    };
    Some(Unsupported::Trait(found.to_owned()))
}

#[cfg(test)]
mod tests;
