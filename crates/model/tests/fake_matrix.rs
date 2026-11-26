//! The fake-model matrix's completeness check (DEV-LOOP §14): every
//! feature a shipped model uses is exercised by a fake under `tests/models`,
//! or is listed as an open gap in `fake_matrix/gaps.txt`, which only
//! shrinks.
use awscli_argparse as _;
use pyload as _;
use pytext as _;
use timestamp as _;

#[path = "fake_matrix/census.rs"]
mod census;
#[path = "fake_matrix/matrix.rs"]
mod matrix;
#[path = "fake_matrix/tests.rs"]
mod tests;
