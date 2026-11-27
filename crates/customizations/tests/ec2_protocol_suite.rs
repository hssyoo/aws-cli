//! botocore's protocol suite for the ec2 protocol, which admission does
//! not yet route natively: replayed through the façade's switched-off
//! registry.
use cli_core as _;
use customizations as _;
use format as _;
use pytext as _;
use timestamp as _;

#[path = "ec2_protocol_suite/tests.rs"]
mod tests;
