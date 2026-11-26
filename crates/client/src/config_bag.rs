//! A call's interceptor state and a client's setting defaults (decision
//! D1): no `TypeId`-layered bag. Core settings stay typed fields of the
//! client; the only call-time store is [`Slots`], and plugins supply
//! defaults through the closed [`SettingDefault`] enum. A layer comes back
//! only when a call-time reader of one exists.
//!
//! The module keeps the `config_bag` name the module root was declared
//! with; renaming it is a `lib.rs` edit for the step that wires it.
use std::any::Any;
use std::time::Duration;

pub use crate::hooks::SettingDefault;

/// The state a call's interceptors share (the SRA's `interceptor_state`,
/// botocore's request `context`): one value per type, made from its
/// default on first use. It travels in the prepared call to the attempts,
/// so it holds only `Send` values.
#[derive(Default)]
pub struct Slots(Vec<Box<dyn Any + Send>>);

impl Slots {
    /// The value of type `T`, made from its default on first use. Hooks
    /// that share state agree on its type.
    #[expect(
        clippy::expect_used,
        reason = "the slot was just found or made with type T"
    )]
    pub fn get<T: Default + Send + 'static>(&mut self) -> &mut T {
        let index = match self.0.iter().position(|slot| slot.is::<T>()) {
            Some(index) => index,
            None => {
                self.0.push(Box::new(T::default()));
                self.0.len() - 1
            }
        };
        self.0[index].downcast_mut::<T>().expect("a slot of type T")
    }
    /// The value of type `T` if a hook has made it, for readers that must
    /// not create state (observers see the context by shared reference).
    pub fn find<T: 'static>(&self) -> Option<&T> {
        self.0.iter().find_map(|slot| slot.downcast_ref::<T>())
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for Slots {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Slots({})", self.0.len())
    }
}

/// The defaults a client's service-wide plugins supply, each from the
/// first that supplies it (`hooks.rs` `Hooks::for_service` today).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SettingDefaults {
    max_attempts: Option<u32>,
    retry_base_delay: Option<Duration>,
}

impl SettingDefaults {
    /// The first value of each kind in `defaults`, in the order given.
    pub fn first_wins<'a>(defaults: impl IntoIterator<Item = &'a SettingDefault>) -> Self {
        let mut resolved = Self::default();
        for default in defaults {
            match *default {
                SettingDefault::MaxAttempts(attempts) => {
                    resolved.max_attempts.get_or_insert(attempts);
                }
                SettingDefault::RetryBaseDelay(delay) => {
                    resolved.retry_base_delay.get_or_insert(delay);
                }
            }
        }
        resolved
    }
    /// `max_attempts`: the user's, whatever it is, else a plugin's, else
    /// unset (`ClientCreator._register_v2_standard_retries`).
    pub fn max_attempts(&self, configured: Option<u32>) -> Option<u32> {
        configured.or(self.max_attempts)
    }
    /// The non-throttling backoff base a plugin supplies, else unset; no
    /// setting configures it.
    pub const fn retry_base_delay(&self) -> Option<Duration> {
        self.retry_base_delay
    }
}

#[cfg(test)]
mod tests;
