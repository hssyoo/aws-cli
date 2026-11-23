//! The built payload's models, with the repository's fake models on
//! `AWS_DATA_PATH`, and parses over a registry that holds them.
#![expect(
    clippy::panic,
    reason = "a test without its payload or fixtures must fail"
)]
use awscli_argparse::{Options, ParseResult, unpack};
use client::hooks::Customization;
use command::Registry;
use command::models::ModelSource;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The fallback payload the product embeds, as `cargo xtask` provides it.
pub fn payload() -> Result<Vec<u8>> {
    let prefix = std::env::var_os("AWS_NATIVE_FALLBACK_PREFIX")
        .ok_or("AWS_NATIVE_FALLBACK_PREFIX must name the fallback payload prefix")?;
    Ok(std::fs::read(PathBuf::from(prefix).join("awscli-v2.blob"))?)
}

fn fake_models() -> OsString {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/models")
        .into_os_string()
}

fn expand(path: &[u8]) -> Option<Vec<u8>> {
    let env = HashMap::from([("HOME".to_owned(), OsString::from("/nonexistent"))]);
    let vars = unpack::expand_vars(path, &env)?;
    unpack::expand_user(&vars, &env)
}

/// `check` of the model source over the payload and the fake models.
pub fn with_models<T>(check: impl FnOnce(ModelSource<'_>) -> T) -> Result<T> {
    let bytes = payload()?;
    let view = pyload::View::new(&bytes)?;
    let data_path = fake_models();
    let source = ModelSource::from_payload(view, Some(&data_path), &expand)?;
    Ok(check(source))
}

/// `argv` parsed and unpacked over a registry holding `models`, as the
/// router completes a model operation's parse.
pub fn parsed<'s>(source: &'s crate::ServiceSource<'s>, argv: &[&str]) -> ParseResult<'s> {
    let registry: &'s Registry<'s> = Box::leak(Box::new(Registry::new(&[], &[], Some(source))));
    let argv: Vec<String> = argv.iter().map(|arg| (*arg).to_owned()).collect();
    let mut result = awscli_argparse::parse(&argv, &Options::default(), &registry.tables());
    let variables = HashMap::new();
    let completed = awscli_argparse::complete(
        &mut result,
        &unpack::Environment {
            directory: std::path::Path::new(env!("CARGO_MANIFEST_DIR")),
            variables: &variables,
            files: &unpack::FsParamfiles,
            preferred_encoding: unpack::PreferredEncoding::Locale,
        },
    );
    if completed || result.outcome != awscli_argparse::chain::Outcome::Parsed {
        panic!("{argv:?} did not complete: {:?}", result.outcome);
    }
    result
}

/// A customization in every scope that replaces every botocore handler
/// the matcher finds for `[command, operation]`: the client's side of
/// coverage, settled, so a test sees only the CLI's.
pub fn replacing_all(
    models: &ModelSource<'_>,
    command: &str,
    operation: &str,
) -> &'static [&'static Customization] {
    let names: Vec<&'static str> = models
        .matches(command, operation)
        .unwrap_or_else(|| panic!("{command} {operation}"))
        .into_iter()
        .filter(|found| found.kind != command::patterns::MatchKind::Registry)
        .map(|found| &*Box::leak(found.name.into_boxed_str()))
        .collect();
    let customization = Box::leak(Box::new(Customization {
        id: "test",
        scope: client::hooks::Scope::ALL,
        setting_defaults: &[],
        interceptors: &[],
        replaces: Box::leak(names.into_boxed_slice()),
    }));
    Box::leak(Box::new([&*customization]))
}
