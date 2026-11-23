//! A model-driven AWS API client, botocore's `BaseClient` and `Endpoint`
//! rebuilt on [`model`]: the seams the call pipeline is built from.
//!
//! - [`http`]: requests and responses, headers in botocore's order.
//! - [`transport`]: the one way out to the network.
//! - [`protocol`]: a wire protocol, chosen as botocore chooses it.
//! - [`error`]: what a call fails with, as botocore's exceptions read.
//! - [`hooks`]: customizations, their scopes, defaults and stages.
//! - [`runtime`]: the clock, uuids, sleep and randomness, injected.
//!
//! The call loop, signing, retries and endpoint resolution build on these.
//! No service is named here: a service's own behaviour is a
//! [`hooks::Customization`].
#![forbid(unsafe_code)]

pub mod admission;
pub mod auth;
pub mod call;
pub mod credentials;
pub mod endpoint;
pub mod error;
pub mod hooks;
pub mod http;
pub mod protocol;
pub mod retry;
pub mod runtime;
pub mod transport;
pub mod useragent;
pub mod xml;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

// The census test (`tests/census.rs`) counts refusals with the CLI's
// customization table; the unit tests do not use it.
#[cfg(test)]
use customizations as _;
