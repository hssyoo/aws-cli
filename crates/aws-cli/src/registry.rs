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
    let variables: HashMap<String, OsString> = env
        .names()
        .into_iter()
        .filter_map(|name| Some((name.to_owned(), env.var_os(name)?.to_owned())))
        .collect();
    let expand =
        |path: &[u8]| unpack::expand_user(&unpack::expand_vars(path, &variables)?, &variables);
    let models = ModelSource::from_payload(view, env.var_os("AWS_DATA_PATH"), &expand).ok()?;
    Some(ServiceSource::new(
        models,
        &customizations::TABLE,
        s3::command::embedded_trust_bundle(payload),
        s3::command::user_agent_versions(),
    ))
}

/// The pinned `s3` grammar, which the router's enum path binds, and the
/// model source, when there is one.
pub fn registry<'s>(source: Option<&'s ServiceSource<'static>>) -> Registry<'s> {
    Registry::new(
        &[&tables::S3],
        source.map(|source| source as &dyn command::OperationSource),
    )
}
