//! Customizations: typed stages in place of botocore's string events, and
//! the unit that registers them.
//!
//! A [`Customization`] applies to a [`Scope`], supplies defaults for
//! settings the user left unset, and adds [`Interceptor`]s at the call's
//! stages. It names the Python handlers it ports (`replaces`), which is how
//! admission knows an operation's every customization has a native one.
//! A client picks the customizations of its service once, when it is
//! built ([`Hooks::for_service`]).
use crate::auth::AuthScheme;
use crate::endpoint::Builtins;
use crate::http::{HttpRequest, HttpResponse};
use crate::retry::{Attempt, RetryDecision};
use model::shapes::{Model, Operation};
use model::value::Value;
use std::any::Any;
use std::time::Duration;

/// Which calls something applies to: a service, by its model directory
/// name (botocore's `ServiceModel.service_name`, the name the loader finds
/// the model under), and an operation, by its model name. `None` matches every one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scope {
    pub service: Option<&'static str>,
    pub operation: Option<&'static str>,
}

impl Scope {
    pub const ALL: Self = Self {
        service: None,
        operation: None,
    };
    pub const fn service(name: &'static str) -> Self {
        Self {
            service: Some(name),
            operation: None,
        }
    }
    pub const fn operation(service: &'static str, operation: &'static str) -> Self {
        Self {
            service: Some(service),
            operation: Some(operation),
        }
    }
    /// Whether some call of `service` is in scope.
    pub fn matches_service(&self, service: &str) -> bool {
        self.service.is_none_or(|name| name == service)
    }
    pub fn matches(&self, service: &str, operation: &str) -> bool {
        self.matches_service(service) && self.operation.is_none_or(|name| name == operation)
    }
}

/// Where an interceptor runs among those of its stage: botocore registers
/// handlers first, in the middle or last (`register_first`,
/// `register_last`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Order {
    First,
    Default,
    Last,
}

/// An interceptor's failure, which ends the call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookError {
    /// Leave the call to the Python route, for the reason named: a partial
    /// port declines what it does not reproduce. Only the stages
    /// [`crate::call::Client::prepare`] runs may refuse (`modify_input`,
    /// `endpoint_builtins`, `after_serialize`), which happen before any
    /// credential provider runs or anything is sent; there it becomes
    /// [`crate::admission::Unsupported::Hook`]. A later stage's refusal is
    /// a fault of the customization, and the call fails with
    /// [`Self::Failed`] instead.
    Refuse(String),
    /// botocore's `ParamValidationError`: the input is invalid, which the
    /// CLI reports as a parameter validation failure (252).
    Validation(String),
    /// Any other exception a ported handler raises, `str(exception)`.
    Failed(String),
}

impl HookError {
    /// The error a stage after `prepare` reports: a refusal there comes too
    /// late to leave the call to Python.
    pub(crate) fn after_prepare(self) -> Self {
        match self {
            Self::Refuse(reason) => Self::Failed(format!(
                "a customization refused a call already started: {reason}"
            )),
            other => other,
        }
    }
}

impl std::fmt::Display for HookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refuse(reason) | Self::Validation(reason) | Self::Failed(reason) => {
                f.write_str(reason)
            }
        }
    }
}

impl std::error::Error for HookError {}

pub type HookResult = Result<(), HookError>;

/// One call's state, which its stages share: botocore's request
/// `context`, with a typed slot in place of each key.
pub struct CallContext<'a> {
    /// The service, as [`Scope::service`] names it.
    pub service: &'a str,
    pub model: &'a Model,
    pub operation: &'a Operation,
    /// 1 for the first attempt.
    pub attempt: u32,
    slots: Vec<Box<dyn Any + Send>>,
}

impl<'a> CallContext<'a> {
    pub fn new(service: &'a str, model: &'a Model, operation: &'a Operation) -> Self {
        Self {
            service,
            model,
            operation,
            attempt: 1,
            slots: Vec::new(),
        }
    }
    /// The call's value of type `T`, made from its default on first use.
    /// Stages that share state agree on its type.
    #[expect(
        clippy::expect_used,
        reason = "the slot was just found or made with type T"
    )]
    pub fn slot<T: Default + Send + 'static>(&mut self) -> &mut T {
        let index = match self.slots.iter().position(|slot| slot.is::<T>()) {
            Some(index) => index,
            None => {
                self.slots.push(Box::new(T::default()));
                self.slots.len() - 1
            }
        };
        self.slots[index]
            .downcast_mut::<T>()
            .expect("a slot of type T")
    }
}

/// What a call's stages share once [`crate::call::Client::prepare`] has
/// run, carried in its [`crate::call::Prepared`] to the attempts.
#[derive(Default)]
pub struct Slots(Vec<Box<dyn Any + Send>>);

impl std::fmt::Debug for Slots {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Slots({})", self.0.len())
    }
}

impl<'a> CallContext<'a> {
    /// The call's shared state, to carry past the borrow of its model.
    pub(crate) fn into_slots(self) -> Slots {
        Slots(self.slots)
    }
    /// A context that resumes `slots` for `operation`.
    pub(crate) fn resume(
        service: &'a str,
        model: &'a Model,
        operation: &'a Operation,
        slots: Slots,
    ) -> Self {
        Self {
            service,
            model,
            operation,
            attempt: 1,
            slots: slots.0,
        }
    }
}

impl std::fmt::Debug for CallContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallContext")
            .field("service", &self.service)
            .field("operation", &self.operation.name)
            .field("attempt", &self.attempt)
            .field("slots", &self.slots.len())
            .finish()
    }
}

/// A call's stages, in the order they run. Each does nothing unless an
/// interceptor implements it; those that can take over say so in their
/// return type.
pub trait Interceptor: Sync + std::fmt::Debug {
    fn order(&self) -> Order {
        Order::Default
    }
    /// The input before validation (`provide-client-params`,
    /// `before-parameter-build`).
    fn modify_input(&self, _cx: &mut CallContext<'_>, _input: &mut Value) -> HookResult {
        Ok(())
    }
    /// The built-ins before the rule parameters are assembled
    /// (`before-endpoint-resolution`'s `builtins` dict): a customization
    /// sets a built-in by its `builtIn` name, and an operation's static or
    /// context parameter still wins over it
    /// (`EndpointRulesetResolver._get_provider_params`). A built-in a rule
    /// set requires is one the customization must name in
    /// [`Customization::builtins`].
    fn endpoint_builtins(&self, _cx: &mut CallContext<'_>, _builtins: &mut Builtins) -> HookResult {
        Ok(())
    }
    /// The request once serialized and joined to its endpoint
    /// (`before-call`'s request dict).
    fn after_serialize(&self, _cx: &mut CallContext<'_>, _request: &mut HttpRequest) -> HookResult {
        Ok(())
    }
    /// A scheme in place of the resolved one; the first `Some` wins.
    fn select_auth(&self, _cx: &mut CallContext<'_>, _resolved: &AuthScheme) -> Option<AuthScheme> {
        None
    }
    /// Each attempt's request before it is signed (`before-sign`).
    fn before_sign(&self, _cx: &mut CallContext<'_>, _request: &mut HttpRequest) -> HookResult {
        Ok(())
    }
    /// A response in place of sending (`before-send`); the first `Some`
    /// is the attempt's response and nothing is sent.
    fn before_transmit(
        &self,
        _cx: &mut CallContext<'_>,
        _request: &HttpRequest,
    ) -> Result<Option<HttpResponse>, HookError> {
        Ok(None)
    }
    /// Each response before it is parsed (`before-parse`).
    fn after_receive(&self, _cx: &mut CallContext<'_>, _response: &mut HttpResponse) -> HookResult {
        Ok(())
    }
    /// A decision in place of the retry policy's (`needs-retry`); the
    /// first `Some` wins.
    fn classify_retry(
        &self,
        _cx: &mut CallContext<'_>,
        _attempt: &Attempt<'_>,
    ) -> Option<RetryDecision> {
        None
    }
    /// Whether the call long-polls: once the retry quota refuses a retry, it
    /// still sleeps the delay a retry would have waited, then stops
    /// (`RetryHandler._is_long_polling_operation`).
    fn long_polls(&self) -> bool {
        false
    }
    /// The output before it is returned (`after-call`).
    fn modify_output(&self, _cx: &mut CallContext<'_>, _output: &mut Value) -> HookResult {
        Ok(())
    }
}

/// A default for a client setting the user left unset: the user's value,
/// whatever it is, wins, as `ClientCreator._register_v2_standard_retries`
/// passes a configured `max_attempts` over `_SERVICE_MAX_ATTEMPTS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingDefault {
    /// `max_attempts`, as botocore's per-service table supplies it
    /// (retries/standard.py `_SERVICE_MAX_ATTEMPTS`).
    MaxAttempts(u32),
    /// The standard backoff's base for errors that are not throttling
    /// (`ExponentialBackoff._SERVICE_BACKOFF_CONFIG`).
    RetryBaseDelay(Duration),
}

/// The unit of registration, for built-ins and later plugins alike.
#[derive(Debug)]
pub struct Customization {
    /// A name for diagnostics.
    pub id: &'static str,
    pub scope: Scope,
    /// Defaults a client of the scope's service takes; only a
    /// customization for a whole service supplies them.
    pub setting_defaults: &'static [SettingDefault],
    /// The endpoint built-ins its interceptors set, by `builtIn` name, so
    /// admission knows before any call that a rule set's required
    /// built-in is supplied ([`crate::admission::static_refusal`]).
    pub builtins: &'static [&'static str],
    pub interceptors: &'static [&'static dyn Interceptor],
    /// The Python handlers this stands in for, as `module.qualname`, the
    /// way the customization-pattern payload entry names them
    /// (`command::patterns`), each with why native may.
    pub replaces: &'static [(&'static str, Basis)],
}

/// Why native stands in for a Python handler or registry entry, so a test
/// can check the claim instead of trusting prose. The client's
/// `Customization::replaces` and the CLI's coverage table both use it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Basis {
    /// Native code does what the handler does. `at` is the Rust path of the
    /// item that does it (`crate::module::Type::method`), which
    /// [`crate::testing::defines`] resolves in the workspace's source.
    Implemented { at: &'static str },
    /// The handler acts only when `acts_when`, and native refuses every
    /// such call before it starts, as `refused_as` says.
    Inert {
        acts_when: &'static str,
        refused_as: Refusal,
    },
}

/// What refuses the calls an inert handler would act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// [`crate::admission::static_refusal`] refuses an operation that
    /// carries this trait, as `Unsupported::Trait` with this key.
    Trait(&'static str),
    /// The router sends an invocation with the setting to Python, for this
    /// reason.
    Router(cli_core::cli_error::PythonReason),
}

/// The defaults a client's customizations supply, each from the first in
/// the table that supplies it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Defaults {
    max_attempts: Option<u32>,
    retry_base_delay: Option<Duration>,
}

impl Defaults {
    /// `max_attempts`: the user's, else a customization's, else unset.
    pub fn max_attempts(&self, configured: Option<u32>) -> Option<u32> {
        configured.or(self.max_attempts)
    }
    /// The non-throttling backoff base a customization supplies, else
    /// unset; no setting configures it.
    pub const fn retry_base_delay(&self) -> Option<Duration> {
        self.retry_base_delay
    }
}

/// The interceptors and defaults of one service's client.
#[derive(Debug)]
pub struct Hooks {
    /// In stage order: First, Default, Last, then table order.
    interceptors: Vec<(Scope, &'static dyn Interceptor)>,
    defaults: Defaults,
}

impl Hooks {
    /// Whether an interceptor in scope for `operation` long-polls.
    pub fn long_polls(&self, operation: &str) -> bool {
        self.interceptors(operation).any(|hook| hook.long_polls())
    }
    /// The customizations of `table` whose scope names `service`.
    pub fn for_service(table: &[&'static Customization], service: &str) -> Self {
        let matching: Vec<&Customization> = table
            .iter()
            .copied()
            .filter(|customization| customization.scope.matches_service(service))
            .collect();
        let mut interceptors: Vec<(Scope, &'static dyn Interceptor)> = matching
            .iter()
            .flat_map(|c| {
                c.interceptors
                    .iter()
                    .map(|interceptor| (c.scope, *interceptor))
            })
            .collect();
        // Stable: within an order, the table's.
        interceptors.sort_by_key(|(_, interceptor)| interceptor.order());
        let mut defaults = Defaults::default();
        let service_wide = matching.iter().filter(|c| c.scope.operation.is_none());
        for default in service_wide.flat_map(|c| c.setting_defaults) {
            match *default {
                SettingDefault::MaxAttempts(attempts) => {
                    defaults.max_attempts.get_or_insert(attempts);
                }
                SettingDefault::RetryBaseDelay(delay) => {
                    defaults.retry_base_delay.get_or_insert(delay);
                }
            }
        }
        Self {
            interceptors,
            defaults,
        }
    }
    /// The interceptors of a call of `operation`, in stage order.
    pub fn interceptors<'a>(
        &'a self,
        operation: &'a str,
    ) -> impl Iterator<Item = &'static dyn Interceptor> + 'a {
        self.interceptors
            .iter()
            .filter(move |(scope, _)| scope.operation.is_none_or(|name| name == operation))
            .map(|(_, interceptor)| *interceptor)
    }
    pub const fn defaults(&self) -> &Defaults {
        &self.defaults
    }
}

#[cfg(test)]
mod tests;
