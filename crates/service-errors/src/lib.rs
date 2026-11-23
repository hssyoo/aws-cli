//! A model command's failures as the Python route renders them: what
//! `cli_core::cli_error` prints and exits with for a `client::CallError`,
//! the pager's failure, and an interrupt, in the enhanced and legacy
//! `cli_error_format` (structured formats route to Python).
#![forbid(unsafe_code)]
use cli_core as _;
use client as _;
use format as _;
use model as _;
