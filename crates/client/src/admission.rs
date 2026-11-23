//! Why the model client leaves a call to the Python route: each is decided
//! before any request ([`crate::call::Client::prepare`]), and the caller
//! routes the command there instead (`PythonReason::UnsupportedTrait`).
use model::shapes::Operation;
use model::validate::Undecided;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unsupported {
    /// `operationContextParams`, which botocore evaluates with JMESPath.
    OperationContextParams,
    /// An endpoint built-in core does not supply, which the rule set
    /// requires without a default and no interceptor set.
    Builtin(String),
    /// An auth type the client cannot sign with (anything but SigV4 and
    /// none), or a payload left unsigned.
    Auth(String),
    /// A validation only the Python route can decide.
    Validation(Undecided),
    /// An operation trait the client does not implement.
    Trait(&'static str),
    /// A partition `regionRegex` the native variant check cannot read.
    RegionPattern,
    /// A protocol not implemented, as botocore resolves it for the model.
    Protocol(String),
}

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OperationContextParams => f.write_str("operation context parameters"),
            Self::Builtin(name) => write!(f, "the endpoint built-in {name}"),
            Self::Auth(name) => write!(f, "the auth type {name}"),
            Self::Validation(undecided) => write!(f, "a validation of {undecided:?}"),
            Self::Trait(name) => write!(f, "the {name} trait"),
            Self::RegionPattern => f.write_str("a partition region pattern"),
            Self::Protocol(name) => write!(f, "the {name} protocol"),
        }
    }
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
    Some(Unsupported::Trait(found))
}

#[cfg(test)]
mod tests;
