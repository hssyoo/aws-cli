//! The S3 client's parts of botocore's standard retries; the shared ones
//! are [`cli_core::retry`]'s.
use super::{INVALID_RESPONSE_STATUS_ERROR_CODE, TransferResult};
pub(crate) use cli_core::retry::{ErrorKind, Quota, backoff_delay};

pub(crate) fn is_unmodeled_throttle(failure: &TransferResult, modeled: &[&[u8]]) -> bool {
    failure.error_code == INVALID_RESPONSE_STATUS_ERROR_CODE
        && failure.response_status == 429
        && !modeled.contains(&failure.service_error.code.as_bytes())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Attempts {
    pub count: u64,
    pub last_retry_cost: Option<u32>,
    pub redirected: bool,
}
impl Default for Attempts {
    fn default() -> Self {
        Self {
            count: 1,
            last_retry_cost: None,
            redirected: false,
        }
    }
}
impl Attempts {
    pub(crate) fn exhausted(self, limit: u64) -> Option<u64> {
        (self.count >= limit).then(|| self.count - 1)
    }
    pub(crate) fn admit_retry(&mut self, limit: u64, quota: &mut Quota, kind: ErrorKind) -> bool {
        if self.exhausted(limit).is_some() {
            return false;
        }
        let Some(cost) = quota.acquire(kind) else {
            return false;
        };
        self.last_retry_cost = Some(cost);
        true
    }
}

#[cfg(test)]
mod tests;
