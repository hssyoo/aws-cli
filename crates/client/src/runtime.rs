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

/// The process's clock, randomness ([`cli_core::random`]) and
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
    /// `uuid.uuid4()`: 122 random bits from the operating system's source
    /// ([`cli_core::random::os_bytes`]), as Python's reads `os.urandom`, in
    /// the RFC 4122 version 4 layout. Should that source fail, which macOS's
    /// never does and Linux's does only when the kernel refuses
    /// `getrandom`, the bits come from [`cli_core::random::word`] instead,
    /// which is not cryptographic but still differs across processes.
    fn uuid4(&self) -> String {
        let mut bytes = cli_core::random::os_bytes::<16>().unwrap_or_else(|_| {
            let mut bytes = [0; 16];
            bytes[..8].copy_from_slice(&cli_core::random::word().to_be_bytes());
            bytes[8..].copy_from_slice(&cli_core::random::word().to_be_bytes());
            bytes
        });
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        format!(
            "{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
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
