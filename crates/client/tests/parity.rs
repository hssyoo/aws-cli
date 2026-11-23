//! The differential test against botocore: `cargo xtask parity` captures
//! botocore's results and runs this against them.
use cli_core as _;
use credentials as _;
use crt as _;
use sigv4 as _;
use timestamp as _;

#[path = "parity/tests.rs"]
mod tests;
