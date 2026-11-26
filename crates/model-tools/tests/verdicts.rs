//! The golden per-operation verdicts: every operation of the shipped
//! models (the payload's, no `AWS_DATA_PATH`, an empty home) has the row
//! `tests/verdicts/verdicts.tsv` holds for it. On a difference the test
//! names each row that differs and writes the rows it found to its target
//! scratch directory; it never writes the golden file. Accepting a verdict
//! change is copying that file over the golden one, with the user's
//! sign-off.
use {awscli_argparse as _, cli_core as _, client as _, command as _, customizations as _};
use {pyload as _, serde_json as _, service_command as _};

#[path = "verdicts/tests.rs"]
mod tests;
