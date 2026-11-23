//! The process's one source of non-cryptographic randomness: retry jitter,
//! the uuids botocore makes with `uuid.uuid4()`, and anything else that
//! needs bits nobody can predict without needing them to be secret.

use std::hash::{BuildHasher, RandomState};

/// 64 random bits: the keys of a fresh `RandomState`, which the standard
/// library draws from the operating system once per thread and then
/// advances for each new state.
pub fn word() -> u64 {
    RandomState::new().hash_one(0)
}

#[cfg(test)]
mod tests;
