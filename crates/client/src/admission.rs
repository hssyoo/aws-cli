//! Why the model client leaves a call to the Python route: each is decided
//! before any request ([`crate::call::Client::prepare`]), and the caller
//! routes the command there instead (`PythonReason::UnsupportedTrait`).
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
    /// A protocol not implemented, as botocore resolves it for the model.
    Protocol(String),
    /// An operation whose endpoint has a host prefix (`endpoint.hostPrefix`),
    /// which the serializer does not yet fill.
    HostPrefix,
}

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OperationContextParams => f.write_str("operation context parameters"),
            Self::Builtin(name) => write!(f, "the endpoint built-in {name}"),
            Self::Auth(name) => write!(f, "the auth type {name}"),
            Self::Validation(undecided) => write!(f, "a validation of {undecided:?}"),
            Self::Protocol(name) => write!(f, "the {name} protocol"),
            Self::HostPrefix => f.write_str("an endpoint host prefix"),
        }
    }
}
