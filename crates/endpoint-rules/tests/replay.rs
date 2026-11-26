//! Replays botocore's endpoint test suite
//! (`aws-cli/tests/functional/botocore/endpoint-rules/*/endpoint-tests-1.json`)
//! against each service's `endpoint-rule-set-1.json`: every case's `params`
//! must give the expected url, properties and headers, or the expected
//! error. A case whose expectation the engine (the CRT engine's semantics)
//! does not meet must be listed in [`DISAGREEMENTS`] with why, and must
//! still disagree. The tally per directory is written to
//! `$CARGO_TARGET_TMPDIR/endpoint-rules-replay.txt`.

#[path = "replay/tests.rs"]
mod tests;
