//! Characterization tests: behaviours of today's call loop, serializers,
//! parsers and validator that the refactor must keep, pinned through the
//! frozen façade (`client::testing::facade`, its `hooks` adapters, and
//! models given inline) before the code they describe is rewritten.
use cli_core as _;
use customizations as _;
use format as _;
use pytext as _;

#[path = "characterization/tests.rs"]
mod tests;
