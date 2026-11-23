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
use crate::endpoint::EndpointParams;
use crate::http::{HttpRequest, HttpResponse};
use crate::retry::{Attempt, RetryDecision};
use model::shapes::{Model, Operation};
use model::value::Value;
use std::any::Any;
use std::time::Duration;

/// Which calls something applies to: a service, by its model directory
/// name (botocore's `service_name`, PLAN §2.1), and an operation, by its
/// model name. `None` matches every one.
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
pub struct HookError(pub String);

impl std::fmt::Display for HookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
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
    /// The rule parameters before resolution (`before-endpoint-resolution`).
    fn endpoint_params(
        &self,
        _cx: &mut CallContext<'_>,
        _params: &mut EndpointParams,
    ) -> HookResult {
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
    /// The output before it is returned (`after-call`).
    fn modify_output(&self, _cx: &mut CallContext<'_>, _output: &mut Value) -> HookResult {
        Ok(())
    }
}

/// A default for a client setting the user left unset (REPORT-m1
/// finding 8): the user's value, whatever it is, wins.
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
    pub interceptors: &'static [&'static dyn Interceptor],
    /// The Python handlers this ports, as `module.function`, the way the
    /// derived pattern entry names them (PLAN §2.2).
    pub replaces: &'static [&'static str],
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
    /// The non-throttling backoff base: the configured one, else a
    /// customization's, else unset.
    pub fn retry_base_delay(&self, configured: Option<Duration>) -> Option<Duration> {
        configured.or(self.retry_base_delay)
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
