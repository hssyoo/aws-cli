//! The frozen projection digests: every model the CLI loads (the shipped
//! ones, extras merged; the fake models of `tests/models`; each protocol
//! suite case's description) projects to the digest
//! `tests/projection/digests.tsv` holds for it. The digests were taken
//! when the C2J front-end's projection equalled the arena's for each of
//! them, so a reader that keeps them reads every model as the arena did.
//! On a difference the test names each row that differs and writes the
//! rows and the differing projections it found to its target scratch
//! directory; it never writes the golden file.
use {awscli_argparse as _, cli_core as _, client as _, command as _, customizations as _};
use {pyload as _, serde_json as _, service_command as _};

#[path = "projection/tests.rs"]
mod tests;
