//! The frozen façade's signatures, pinned: a façade change that would make
//! a frozen harness edit its calls fails to compile here first.
use cli_core as _;
use customizations as _;
use format as _;
use pytext as _;
use serde_json as _;
use timestamp as _;

#[path = "facade_api/tests.rs"]
mod tests;
