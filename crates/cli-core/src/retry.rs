//! The parts of botocore's standard retry mode that every client shares:
//! the backoff's ceiling and draw, and the retry quota
//! (`botocore/retries/standard.py`, `quota.py`). The S3 client's retries
//! and the model client's both build on them.
use std::time::Duration;

/// The longest backoff (`ExponentialBackoff._MAX_BACKOFF`).
pub const BACKOFF_MAX: Duration = Duration::from_secs(20);

/// Whether a retried error is throttling, which sets the backoff's base
/// and the quota's cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    Throttling,
    Transient,
}

impl ErrorKind {
    /// The backoff's base (`ExponentialBackoff._DEFAULT_BACKOFF_CONFIG`):
    /// one second after throttling, 50 milliseconds after any other error.
    pub const fn base(self) -> Duration {
        match self {
            Self::Throttling => Duration::from_secs(1),
            Self::Transient => Duration::from_millis(50),
        }
    }
}

/// `min(base * 2 ** (attempt - 1), _MAX_BACKOFF)`: the most a retry after
/// the 1-based `attempt` waits (`ExponentialBackoff.delay_amount`).
pub fn backoff_ceiling(attempt: u64, base: Duration) -> Duration {
    let doublings = u32::try_from(attempt.saturating_sub(1))
        .unwrap_or(u32::MAX)
        .min(31);
    base.checked_mul(1 << doublings)
        .map_or(BACKOFF_MAX, |ceiling| ceiling.min(BACKOFF_MAX))
}

/// A delay drawn uniformly below [`backoff_ceiling`], from 64-bit random
/// words: `rand(0, 1) * ceiling` to the nanosecond.
pub fn backoff_delay(random: &mut dyn FnMut() -> u64, attempt: u64, base: Duration) -> Duration {
    let ceiling = u64::try_from(backoff_ceiling(attempt, base).as_nanos()).unwrap_or(u64::MAX);
    if ceiling == 0 {
        return Duration::ZERO;
    }
    Duration::from_nanos(uint_less_than(random, ceiling))
}

/// A uniform integer below `less_than`, by Lemire's multiply-and-reject.
fn uint_less_than(random: &mut dyn FnMut() -> u64, less_than: u64) -> u64 {
    let biased = less_than.wrapping_neg() % less_than;
    loop {
        let product = u128::from(random()) * u128::from(less_than);
        // The low half decides rejection; the high half is the draw.
        if (product as u64) >= biased {
            return (product >> 64) as u64;
        }
    }
}

/// botocore's `RetryQuota` with `RetryQuotaChecker`'s costs, one per client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quota {
    pub available: u32,
}

impl Quota {
    /// The starting and largest capacity (quota.py `INITIAL_CAPACITY`).
    pub const CAPACITY_MAX: u32 = 500;
    /// What a retry takes (standard.py `_RETRY_COST`,
    /// `_THROTTLING_RETRY_COST`).
    pub const RETRY_COST: u32 = 14;
    pub const THROTTLING_RETRY_COST: u32 = 5;
    /// What a call that never retried gives back (`_NO_RETRY_INCREMENT`).
    pub const NO_RETRY_INCREMENT: u32 = 1;
    /// Takes a retry's cost (`acquire_retry_quota`): the cost taken, or
    /// `None`, taking nothing, when too little is left.
    pub fn acquire(&mut self, kind: ErrorKind) -> Option<u32> {
        let cost = match kind {
            ErrorKind::Throttling => Self::THROTTLING_RETRY_COST,
            ErrorKind::Transient => Self::RETRY_COST,
        };
        self.available = self.available.checked_sub(cost)?;
        Some(cost)
    }
    /// Gives back what a successful call earns (`release_retry_quota`): the
    /// last retry's cost, or [`Self::NO_RETRY_INCREMENT`] without a retry.
    pub fn release(&mut self, last_retry_cost: Option<u32>) {
        let amount = last_retry_cost.unwrap_or(Self::NO_RETRY_INCREMENT);
        self.available = (self.available + amount).min(Self::CAPACITY_MAX);
    }
}

impl Default for Quota {
    fn default() -> Self {
        Self {
            available: Self::CAPACITY_MAX,
        }
    }
}

#[cfg(test)]
mod tests;
