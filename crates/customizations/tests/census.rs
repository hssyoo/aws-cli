//! The admission census: every operation of every shipped model through
//! `client::admission::static_refusal` with this crate's table, counted by
//! refusal.
use cli_core as _;
use format as _;
use pytext as _;
use timestamp as _;

#[path = "census/tests.rs"]
mod tests;
