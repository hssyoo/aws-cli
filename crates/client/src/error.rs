//! What a call fails with. Each error carries what its botocore exception's
//! text needs, and reads as that text; the CLI's framing and exit status
//! are the renderer's.
use crate::admission::Unsupported;
use crate::hooks::HookError;
use crate::http::Headers;
use crate::transport::TransportError;
use model::validate::Report;
use model::value::{Map, Value};

/// An error the service returned: what botocore's parsed response holds
/// for one (`ClientError.response`: `Error`, `ResponseMetadata`, and the
/// modelled error's members), each key where botocore puts it and in its
/// order, as the CLI's error formatters read them.
#[derive(Clone, Debug, PartialEq)]
pub struct ServiceError {
    /// The operation called (`ClientError.operation_name`).
    pub operation: String,
    /// `parsed['Error']` in full: `Code` and `Message`, then whatever else
    /// the protocol's error parse put there (a query error's `Type`, a
    /// query-compatible error's `Type` and `QueryErrorCode`).
    pub error: Map,
    /// `ResponseMetadata.HTTPStatusCode`.
    pub status: u16,
    /// `ResponseMetadata.RequestId`.
    pub request_id: Option<String>,
    /// `ResponseMetadata.HTTPHeaders`.
    pub headers: Headers,
    /// The other top-level keys of the parsed response, `Error` and
    /// `ResponseMetadata` aside, in order: the modelled error's members,
    /// which botocore's second parse adds there, and what a query error's
    /// body holds besides `Error` and `RequestId`.
    pub members: Map,
    /// `ResponseMetadata.RetryAttempts`: attempts after the first.
    pub retry_attempts: u32,
    /// `ResponseMetadata.MaxAttemptsReached`.
    pub max_attempts_reached: bool,
}

impl ServiceError {
    /// An error of `operation` with `status` and nothing else known.
    pub fn new(operation: &str, status: u16) -> Self {
        Self {
            operation: operation.to_owned(),
            error: Map::new(),
            status,
            request_id: None,
            headers: Headers::new(),
            members: Map::new(),
            retry_attempts: 0,
            max_attempts_reached: false,
        }
    }
}

impl ServiceError {
    /// `Error.Code`, which botocore prints as `Unknown` when absent.
    pub fn code(&self) -> Option<&str> {
        self.text("Code")
    }

    /// `Error.Message`, likewise.
    pub fn message(&self) -> Option<&str> {
        self.text("Message")
    }

    fn text(&self, key: &str) -> Option<&str> {
        match self.error.get(key)? {
            Value::String(text) => Some(text),
            _ => None,
        }
    }
}

impl std::fmt::Display for ServiceError {
    /// `ClientError.MSG_TEMPLATE`, with `_get_retry_info`'s note when the
    /// attempts ran out.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = cli_core::botocore_error::client_error(
            self.code().unwrap_or("Unknown").as_bytes(),
            self.operation.as_bytes(),
            self.max_attempts_reached
                .then_some(u64::from(self.retry_attempts)),
            self.message().unwrap_or("Unknown").as_bytes(),
        );
        f.write_str(&String::from_utf8_lossy(&text))
    }
}

impl std::error::Error for ServiceError {}

/// A request that could not be serialized from its input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SerializeError {
    pub message: String,
}

/// A response that could not be parsed: botocore's `ResponseParserError`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
}

macro_rules! message_error {
    ($($name:ident),+) => {$(
        impl $name {
            pub fn new(message: impl Into<String>) -> Self {
                Self { message: message.into() }
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.message)
            }
        }
        impl std::error::Error for $name {}
    )+};
}
message_error!(SerializeError, ParseError);

/// What botocore's paginator (paginate.py) raises of its own as it pages,
/// besides a page's call failure. The CLI's `GeneralExceptionHandler`
/// prints each as `str(exception)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaginationError {
    /// `PaginationError`, with its `message`.
    Pagination(String),
    /// A `ValueError`: `Bad starting token: ...`, or `int()`'s of
    /// `MaxItems` or `PageSize`.
    Value(String),
    /// Any other exception the Python route raises on the same input (a
    /// `TypeError`, `KeyError`, `IndexError`, `AttributeError` or
    /// `OverflowError`): its text.
    Other(String),
}

impl std::fmt::Display for PaginationError {
    /// `PaginationError.fmt`, and the other exceptions' `str`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pagination(message) => write!(f, "Error during pagination: {message}"),
            Self::Value(text) | Self::Other(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for PaginationError {}

/// Why a call has no output.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum CallError {
    /// `ParamValidationError`.
    Validation(Report),
    /// `EndpointResolutionError` and the rule errors botocore raises as
    /// others: the text botocore prints.
    Endpoint(String),
    /// `NoRegionError`.
    NoRegion,
    /// `NoCredentialsError`.
    NoCredentials,
    /// A credential provider that failed, with the text of its error.
    Credentials(String),
    Serialize(SerializeError),
    /// An interceptor's failure.
    Hook(HookError),
    /// No response, from the endpoint at `endpoint_url`.
    Transport {
        error: TransportError,
        endpoint_url: String,
    },
    /// `ResponseParserError`.
    Parse(ParseError),
    /// `ClientError`.
    Service(Box<ServiceError>),
    /// The user interrupted the call.
    Interrupted,
    /// An operation the model does not have.
    UnknownOperation(String),
    /// A call the model client leaves to the Python route, decided before
    /// any request.
    Unsupported(Unsupported),
    /// What the paginator raises between its calls.
    Pagination(PaginationError),
}

impl From<PaginationError> for CallError {
    fn from(error: PaginationError) -> Self {
        Self::Pagination(error)
    }
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(report) => f.write_str(&report.message()),
            Self::Endpoint(text) | Self::Credentials(text) => f.write_str(text),
            Self::NoRegion => f.write_str(cli_core::botocore_error::NO_REGION),
            Self::NoCredentials => f.write_str(cli_core::botocore_error::NO_CREDENTIALS),
            Self::Serialize(error) => error.fmt(f),
            Self::Hook(error) => error.fmt(f),
            Self::Transport {
                error,
                endpoint_url,
            } => transport_text(f, error, endpoint_url),
            Self::Parse(error) => error.fmt(f),
            Self::Service(error) => error.fmt(f),
            Self::Interrupted => f.write_str("the call was interrupted"),
            Self::UnknownOperation(name) => write!(f, "Unknown operation: {name}"),
            Self::Unsupported(unsupported) => write!(f, "not supported natively: {unsupported}"),
            Self::Pagination(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for CallError {}

/// The text of the exception botocore's session raises for `error`
/// (botocore/exceptions.py, each class's `fmt`).
fn transport_text(
    f: &mut std::fmt::Formatter<'_>,
    error: &TransportError,
    url: &str,
) -> std::fmt::Result {
    use cli_core::botocore_error as text;
    match error {
        TransportError::Connect => f.write_str(&text::endpoint_connection(url)),
        TransportError::ConnectTimeout => f.write_str(&text::connect_timeout(url)),
        TransportError::ReadTimeout => f.write_str(&text::read_timeout(url)),
        TransportError::ConnectionClosed => f.write_str(&text::connection_closed(url)),
        TransportError::Tls { reason } => f.write_str(&text::ssl_validation(url, reason)),
        TransportError::Proxy { url: proxy } => f.write_str(&String::from_utf8_lossy(
            &text::proxy_connection(proxy.as_bytes()),
        )),
        TransportError::Client { reason } => f.write_str(&text::http_client(reason)),
        TransportError::Interrupted => f.write_str("the request was interrupted"),
    }
}

#[cfg(test)]
mod tests;
