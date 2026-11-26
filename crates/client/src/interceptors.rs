//! Interceptors (decision D2): the modify positions botocore has client
//! events for, two extensions for botocore positions the SRA has no hook
//! for, and one read-only [`Intercept::observe`] seam in place of the
//! SRA's read hooks. Each modify hook takes the value it may change as a
//! typed argument, as today's stages do; the argument is the typestate.
//!
//! The SRA names stay reserved positions (`interceptors.rs` of the Rust
//! runtime API): a name is a position, and a public position never moves.
//!
//! "Hook" is the [`Intercept`] method of that name; "observe" is the
//! [`HookPoint`] of that name; "core" is a hook only the core implements.
//!
//! | SRA position | Here | botocore event |
//! |---|---|---|
//! | `read_before_execution` | observe | — |
//! | `modify_before_serialization` | hook | `provide-client-params`, `before-parameter-build` |
//! | `read_before_serialization` | observe | (history `API_CALL`) |
//! | `modify_endpoint_builtins` | extension hook | `before-endpoint-resolution` |
//! | `read_after_serialization` | observe | — |
//! | `modify_before_retry_loop` | hook | `before-call` |
//! | `read_before_attempt` | observe | — |
//! | `modify_before_signing` | hook | `request-created`, `before-sign` |
//! | `read_before_signing` | reserved: the view would hold the signature | — |
//! | `read_after_signing` | reserved, as `read_before_signing` | — |
//! | `modify_before_transmit` | core | (`add_retry_headers`) |
//! | `read_before_transmit` | observe | (history `HTTP_REQUEST`) |
//! | `short_circuit_transmit` | extension hook | `before-send` |
//! | `read_after_transmit` | observe | `response-received` |
//! | `modify_before_deserialization` | hook | `before-parse` |
//! | `read_before_deserialization` | reserved | — |
//! | `read_after_deserialization` | observe | (history `PARSED_RESPONSE`) |
//! | `modify_before_attempt_completion` | core | — |
//! | `read_after_attempt` | observe | — |
//! | `modify_before_completion` | hook | `after-call` |
//! | `read_after_execution` | observe | `after-call-error` (in part) |
//!
//! Auth selection (`choose-signer`) and retry classification
//! (`needs-retry`) are components, not hooks; long polling is a plugin
//! flag ([`crate::runtime_plugin::Customization`]).
use crate::config_bag::Slots;
use crate::endpoint::Builtins;
use crate::error::CallError;
use crate::http::{HttpRequest, HttpResponse};
use model::schema::OperationSchema;
use model::value::Value;

pub use crate::hooks::{HookError, HookResult, Order};

/// One call as its hooks see it (decision D5): the operation, both scope
/// keys and the shared state. Built in `prepare`, carried as its state
/// into the attempts, resumed in `send`.
pub struct InterceptorContext<'m> {
    /// The model directory name, which plugins scope by.
    pub service: &'m str,
    /// The hyphenized `serviceId` (`ServiceId.hyphenize()`), the key
    /// botocore's own tables use; `None` for a model without one.
    pub service_id: Option<&'m str>,
    pub operation: OperationSchema<'m>,
    /// 1 for the first attempt.
    pub attempt: u32,
    state: Slots,
}

impl<'m> InterceptorContext<'m> {
    pub fn new(
        service: &'m str,
        service_id: Option<&'m str>,
        operation: OperationSchema<'m>,
    ) -> Self {
        Self::resume(service, service_id, operation, Slots::default())
    }
    /// A context that carries on with `state`, at the first attempt.
    pub fn resume(
        service: &'m str,
        service_id: Option<&'m str>,
        operation: OperationSchema<'m>,
        state: Slots,
    ) -> Self {
        Self {
            service,
            service_id,
            operation,
            attempt: 1,
            state,
        }
    }
    /// The call's shared state, to carry past the borrow of its model.
    pub fn into_state(self) -> Slots {
        self.state
    }
    /// The call's value of type `T`, made from its default on first use.
    pub fn state<T: Default + Send + 'static>(&mut self) -> &mut T {
        self.state.get()
    }
    /// The call's value of type `T` if a hook has made it.
    pub fn find_state<T: 'static>(&self) -> Option<&T> {
        self.state.find()
    }
}

impl std::fmt::Debug for InterceptorContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InterceptorContext")
            .field("service", &self.service)
            .field("service_id", &self.service_id)
            .field("operation", &self.operation.name())
            .field("attempt", &self.attempt)
            .field("state", &self.state)
            .finish()
    }
}

/// A read-only point of the call and what exists there. The points after
/// signing see a signed request; they are workspace-internal, and any
/// plugin façade redacts them.
#[derive(Clone, Copy, Debug)]
pub enum HookPoint<'a> {
    BeforeExecution {
        input: &'a Value,
    },
    /// After the input hooks and the idempotency tokens.
    BeforeSerialization {
        input: &'a Value,
    },
    AfterSerialization {
        request: &'a HttpRequest,
    },
    BeforeAttempt {
        request: &'a HttpRequest,
    },
    BeforeTransmit {
        request: &'a HttpRequest,
    },
    /// Only when a response arrived.
    AfterTransmit {
        response: &'a HttpResponse,
    },
    AfterDeserialization {
        output: &'a Result<Value, CallError>,
    },
    AfterAttempt {
        output: &'a Result<Value, CallError>,
    },
    AfterExecution {
        output: &'a Result<Value, CallError>,
    },
}

impl HookPoint<'_> {
    /// The point's stable name: its SRA position.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::BeforeExecution { .. } => "read_before_execution",
            Self::BeforeSerialization { .. } => "read_before_serialization",
            Self::AfterSerialization { .. } => "read_after_serialization",
            Self::BeforeAttempt { .. } => "read_before_attempt",
            Self::BeforeTransmit { .. } => "read_before_transmit",
            Self::AfterTransmit { .. } => "read_after_transmit",
            Self::AfterDeserialization { .. } => "read_after_deserialization",
            Self::AfterAttempt { .. } => "read_after_attempt",
            Self::AfterExecution { .. } => "read_after_execution",
        }
    }
}

/// A call's hooks. Each does nothing unless implemented. A modify hook's
/// error ends the call; an observer's never does ([`observe_all`]).
pub trait Intercept: Sync + std::fmt::Debug {
    /// A name for diagnostics.
    fn name(&self) -> &'static str;
    /// Where it runs among the hooks of its position: by band, built-ins
    /// first within a band, then registration order (decision D6).
    fn order(&self) -> Order {
        Order::Default
    }
    /// The input before idempotency tokens, the endpoint and validation.
    fn modify_before_serialization(
        &self,
        _ctx: &mut InterceptorContext<'_>,
        _input: &mut Value,
    ) -> HookResult {
        Ok(())
    }
    /// The built-ins before the rule parameters are assembled: a plugin
    /// sets one by its `builtIn` name; an operation's static or context
    /// parameter still wins over it. A built-in a rule set requires is one
    /// the plugin names in its admission metadata.
    fn modify_endpoint_builtins(
        &self,
        _ctx: &mut InterceptorContext<'_>,
        _builtins: &mut Builtins,
    ) -> HookResult {
        Ok(())
    }
    /// The request once serialized and joined to its endpoint, once per
    /// call (and again when the account id changes the endpoint).
    fn modify_before_retry_loop(
        &self,
        _ctx: &mut InterceptorContext<'_>,
        _request: &mut HttpRequest,
    ) -> HookResult {
        Ok(())
    }
    /// Each attempt's request before credentials load and it is signed.
    /// An error ends the call without a retry.
    fn modify_before_signing(
        &self,
        _ctx: &mut InterceptorContext<'_>,
        _request: &mut HttpRequest,
    ) -> HookResult {
        Ok(())
    }
    /// The signed request: the core adds its unsigned headers here.
    fn modify_before_transmit(
        &self,
        _ctx: &mut InterceptorContext<'_>,
        _request: &mut HttpRequest,
    ) -> HookResult {
        Ok(())
    }
    /// A response in place of sending; the first `Some` is the attempt's
    /// response and nothing is sent. An error ends the call without a
    /// retry.
    fn short_circuit_transmit(
        &self,
        _ctx: &mut InterceptorContext<'_>,
        _request: &HttpRequest,
    ) -> Result<Option<HttpResponse>, HookError> {
        Ok(None)
    }
    /// Each response before it is parsed.
    fn modify_before_deserialization(
        &self,
        _ctx: &mut InterceptorContext<'_>,
        _response: &mut HttpResponse,
    ) -> HookResult {
        Ok(())
    }
    /// Each attempt's outcome before the retry decision; the core's
    /// finalizers run here.
    fn modify_before_attempt_completion(
        &self,
        _ctx: &mut InterceptorContext<'_>,
        _outcome: &mut Result<Value, CallError>,
    ) -> HookResult {
        Ok(())
    }
    /// The call's outcome before it is returned; today's output hooks act
    /// on `Ok` only.
    fn modify_before_completion(
        &self,
        _ctx: &mut InterceptorContext<'_>,
        _outcome: &mut Result<Value, CallError>,
    ) -> HookResult {
        Ok(())
    }
    /// A read-only look at `point`. Its error is reported and ignored.
    fn observe(&self, _point: HookPoint<'_>, _ctx: &InterceptorContext<'_>) -> HookResult {
        Ok(())
    }
}

/// An observer's error, which the call reports (under `--debug`) and
/// otherwise ignores.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObserveFailure {
    pub interceptor: &'static str,
    pub point: &'static str,
    pub error: HookError,
}

/// Shows `point` to every interceptor in order. An error goes to `report`
/// and the next observer still runs; the call goes on either way.
pub fn observe_all<'i>(
    interceptors: impl IntoIterator<Item = &'i dyn Intercept>,
    point: HookPoint<'_>,
    ctx: &InterceptorContext<'_>,
    report: &mut dyn FnMut(ObserveFailure),
) {
    for interceptor in interceptors {
        if let Err(error) = interceptor.observe(point, ctx) {
            report(ObserveFailure {
                interceptor: interceptor.name(),
                point: point.name(),
                error,
            });
        }
    }
}

#[cfg(test)]
mod tests;
