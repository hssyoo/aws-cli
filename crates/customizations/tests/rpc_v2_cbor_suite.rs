//! botocore's protocol suites for the smithy-rpc-v2-cbor protocol, replayed
//! as `aws-cli/tests/unit/botocore/test_protocols.py` replays them: the
//! three files (`smithy-rpc-v2-cbor`, its `-query-compatible` and
//! `-non-query-compatible` variants), each case's own model, the same
//! comparisons, and the ignore list, read from
//! `protocol-tests-ignore-list.json` when the test runs. A request body
//! compares as `_assert_requests_equal` compares a CBOR one: both decoded,
//! the special floats at the top as text, then Python's `==`.
//!
//! botocore's CBOR decode vectors (`tests/unit/botocore/cbor`) replay as
//! `test_cbor_decoding.py` does, with its ignore list.
//!
//! `rpc_v2_cbor_suite/fakes.json` holds what botocore's own
//! `RpcV2CBORSerializer` and `RpcV2CBORParser` give over the zcbor and
//! zcborqc fake models (`tests/models`), written by `fakes.py` beside it:
//! the diff corpus's rows on them and edge inputs and replies. Those
//! compare exactly: the request byte for byte, its headers in order, and
//! the parsed output, service error or error text with keys in order.
use cli_core as _;
use customizations as _;
use format as _;
use pytext as _;
use timestamp as _;

// The tests live beside the crate file, as the crate's other suites keep
// theirs (`rest_xml_suite/tests.rs`), with their data (`fakes.json`).
#[path = "rpc_v2_cbor_suite/tests.rs"]
mod tests;
