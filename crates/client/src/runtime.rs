//! What a call takes from the process, injected so that tests and the
//! byte comparison against botocore can pin it: the clock, uuids, sleep and
//! the random draw for retry jitter.
use cli_core::interrupt::State;
use std::time::Duration;
use timestamp::Timestamp;

/// A sleep the user interrupted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interrupted;

pub trait Runtime: Send + Sync + std::fmt::Debug {
    /// The current instant (`datetime.datetime.utcnow()`), which signing
    /// and idempotency read.
    fn now(&self) -> Timestamp;
    /// `str(uuid.uuid4())`: `amz-sdk-invocation-id` and idempotency tokens.
    fn uuid4(&self) -> String;
    /// Waits `duration` (a retry's backoff, `time.sleep`), or ends early
    /// when the user interrupts.
    ///
    /// # Errors
    /// [`Interrupted`], at once, for an interrupt requested before or
    /// during the wait.
    fn sleep(&self, duration: Duration) -> Result<(), Interrupted>;
    /// `random.random()`: a float in [0, 1), the backoff's jitter.
    fn random(&self) -> f64;
}

/// The process's clock, randomness ([`cli_core::random::word`]) and
/// interruptible sleep ([`State::sleep`]).
#[derive(Clone, Copy, Debug)]
pub struct SystemRuntime {
    interrupt: &'static State,
}

impl SystemRuntime {
    /// A runtime whose sleeps end when `interrupt` is requested, the
    /// process's own (`cli_core::interrupt::process_state`) for a command.
    pub const fn new(interrupt: &'static State) -> Self {
        Self { interrupt }
    }
}

impl Runtime for SystemRuntime {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
    /// 122 random bits in the RFC 4122 version 4 layout.
    fn uuid4(&self) -> String {
        let (high, low) = (cli_core::random::word(), cli_core::random::word());
        format!(
            "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
            high >> 32,
            (high >> 16) & 0xffff,
            high & 0x0fff,
            ((low >> 48) & 0x3fff) | 0x8000,
            low & 0xffff_ffff_ffff,
        )
    }
    fn sleep(&self, duration: Duration) -> Result<(), Interrupted> {
        self.interrupt.sleep(duration).map_err(|_| Interrupted)
    }
    /// The top 53 bits of a random word, scaled to [0, 1), as Python's
    /// `random()` builds its float.
    fn random(&self) -> f64 {
        const SCALE: f64 = 1.0 / (1u64 << 53) as f64;
        (cli_core::random::word() >> 11) as f64 * SCALE
    }
}

#[cfg(test)]
mod tests;
