//! The model-derived tables, from the payload under
//! `AWS_NATIVE_FALLBACK_PREFIX` and the search path `AWS_DATA_PATH` and the
//! home directory `<dir>` give:
//!
//! - `derive-tables --home <dir> <command>...`: the root table, and the
//!   command and argument tables of each named root, as JSON on stdout
//!   (`cargo xtask check-argparse --derived`);
//! - `derive-tables --home <dir> parse <jsonl>`: each request's parse over
//!   the registry the binary assembles (the pinned `s3` tree and this
//!   source), one result per line as `argparse-tool parse` writes them (the
//!   parse oracle's derived cases);
//! - `derive-tables --home <dir> touching <command>...`: for every operation
//!   of each named root, the awscli entries and botocore handlers that touch
//!   it, one `<command> <operation> <name>` line each.
#![forbid(unsafe_code)]
use awscli_argparse::unpack::{self, expand_user, expand_vars};
use command::models::ModelSource;
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use {
    awscli_config as _, cli_core as _, client as _, credentials as _, format as _, model as _,
    service_errors as _,
};
// The library's test dependencies.
#[cfg(test)]
use {cli_router as _, customizations as _};

type Error = Box<dyn std::error::Error>;

fn main() -> Result<(), Error> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some("--home".as_ref()) {
        return Err("expected --home <dir> (parse <jsonl> | <command>...)".into());
    }
    let home = args.next().ok_or("--home requires a directory")?;
    let rest = args
        .map(|arg| arg.into_string().map_err(|_| "an argument is not Unicode"))
        .collect::<Result<Vec<String>, _>>()?;
    let prefix = std::env::var_os("AWS_NATIVE_FALLBACK_PREFIX")
        .ok_or("AWS_NATIVE_FALLBACK_PREFIX must name the fallback payload prefix")?;
    let bytes = std::fs::read(PathBuf::from(prefix).join("awscli-v2.blob"))?;
    let view = pyload::View::new(&bytes)?;
    let mut env: HashMap<String, OsString> = std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value)))
        .collect();
    env.insert("HOME".to_owned(), home);
    let expand = |path: &[u8]| expand_user(&expand_vars(path, &env)?, &env);
    let data_path = env.get("AWS_DATA_PATH").map(OsString::as_os_str);
    let edits = service_command::coverage::table_edits();
    let source = ModelSource::from_payload(view, data_path, &expand, &edits)?;
    let mut stdout = std::io::BufWriter::new(std::io::stdout().lock());
    match rest.as_slice() {
        [mode, requests] if mode == "parse" => parse(&source, Path::new(requests), &mut stdout)?,
        [mode, commands @ ..] if mode == "touching" => touching(&source, commands, &mut stdout)?,
        commands => {
            let tables = command::models::dump::tables(&source, commands);
            serde_json::to_writer(&mut stdout, &tables)?;
            writeln!(stdout)?;
        }
    }
    stdout.flush()?;
    Ok(())
}

/// The entries that touch each operation of `commands`.
fn touching(
    source: &ModelSource<'_>,
    commands: &[String],
    output: &mut dyn Write,
) -> Result<(), Error> {
    for command in commands {
        let service = source.service(command).ok_or("no such root")?;
        for operation in &service.operations {
            for name in source.touching(command, operation).unwrap_or_default() {
                writeln!(output, "{command} {operation} {name}")?;
            }
        }
    }
    Ok(())
}

/// Parse every request of the JSON-lines file: `{"argv": [...], "color":
/// bool}`, with the chain's value processing for a parse that completes.
fn parse(source: &ModelSource<'_>, requests: &Path, output: &mut dyn Write) -> Result<(), Error> {
    let registry = command::Registry::new(&[&awscli_argparse::tables::S3], Some(source));
    let tables = registry.tables();
    let variables = unpack::process_variables();
    let directory = unpack::current_directory()?;
    let environment = unpack::Environment {
        directory: &directory,
        variables: &variables,
        files: &unpack::FsParamfiles,
        preferred_encoding: unpack::host_preferred_encoding(),
    };
    for line in std::fs::read_to_string(requests)?
        .lines()
        .filter(|line| !line.is_empty())
    {
        let request: serde_json::Value = serde_json::from_str(line)?;
        let argv = request["argv"]
            .as_array()
            .ok_or("argv must be a list")?
            .iter()
            .map(|arg| {
                arg.as_str()
                    .map(str::to_owned)
                    .ok_or("arguments must be strings")
            })
            .collect::<Result<Vec<String>, _>>()?;
        let color = request
            .get("color")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let mut parsed =
            awscli_argparse::parse(&argv, &awscli_argparse::Options::default(), &tables);
        let unsupported = parsed.outcome == awscli_argparse::chain::Outcome::Parsed
            && awscli_argparse::complete(&mut parsed, &environment);
        awscli_argparse::tool::write_result(&parsed, unsupported, color, output)?;
    }
    Ok(())
}
