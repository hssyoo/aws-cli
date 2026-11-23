//! The command roots, assembled once per invocation: the only place that
//! lists them.
use awscli_argparse::{tables, unpack};
use cli_core::{EnvMap, SnapshotEnv};
use command::Registry;
use command::models::ModelSource;
use service_command::ServiceSource;
use std::collections::HashMap;
use std::ffi::OsString;

/// The model source over the embedded payload and the search path the
/// process's `AWS_DATA_PATH` and home name. A payload without the
/// customization patterns (a system build's), or a search path native
/// cannot expand as Python does, gives none: every model command is then
/// Python's.
pub fn source(env: &SnapshotEnv) -> Option<ServiceSource<'static>> {
    let payload = pyload::embedded();
    let view = pyload::View::new(payload).ok()?;
    let mut variables: HashMap<String, OsString> = env
        .names()
        .into_iter()
        .filter_map(|name| Some((name.to_owned(), env.var_os(name)?.to_owned())))
        .collect();
    // posixpath.expanduser reads the password database when HOME is unset;
    // ntpath's reads USERPROFILE and never HOME.
    if cfg!(unix)
        && !variables.contains_key("HOME")
        && let Some(home) = cli_core::shared_config::home_directory(env)
    {
        variables.insert("HOME".to_owned(), home.into_os_string());
    }
    let expand =
        |path: &[u8]| unpack::expand_user(&unpack::expand_vars(path, &variables)?, &variables);
    let edits = service_command::coverage::table_edits();
    let models =
        ModelSource::from_payload(view, env.var_os("AWS_DATA_PATH"), &expand, &edits).ok()?;
    Some(ServiceSource::new(
        models,
        &customizations::TABLE,
        s3::command::embedded_trust_bundle(payload),
        s3::command::user_agent_versions(),
    ))
}

/// The pinned `s3` grammar, which the router's enum path binds, and the
/// model source's tables and binder, when there is one.
pub fn registry<'s>(source: Option<&'s ServiceSource<'static>>) -> Registry<'s> {
    match source {
        Some(source) => Registry::new(&[&tables::S3], Some(source.models())).with_binder(source),
        None => Registry::new(&[&tables::S3], None).without_models(),
    }
}
