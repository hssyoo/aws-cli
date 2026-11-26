//! Tooling over the model-derived command tables: the tables as JSON for
//! `check-argparse --derived` ([`dump`]), and the eligibility census
//! ([`eligibility`]). Never part of the product.
#![forbid(unsafe_code)]
use awscli_argparse::unpack::{expand_user, expand_vars};
use command::models::ModelSource;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;

pub mod dump;
pub mod eligibility;
pub mod verdicts;

// The census bin and the tests read the CLI's customization table.
use customizations as _;

pub type Error = Box<dyn std::error::Error>;

/// Runs `visit` over the model source the CLI builds: the payload under
/// `AWS_NATIVE_FALLBACK_PREFIX`, the search path of `data_path` (the
/// caller's `AWS_DATA_PATH`, or none) and the home directory `home`, with
/// the CLI's table edits.
pub fn with_source<T>(
    home: OsString,
    data_path: Option<OsString>,
    visit: impl FnOnce(&ModelSource<'_>) -> T,
) -> Result<T, Error> {
    let prefix = std::env::var_os("AWS_NATIVE_FALLBACK_PREFIX")
        .ok_or("AWS_NATIVE_FALLBACK_PREFIX must name the fallback payload prefix")?;
    let bytes = std::fs::read(PathBuf::from(prefix).join("awscli-v2.blob"))?;
    let view = pyload::View::new(&bytes)?;
    let mut env: HashMap<String, OsString> = std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value)))
        .collect();
    env.insert("HOME".to_owned(), home);
    let expand = |path: &[u8]| expand_user(&expand_vars(path, &env)?, &env);
    let edits = service_command::coverage::table_edits();
    let source = ModelSource::from_payload(view, data_path.as_deref(), &expand, &edits)?;
    Ok(visit(&source))
}
