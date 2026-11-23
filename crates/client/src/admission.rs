//! Why the model client leaves a call to the Python route. Each refusal
//! is decided before any request, and before any credential provider runs:
//! [`static_refusal`] from the model and the rule set alone, and
//! [`crate::call::Client::prepare`] from the configuration and the
//! endpoint resolution, which needs no credentials. The caller routes the
//! command to Python with the reason each variant names.
use crate::endpoint::EndpointData;
use crate::hooks::Customization;
use model::shapes::{Flag, Model, Operation, ShapeId};
use model::validate::Undecided;

/// Why a call is left to the Python route; each variant is one reason the
/// caller can name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unsupported {
    /// A protocol not implemented, as botocore resolves it for the model.
    Protocol(String),
    /// An operation trait the client does not implement, by its model key:
    /// a request checksum, required endpoint discovery, the discovery
    /// operation, request compression, an event stream or a streaming
    /// member, or a key the model loader does not interpret.
    Trait(String),
    /// `operationContextParams`, which botocore evaluates with JMESPath.
    OperationContextParams,
    /// An endpoint built-in neither core nor a customization supplies,
    /// which the rule set requires without a default.
    Builtin(String),
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
}

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Protocol(name) => write!(f, "the {name} protocol"),
            Self::Trait(name) => write!(f, "the {name} trait"),
            Self::OperationContextParams => f.write_str("operation context parameters"),
            Self::Builtin(name) => write!(f, "the endpoint built-in {name}"),
            Self::Auth(name) => write!(f, "the auth type {name}"),
            Self::UnsignedPayload => f.write_str("an unsigned payload"),
            Self::EndpointHeaders => f.write_str("endpoint headers"),
            Self::RegionPattern => f.write_str("a partition region pattern"),
            Self::EndpointData => f.write_str("an unreadable endpoints.json"),
            Self::Validation(undecided) => write!(f, "a validation of {undecided:?}"),
            Self::Hook(reason) => write!(f, "a customization: {reason}"),
        }
    }
}

/// Every refusal the model and the rule set decide alone, whatever the
/// configuration, the input and the credentials, in the order a caller
/// meets them: the protocol, the operation's traits, its context
/// parameters, its auth, then the rule set's required built-ins. A call
/// this passes can still be refused by [`crate::call::Client::prepare`],
/// which needs no credentials either.
pub fn static_refusal(
    model: &Model,
    operation: &Operation,
    endpoints: &EndpointData,
    service: &str,
    customizations: &[&'static Customization],
) -> Option<Unsupported> {
    if crate::protocol::for_model(model).is_none() {
        let name = crate::protocol::resolved_protocol(model.metadata()).unwrap_or_default();
        return Some(Unsupported::Protocol(name.to_owned()));
    }
    if let Some(unsupported) = unimplemented_trait(operation).or_else(|| streams(model, operation))
    {
        return Some(unsupported);
    }
    if !operation.operation_context_params.is_empty() {
        return Some(Unsupported::OperationContextParams);
    }
    if let Some(unsupported) = crate::auth::static_refusal(model, operation) {
        return Some(unsupported);
    }
    crate::endpoint::missing_builtin(endpoints, service, customizations).map(Unsupported::Builtin)
}

/// An event stream or a streaming member at either end of the operation,
/// which botocore reads from the top-level members (`OperationModel.
/// has_event_stream_input`, `has_streaming_output` and their kin).
fn streams(model: &Model, operation: &Operation) -> Option<Unsupported> {
    let reaches = |shape: Option<ShapeId>, flag: Flag| {
        shape.is_some_and(|shape| {
            model.shape(shape).traits.is(flag)
                || model
                    .members(shape)
                    .iter()
                    .any(|member| model.shape(member.shape).traits.is(flag))
        })
    };
    let ends = [operation.input, operation.output];
    if ends.iter().any(|end| reaches(*end, Flag::EventStream)) {
        return Some(Unsupported::Trait("eventstream".to_owned()));
    }
    if ends.iter().any(|end| reaches(*end, Flag::Streaming)) {
        return Some(Unsupported::Trait("streaming".to_owned()));
    }
    None
}

/// The first operation trait the client does not implement: request
/// checksums (`httpChecksum`, `httpChecksumRequired`), required endpoint
/// discovery, the discovery operation itself (which botocore sends with
/// `x-amz-api-version`, `inject_api_version_header_if_needed`), and request
/// compression. Optional discovery is no refusal: botocore's default
/// `auto` mode discovers only for required operations
/// (`_requires_endpoint_discovery`, `EndpointDiscoveryManager.
/// describe_endpoint`).
pub fn unimplemented_trait(operation: &Operation) -> Option<Unsupported> {
    let found = if operation.http_checksum.is_some() {
        "httpChecksum"
    } else if operation.http_checksum_required {
        "httpChecksumRequired"
    } else if operation
        .endpoint_discovery
        .is_some_and(|discovery| discovery.required)
    {
        "endpointdiscovery"
    } else if operation.endpoint_operation {
        "endpointoperation"
    } else if operation.request_compression.is_some() {
        "requestcompression"
    } else {
        return None;
    };
    Some(Unsupported::Trait(found.to_owned()))
}

#[cfg(test)]
mod tests;
