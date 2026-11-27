//! The call loop: botocore's `BaseClient._make_api_call`,
//! `_convert_to_request_dict` and `Endpoint._send_request`, with the
//! interceptor stages at the points their botocore events fire.
//!
//! [`Client::prepare`] does everything that needs no network: the input
//! stages, the endpoint, validation, serialization and auth selection, so
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
use crate::auth::AuthScheme;
use crate::credentials::CredentialSource;
use crate::endpoint::{EndpointConfig, EndpointData};
use crate::error::CallError;
use crate::hooks::{CallContext, Customization, Hooks, Slots};
use crate::http::{Headers, HttpRequest, HttpResponse, Method};
use crate::protocol::{self, ClientProtocol, ParseHooks, Serialized};
use crate::runtime::Runtime;
use crate::runtime_components::RuntimeComponents;
use crate::transport::{Timeouts, Transport};
pub use crate::useragent::UserAgentContext;
use cli_core::client_config::ClientSettings;
use cli_core::client_config::user_agent::Features;
use model::schema::{Flag, OperationSchema};
use model::shapes::Model;
use model::value::{Map, Value};
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
    /// The client configuration, built once by `Client::new`: every call
    /// and page of the client shares its one retry strategy and quota.
    pub(crate) components: RuntimeComponents,
    /// The service's customizations, which op scope filters per call.
    pub(crate) hooks: Hooks,
    pub(crate) region: Option<String>,
    pub(crate) timeouts: Timeouts,
}

/// A call ready to send: [`Client::prepare`]'s result, which owns all it
/// holds, so a caller can keep it beside its client and send it later
/// without preparing again.
#[derive(Debug)]
pub struct Prepared {
    pub(crate) operation: String,
    pub(crate) slots: Slots,
    pub(crate) request: HttpRequest,
    pub(crate) scheme: AuthScheme,
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
    /// `modify_input` stage (`provide-client-params`,
    /// `before-parameter-build`), idempotency tokens
    /// (`generate_idempotent_uuid`), the endpoint (`_resolve_endpoint_ruleset`),
    /// validation and serialization (`_convert_to_request_dict`), the join
    /// with the endpoint and the `User-Agent` (`prepare_request_dict`), the
    /// `after_serialize` stage, and auth selection.
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
    /// `generate_idempotent_uuid`: each input member marked
    /// `idempotencyToken` and left out gets `str(uuid.uuid4())`.
    pub(crate) fn fill_idempotency_tokens(
        &self,
        operation: OperationSchema<'_>,
        input: &mut Value,
    ) {
        let Some(shape) = operation.input() else {
            return;
        };
        let tokens: Vec<&str> = shape
            .members()
            .filter(|member| member.flag(Flag::IdempotencyToken))
            .filter_map(|member| member.member_name())
            .collect();
        if tokens.is_empty() {
            return;
        }
        if *input == Value::Null {
            *input = Value::Map(Map::new());
        }
        if let Value::Map(map) = input {
            for name in tokens {
                if !map.contains_key(name) {
                    map.insert(
                        name.to_owned(),
                        Value::String(self.components.runtime.uuid4()),
                    );
                }
            }
        }
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
    /// The core's part of an attempt's `modify_before_transmit`: the
    /// unsigned headers, after the signature (`add_retry_headers`, with
    /// `max` as `seed_max_attempts` seeds it, then `_prepare_headers`).
    pub(crate) fn attempt_request(
        &self,
        request: &mut HttpRequest,
        attempt: u32,
        invocation: &str,
        ttl: Option<&str>,
    ) {
        request.headers.set("amz-sdk-invocation-id", invocation);
        let ttl = ttl.map(|ttl| format!("ttl={ttl}; ")).unwrap_or_default();
        let max = self.components.retry_strategy.max_attempts();
        request.headers.set(
            "amz-sdk-request",
            format!("{ttl}attempt={attempt}; max={max}"),
        );
        let framed = !matches!(request.method, Method::Get | Method::Head | Method::Options);
        if framed && !request.headers.contains("Content-Length") {
            request
                .headers
                .set("Content-Length", request.body.len().to_string());
        }
    }
    /// A parsed response no retry was decided for: the quota's refund
    /// (`release_retry_quota`), then `modify_before_completion`, the
    /// `modify_output` stage.
    pub(crate) fn succeeded(
        &self,
        cx: &mut CallContext<'_>,
        attempt: u32,
        response: HttpResponse,
        mut value: Value,
        last_cost: Option<u32>,
    ) -> Result<Output, CallError> {
        self.components.retry_strategy.succeeded(last_cost);
        for hook in self.hooks.interceptors(cx.operation.name()) {
            hook.modify_output(cx, &mut value)
                .map_err(|error| CallError::Hook(error.after_prepare()))?;
        }
        Ok(Output {
            value,
            status: response.status,
            headers: response.headers,
            retry_attempts: attempt - 1,
        })
    }
}

#[cfg(test)]
mod tests;
