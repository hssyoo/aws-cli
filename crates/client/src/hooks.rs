//! The vocabulary customizations are declared in: the [`Scope`] they
//! apply to, the [`Order`] band a hook runs in, the [`HookError`] a hook
//! ends a call with, the [`SettingDefault`]s they supply, and the
//! [`Basis`] on which each stands in for a Python handler.
//!
//! The unit of registration is [`Customization`]
//! ([`crate::runtime_plugin`]), its hooks [`crate::interceptors::Intercept`]
//! and its retry classifiers [`crate::runtime_components::ClassifyRetry`];
//! a client picks those of its service once, when it is built
//! ([`crate::runtime_plugin::ServicePlugins::for_service`]).
use std::time::Duration;

pub use crate::runtime_plugin::Customization;

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

/// Where a hook runs among those of its position: botocore registers
/// handlers first, in the middle or last (`register_first`,
/// `register_last`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Order {
    First,
    Default,
    Last,
}

/// A hook's failure, which ends the call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookError {
    /// Leave the call to the Python route, for the reason named: a partial
    /// port declines what it does not reproduce. Only the hooks
    /// [`crate::call::Client::prepare`] runs may refuse
    /// (`modify_before_serialization`, `modify_endpoint_builtins`,
    /// `modify_before_retry_loop`), which happen before any credential
    /// provider runs or anything is sent; there it becomes
    /// [`crate::admission::Unsupported::Hook`]. A later hook's refusal is
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
    /// The error a hook after `prepare` reports: a refusal there comes too
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

#[cfg(test)]
mod tests;
