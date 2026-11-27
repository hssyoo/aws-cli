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
//!
//! The Smithy-shaped pipeline's roots, filled step by step: [`codec`],
//! [`orchestrator`], [`interceptors`], [`config_bag`],
//! [`runtime_components`], [`runtime_plugin`], [`identity`], [`retries`].
#![forbid(unsafe_code)]

pub mod admission;
pub mod auth;
pub mod call;
pub mod checksum;
pub mod codec;
pub mod config_bag;
pub mod credentials;
pub mod endpoint;
pub mod error;
pub mod hooks;
pub mod http;
pub mod identity;
pub mod interceptors;
pub mod orchestrator;
pub mod paginate;
pub mod protocol;
mod python;
pub mod retries;
pub mod retry;
pub mod runtime;
pub mod runtime_components;
pub mod runtime_plugin;
pub mod transport;
pub mod useragent;
pub mod xml;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
