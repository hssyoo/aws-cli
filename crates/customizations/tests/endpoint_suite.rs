//! botocore's endpoint rule-set tests, every case through the client's
//! endpoint path, with the cases where it differs from the expectation
//! recorded as the baseline.
use cli_core as _;
use customizations as _;
use format as _;
use pytext as _;
use timestamp as _;

#[path = "endpoint_suite/tests.rs"]
mod tests;
