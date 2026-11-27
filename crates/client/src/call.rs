//! The call loop: botocore's `BaseClient._make_api_call`,
//! `_convert_to_request_dict` and `Endpoint._send_request`, with the
//! customizations' hooks at the points their botocore events fire.
//!
//! [`Client::prepare`] does everything that needs no network: the input
//! hooks, the endpoint, validation, serialization and auth selection, so
//! a caller can send a call it cannot run natively to the Python route
//! before anything is sent. [`Client::send`] runs the attempts: sign, add
//! the unsigned `amz-sdk-invocation-id`, `amz-sdk-request` and
//! `Content-Length`, send, parse, and retry. botocore signs first:
//! `RequestSigner.handler` is registered on `request-created.<service>`,
//! which the emitter runs before `handlers.add_retry_headers` on the
//! generic `request-created` (hooks.py `PrefixTrie.prefix_search`: the more
//! specific event's handlers first), and `Content-Length` is set after the
//! event, when `Endpoint.create_request` prepares the request
//! (awsrequest.py `AWSRequestPreparer._prepare_headers`). So the signature
//! covers none of the three.
//!
//! The phases themselves, in that order, are [`crate::orchestrator`]'s.
// The call's tests name the refusals through this module, as they did
// when the prepare phases were defined here.
#[cfg(test)]
use crate::admission::Unsupported;
use crate::auth::SelectedAuth;
use crate::config_bag::Slots;
use crate::credentials::CredentialSource;
use crate::endpoint::{EndpointConfig, EndpointData};
use crate::error::CallError;
use crate::hooks::Customization;
use crate::http::{Headers, HttpRequest, HttpResponse};
use crate::interceptors::InterceptorContext;
use crate::protocol::{self, ClientProtocol, ParseHooks, Serialized};
use crate::runtime::Runtime;
use crate::runtime_components::RuntimeComponents;
use crate::transport::{Timeouts, Transport};
pub use crate::useragent::UserAgentContext;
use cli_core::client_config::ClientSettings;
use cli_core::client_config::user_agent::Features;
use model::shapes::Model;
use model::value::Value;
use std::sync::Arc;

/// Everything a client is built from, one per service.
#[derive(Debug)]
pub struct ClientParts {
    /// The model directory name, which customizations scope by.
    pub service: String,
    pub model: Arc<Model>,
    pub endpoints: EndpointData,
    pub endpoint: EndpointConfig,
    pub settings: ClientSettings,
    pub credentials: Arc<dyn CredentialSource>,
    pub transport: Arc<dyn Transport>,
    pub runtime: Arc<dyn Runtime>,
    pub customizations: &'static [&'static Customization],
    /// The response parsers' timestamp and blob handlers: the CLI's own,
    /// which it sets on every client (`add_timestamp_parser`).
    pub parse: ParseHooks,
    pub user_agent: UserAgentContext,
}

/// A call's output and what botocore's `ResponseMetadata` holds of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    pub value: Value,
    pub status: u16,
    pub headers: Headers,
    /// `RetryAttempts`: attempts after the first.
    pub retry_attempts: u32,
}

/// A client of one service.
#[derive(Debug)]
pub struct Client {
    /// The model directory name, which customizations scope by.
    pub(crate) service: String,
    pub(crate) model: Arc<Model>,
    pub(crate) settings: ClientSettings,
    pub(crate) customizations: &'static [&'static Customization],
    pub(crate) parse: ParseHooks,
    pub(crate) user_agent: UserAgentContext,
    /// The hyphenized `serviceId`, the hooks' second scope key.
    pub(crate) service_id: Option<String>,
    /// The client configuration, built once by `Client::new`: every call
    /// and page of the client shares its one retry strategy and quota,
    /// and its service's customizations, which op scope filters per call.
    pub(crate) components: RuntimeComponents,
    pub(crate) region: Option<String>,
    pub(crate) timeouts: Timeouts,
    /// Whether the call runs botocore's flexible checksums
    /// ([`crate::checksum`]) and admission lets the checksum traits
    /// through: built but switched off, so only a test sets it.
    pub(crate) http_checksum: bool,
}

/// A call ready to send: [`Client::prepare`]'s result, which owns all it
/// holds, so a caller can keep it beside its client and send it later
/// without preparing again.
#[derive(Debug)]
pub struct Prepared {
    pub(crate) operation: String,
    pub(crate) slots: Slots,
    pub(crate) request: HttpRequest,
    /// The auth option and the scheme that signs each attempt.
    pub(crate) auth: SelectedAuth,
    /// The serialized request and the call's feature ids, to join again
    /// when the deferred account id changes the endpoint.
    pub(crate) serialized: Serialized,
    pub(crate) features: Features,
    pub(crate) input: Value,
    /// The endpoint the deferred account id left out.
    pub(crate) endpoint_url: String,
}

impl Client {
    /// A client speaking the protocol botocore resolves for the model.
    ///
    /// # Errors
    /// [`Unsupported::Protocol`] for one not implemented; see
    /// [`Self::with_protocol`].
    pub fn new(parts: ClientParts) -> Result<Self, CallError> {
        let protocol = protocol::select(&parts.model).map_err(CallError::Unsupported)?;
        Self::with_protocol(parts, protocol)
    }
    /// A client speaking `protocol`, its customizations chosen by scope
    /// and its retry settings resolved (`ClientArgsCreator`): the user's
    /// `max_attempts`, else a customization's default, else 3.
    ///
    /// # Errors
    /// `NoRegionError`, which botocore raises as it creates the client.
    pub fn with_protocol(
        parts: ClientParts,
        protocol: &'static dyn ClientProtocol,
    ) -> Result<Self, CallError> {
        crate::orchestrator::configure(parts, protocol)
    }
    /// Runs `operation` with `input`: [`Self::prepare`], then [`Self::send`].
    ///
    /// # Errors
    /// See [`CallError`].
    pub fn call(&self, operation: &str, input: Value) -> Result<Output, CallError> {
        let prepared = self.prepare(operation, input)?;
        self.send(prepared)
    }
    /// [`Self::call`], its `User-Agent` also naming `features`: the feature
    /// ids a caller registers in the call's context, as
    /// `PageIterator._make_request` registers `PAGINATOR`
    /// (`with_current_context(partial(register_feature_id, ...))`).
    ///
    /// # Errors
    /// See [`CallError`].
    pub(crate) fn call_with_features(
        &self,
        operation: &str,
        input: Value,
        features: Features,
    ) -> Result<Output, CallError> {
        let prepared = self.prepare_with_features(operation, input, features)?;
        self.send(prepared)
    }
    /// The service's model.
    pub(crate) fn model(&self) -> &Model {
        &self.model
    }
}

impl Client {
    /// Everything before the first attempt, in botocore's order: the
    /// static refusals ([`crate::admission::static_refusal`]), the
    /// `modify_before_serialization` hooks (`provide-client-params`,
    /// `before-parameter-build`), idempotency tokens
    /// (`generate_idempotent_uuid`), the endpoint (`_resolve_endpoint_ruleset`),
    /// validation and serialization (`_convert_to_request_dict`), the join
    /// with the endpoint and the `User-Agent` (`prepare_request_dict`), the
    /// `modify_before_retry_loop` hooks (`before-call`), and auth selection.
    ///
    /// It needs no credentials, network or runtime, so every refusal comes
    /// before any credential provider runs: the endpoint resolves with
    /// `AWS::Auth::AccountId` deferred, as botocore defers it
    /// (`credentials.get_deferred_property`), and [`Self::send`] resolves
    /// again only when the credentials carry an account id the rule set
    /// reads.
    ///
    /// # Errors
    /// Anything botocore raises before it sends, and
    /// [`CallError::Unsupported`] for a call to leave to the Python route.
    pub fn prepare(&self, operation: &str, input: Value) -> Result<Prepared, CallError> {
        self.prepare_with_features(operation, input, Features::default())
    }
    /// [`Self::prepare`], with `extra` among the call's feature ids.
    pub(crate) fn prepare_with_features(
        &self,
        operation: &str,
        input: Value,
        extra: Features,
    ) -> Result<Prepared, CallError> {
        crate::orchestrator::prepare(self, operation, input, extra)
    }
}

impl Client {
    /// Sends `prepared`, retrying as standard mode does
    /// (`Endpoint._send_request`).
    ///
    /// # Errors
    /// See [`CallError`].
    pub fn send(&self, prepared: Prepared) -> Result<Output, CallError> {
        crate::orchestrator::send(self, prepared)
    }
    /// A parsed response no retry was decided for: the quota's refund
    /// (`release_retry_quota`), then the `modify_before_completion` hooks
    /// (`after-call`), which see the outcome as `Ok` and may turn it into
    /// an error; as botocore's `after-call`, they run for a success only.
    /// It stays a method here, where the source path tests of
    /// `testing::defines` name it.
    pub(crate) fn succeeded(
        &self,
        ctx: &mut InterceptorContext<'_>,
        attempt: u32,
        response: HttpResponse,
        value: Value,
        last_cost: Option<u32>,
    ) -> Result<Output, CallError> {
        self.components.retry_strategy.succeeded(last_cost);
        let mut outcome = Ok(value);
        for hook in self.components.plugins.interceptors(ctx.operation.name()) {
            hook.modify_before_completion(ctx, &mut outcome)
                .map_err(|error| CallError::Hook(error.after_prepare()))?;
        }
        Ok(Output {
            value: outcome?,
            status: response.status,
            headers: response.headers,
            retry_attempts: attempt - 1,
        })
    }
}

#[cfg(test)]
mod tests;
