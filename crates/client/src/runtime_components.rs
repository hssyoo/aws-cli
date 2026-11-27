//! The components a client's calls run on, built once in `Client::new` so
//! that every call and page of the client shares one retry quota.
//!
//! A slot with one implementation is a concrete type (decision D4: the
//! standard retry strategy, the endpoint resolver, the auth option
//! resolver); a trait object only where two or more exist (protocol, auth
//! schemes, identity, transport, runtime).
//! Plugins configure a client by data ([`crate::runtime_plugin`]) and never
//! receive the components.
use crate::auth::{AUTH_SCHEMES, AuthScheme, AuthSchemeOptionResolver};
use crate::credentials::CredentialSource;
use crate::endpoint::{EndpointConfig, EndpointData, EndpointParams, ResolvedEndpoint};
use crate::error::CallError;
use crate::identity::ResolveIdentity;
use crate::interceptors::InterceptorContext;
use crate::protocol::ClientProtocol;
use crate::retry::{Attempt, ErrorKind, RetryDecision, Standard};
use crate::runtime::Runtime;
use crate::runtime_plugin::ServicePlugins;
use crate::transport::Transport;
use std::sync::Arc;

/// botocore's standard mode default (`DEFAULT_MAX_ATTEMPTS`).
const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// A retry classifier a plugin adds: it runs before the standard
/// classification, and the first decision wins (`needs-retry`).
pub trait ClassifyRetry: Sync + std::fmt::Debug {
    /// A name for diagnostics.
    fn name(&self) -> &'static str;
    /// A decision in place of the standard one, or `None` for none.
    fn classify_retry(
        &self,
        ctx: &mut InterceptorContext<'_>,
        attempt: &Attempt<'_>,
    ) -> Option<RetryDecision>;
}

/// The endpoint resolver: the rule set and partitions with the client's
/// endpoint configuration. One implementation, so a concrete type.
#[derive(Debug)]
pub struct EndpointResolver {
    pub data: EndpointData,
    pub config: EndpointConfig,
}

impl EndpointResolver {
    /// The endpoint the rule set gives `params`.
    ///
    /// # Errors
    /// botocore's endpoint errors, as [`crate::endpoint::resolve`] reports them.
    pub fn resolve(&self, params: &EndpointParams) -> Result<ResolvedEndpoint, CallError> {
        crate::endpoint::resolve(&self.data, params)
    }
}

/// What a client's components are built from.
#[derive(Debug)]
pub struct ComponentParts {
    pub protocol: &'static dyn ClientProtocol,
    pub endpoints: EndpointData,
    pub endpoint: EndpointConfig,
    pub credentials: Arc<dyn CredentialSource>,
    pub transport: Arc<dyn Transport>,
    pub runtime: Arc<dyn Runtime>,
    /// The user's `max_attempts`, if set.
    pub max_attempts: Option<u32>,
}

/// A client's components.
#[derive(Debug)]
pub struct RuntimeComponents {
    pub protocol: &'static dyn ClientProtocol,
    pub endpoint_resolver: EndpointResolver,
    /// The one strategy, and so the one quota, of all the client's calls.
    pub retry_strategy: Standard,
    pub auth_scheme_option_resolver: AuthSchemeOptionResolver,
    /// The schemes an auth option is matched to, in order.
    pub auth_schemes: &'static [&'static dyn AuthScheme],
    /// The credentials SigV4 signs with, asked on every attempt.
    pub identity_resolver: Arc<dyn ResolveIdentity>,
    pub http_client: Arc<dyn Transport>,
    pub runtime: Arc<dyn Runtime>,
    /// The interceptors, retry classifiers, long-polling scopes and
    /// defaults of the client's service; op scope filters them per call.
    pub plugins: ServicePlugins,
}

impl RuntimeComponents {
    pub fn new(parts: ComponentParts, plugins: ServicePlugins) -> Self {
        let retry_strategy = retry_strategy(&plugins, parts.max_attempts);
        Self {
            protocol: parts.protocol,
            endpoint_resolver: EndpointResolver {
                data: parts.endpoints,
                config: parts.endpoint,
            },
            retry_strategy,
            auth_scheme_option_resolver: AuthSchemeOptionResolver,
            auth_schemes: &AUTH_SCHEMES,
            identity_resolver: parts.credentials,
            http_client: parts.transport,
            runtime: parts.runtime,
            plugins,
        }
    }
}

/// The standard strategy as `ClientArgsCreator` configures it: the user's
/// `max_attempts`, else a plugin's default, else 3; a plugin's base delay
/// for errors that are not throttling, else the standard one.
pub fn retry_strategy(plugins: &ServicePlugins, configured: Option<u32>) -> Standard {
    let defaults = plugins.defaults();
    let max_attempts = defaults
        .max_attempts(configured)
        .unwrap_or(DEFAULT_MAX_ATTEMPTS);
    let base = defaults
        .retry_base_delay()
        .unwrap_or(ErrorKind::Transient.base());
    Standard::new(max_attempts, base)
}

#[cfg(test)]
mod tests;
