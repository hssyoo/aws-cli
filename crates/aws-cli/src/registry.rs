//! The command roots, assembled once: the only place that lists them.
use awscli_argparse::tables;
use command::Registry;

/// The pinned `s3` grammar, which the router's enum path binds; no
/// hand-written spec; and no model source yet, so every other command word
/// is no root and its invocation is Python's.
pub const REGISTRY: Registry<'static> = Registry::new(&[&tables::S3], &[], None);
