//! The fake-model matrix's completeness check (DEV-LOOP §14): every
//! feature a shipped model uses is exercised by a fake under `tests/models`,
//! or is listed as an open gap in `fake_matrix/gaps.txt`, which only
//! shrinks. It reads the models itself and imports nothing from `model`,
//! so no later change to the crate's modules reaches it.
use awscli_argparse as _;
use model as _;
use pyload as _;
use pytext as _;
use timestamp as _;

#[path = "fake_matrix/census.rs"]
mod census;
#[path = "fake_matrix/forms.rs"]
mod forms;
#[path = "fake_matrix/matrix.rs"]
mod matrix;
#[path = "fake_matrix/observe.rs"]
mod observe;
#[path = "fake_matrix/tests.rs"]
mod tests;
#[path = "fake_matrix/walk.rs"]
mod walk;
