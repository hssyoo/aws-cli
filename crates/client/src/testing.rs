//! Deterministic stand-ins for the network and the process, for tests of
//! this crate and of those built on it (feature `testing`).
use crate::http::{HttpRequest, HttpResponse};
use crate::runtime::{Interrupted, Runtime};
use crate::transport::{Timeouts, Transport, TransportError};
use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use timestamp::Timestamp;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Answers each request with the next scripted result, in order, and
/// records every request with its timeouts. A request beyond the script
/// fails as `TransportError::Client`.
#[derive(Debug, Default)]
pub struct InMemoryTransport {
    script: Mutex<VecDeque<Result<HttpResponse, TransportError>>>,
    sent: Mutex<Vec<(HttpRequest, Timeouts)>>,
}

impl InMemoryTransport {
    pub fn new() -> Self {
        Self::default()
    }
    /// Answers a later request with `response`.
    pub fn respond(&self, response: HttpResponse) {
        lock(&self.script).push_back(Ok(response));
    }
    /// Fails a later request with `error`.
    pub fn fail(&self, error: TransportError) {
        lock(&self.script).push_back(Err(error));
    }
    /// Every request sent so far, with its timeouts.
    pub fn sent(&self) -> Vec<(HttpRequest, Timeouts)> {
        lock(&self.sent).clone()
    }
}

impl Transport for InMemoryTransport {
    fn send(
        &self,
        request: &HttpRequest,
        timeouts: Timeouts,
    ) -> Result<HttpResponse, TransportError> {
        lock(&self.sent).push((request.clone(), timeouts));
        lock(&self.script).pop_front().unwrap_or_else(|| {
            Err(TransportError::Client {
                reason: "the in-memory transport has no response left".to_owned(),
            })
        })
    }
}

/// A runtime that never waits: a fixed clock, uuids numbered from 1, one
/// fixed draw, and sleeps that are recorded instead of slept.
#[derive(Debug)]
pub struct FixedRuntime {
    now: Timestamp,
    random: f64,
    issued: Mutex<u64>,
    slept: Mutex<Vec<Duration>>,
}

impl FixedRuntime {
    pub fn new(now: Timestamp, random: f64) -> Self {
        Self {
            now,
            random,
            issued: Mutex::new(0),
            slept: Mutex::new(Vec::new()),
        }
    }
    /// Every sleep asked for so far.
    pub fn slept(&self) -> Vec<Duration> {
        lock(&self.slept).clone()
    }
}

impl Runtime for FixedRuntime {
    fn now(&self) -> Timestamp {
        self.now
    }
    fn uuid4(&self) -> String {
        let mut issued = lock(&self.issued);
        *issued += 1;
        format!("00000000-0000-4000-8000-{:012x}", *issued)
    }
    fn sleep(&self, duration: Duration) -> Result<(), Interrupted> {
        lock(&self.slept).push(duration);
        Ok(())
    }
    fn random(&self) -> f64 {
        self.random
    }
}
