//! The census of client F1: every operation of every shipped model through
//! `client::admission::static_refusal`, counted by refusal.
use awscli_argparse as _;
use cli_core as _;
use credentials as _;
use crt as _;
use sigv4 as _;
use timestamp as _;

#[path = "census/tests.rs"]
mod tests;
