//! botocore's protocol suite for the rest-xml protocol, replayed as
//! `aws-cli/tests/unit/botocore/test_protocols.py` replays it, through the
//! façade's handle to the protocols built but switched off (rest-xml is
//! not admitted): the same
//! per-case model, the same comparisons, and the same ignore list, read
//! from `protocol-tests-ignore-list.json` when the test runs.
//!
//! Request bodies compare as `_assert_xml_bodies` compares them,
//! `ElementTree.canonicalize(strip_text=True)` of each, here `canonical`:
//! both parsed, each element's expanded tag, its attributes sorted, its
//! text stripped, its children in order, and the namespace declarations
//! of the whole document by prefix and URI; a body either side cannot
//! parse compares as bytes.
//!
//! `rest_xml_suite/fakes.json` holds what botocore's own `RestXMLSerializer`
//! and `RestXMLParser` give over the zrestxml fake model
//! (`tests/models/zrestxml`): for the diff corpus's zrestxml rows (whose
//! inputs reproduce the Python route's captured body and target) and for
//! edge inputs and replies around them. Those compare exactly: the request
//! byte for byte, its headers in order, and the parsed output or service
//! error with its keys in order.
use cli_core as _;
use customizations as _;
use format as _;
use pytext as _;
use timestamp as _;

// The tests live beside the crate file, as the crate's other suites keep
// theirs (`protocol_suite/tests.rs`), with their data (`fakes.json`).
#[path = "rest_xml_suite/tests.rs"]
mod tests;
