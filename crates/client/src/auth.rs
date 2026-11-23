//! How a request is authenticated: the scheme the model and the endpoint
//! choose (`OperationModel.resolved_auth_type`, the endpoint's
//! `authSchemes`), which the `select_auth` stage may replace.

/// A request's auth scheme, with what signing needs from the endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthScheme {
    /// SigV4 (`v4`), for the signing region and service name.
    SigV4 {
        signing_region: String,
        signing_name: String,
    },
    /// Sent unsigned (`smithy.api#noAuth`, `--no-sign-request`).
    None,
}
