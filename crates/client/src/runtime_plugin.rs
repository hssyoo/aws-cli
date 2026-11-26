//! Plugins as static data (decision D3): a [`Customization`] declares its
//! scope, setting defaults, admission metadata, hooks, retry classifiers
//! and long polling; there is no `runtime_components` callback and no
//! per-operation override set. Operation scope stays a filter applied per
//! call, so whatever is declared is known before any call starts.
use crate::config_bag::{SettingDefault, SettingDefaults};
use crate::interceptors::Intercept;
use crate::runtime_components::ClassifyRetry;

pub use crate::hooks::{Basis, Scope};

/// The unit of registration, for built-ins and later plugins alike.
#[derive(Debug)]
pub struct Customization {
    /// A name for diagnostics.
    pub id: &'static str,
    pub scope: Scope,
    /// Defaults a client of the scope's service takes; only a
    /// customization for a whole service supplies them.
    pub setting_defaults: &'static [SettingDefault],
    /// The endpoint built-ins its hooks set, by `builtIn` name, so
    /// admission knows before any call that a rule set's required built-in
    /// is supplied.
    pub builtins: &'static [&'static str],
    pub interceptors: &'static [&'static dyn Intercept],
    /// Classifiers that run before the standard one (`needs-retry`).
    pub retry_classifiers: &'static [&'static dyn ClassifyRetry],
    /// Whether the scope's calls long-poll: once the retry quota refuses a
    /// retry, the call still sleeps the delay a retry would have waited,
    /// then stops (`RetryHandler._is_long_polling_operation`).
    pub long_polling: bool,
    /// The Python handlers this stands in for, as `module.qualname`, each
    /// with why native may.
    pub replaces: &'static [(&'static str, Basis)],
}

/// Where a customization was registered from. Built-ins come first
/// within each [`crate::interceptors::Order`] band, so no plugin runs
/// before a built-in of its band (decision D6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Origin {
    BuiltIn,
    Configured,
}

/// What a client's customizations add, chosen once for its service.
#[derive(Debug)]
pub struct ServicePlugins {
    /// By band, then origin, then registration order.
    interceptors: Vec<(Scope, &'static dyn Intercept)>,
    /// Built-ins, then configured, each in registration order.
    retry_classifiers: Vec<(Scope, &'static dyn ClassifyRetry)>,
    long_polling: Vec<Scope>,
    defaults: SettingDefaults,
}

impl ServicePlugins {
    /// The customizations of `built_ins`, then of `configured`, whose scope
    /// names `service` (the model directory).
    pub fn for_service(
        built_ins: &[&'static Customization],
        configured: &[&'static Customization],
        service: &str,
    ) -> Self {
        let matching: Vec<(Origin, &'static Customization)> = built_ins
            .iter()
            .map(|c| (Origin::BuiltIn, *c))
            .chain(configured.iter().map(|c| (Origin::Configured, *c)))
            .filter(|(_, c)| c.scope.matches_service(service))
            .collect();
        let mut ordered: Vec<(Origin, Scope, &'static dyn Intercept)> = matching
            .iter()
            .flat_map(|(origin, c)| c.interceptors.iter().map(|i| (*origin, c.scope, *i)))
            .collect();
        // Stable: within a band and origin, registration order.
        ordered.sort_by_key(|(origin, _, interceptor)| (interceptor.order(), *origin));
        let interceptors = ordered.into_iter().map(|(_, s, i)| (s, i)).collect();
        let retry_classifiers = matching
            .iter()
            .flat_map(|(_, c)| c.retry_classifiers.iter().map(|r| (c.scope, *r)))
            .collect();
        let long_polling = matching
            .iter()
            .filter(|(_, c)| c.long_polling)
            .map(|(_, c)| c.scope)
            .collect();
        let service_wide = matching.iter().filter(|(_, c)| c.scope.operation.is_none());
        let defaults =
            SettingDefaults::first_wins(service_wide.flat_map(|(_, c)| c.setting_defaults));
        Self {
            interceptors,
            retry_classifiers,
            long_polling,
            defaults,
        }
    }
    /// The interceptors of a call of `operation`, in run order.
    pub fn interceptors<'a>(
        &'a self,
        operation: &'a str,
    ) -> impl Iterator<Item = &'static dyn Intercept> + 'a {
        in_scope(&self.interceptors, operation)
    }
    /// The retry classifiers of a call of `operation`, in run order.
    pub fn retry_classifiers<'a>(
        &'a self,
        operation: &'a str,
    ) -> impl Iterator<Item = &'static dyn ClassifyRetry> + 'a {
        in_scope(&self.retry_classifiers, operation)
    }
    /// Whether a customization in scope for `operation` long-polls.
    pub fn long_polls(&self, operation: &str) -> bool {
        self.long_polling
            .iter()
            .any(|scope| scope.operation.is_none_or(|name| name == operation))
    }
    pub const fn defaults(&self) -> &SettingDefaults {
        &self.defaults
    }
}

fn in_scope<'a, T: ?Sized>(
    entries: &'a [(Scope, &'static T)],
    operation: &'a str,
) -> impl Iterator<Item = &'static T> + 'a {
    entries
        .iter()
        .filter(move |(scope, _)| scope.operation.is_none_or(|name| name == operation))
        .map(|(_, item)| *item)
}

#[cfg(test)]
mod tests;
