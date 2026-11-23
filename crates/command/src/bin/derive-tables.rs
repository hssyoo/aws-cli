//! `derive-tables --home <dir> <command>...`: the derived root table, and the
//! command and argument tables of each named root, as JSON on stdout, from
//! the payload under `AWS_NATIVE_FALLBACK_PREFIX` and the search path
//! `AWS_DATA_PATH` and the home directory `<dir>` give (`cargo xtask
//! check-argparse --derived`).
#![forbid(unsafe_code)]
use awscli_argparse::unpack::{expand_user, expand_vars};
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use {awscli_config as _, cli_core as _, model as _};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some("--home".as_ref()) {
        return Err("expected --home <dir> <command>...".into());
    }
    let home = args.next().ok_or("--home requires a directory")?;
    let commands = args
        .map(|arg| {
            arg.into_string()
                .map_err(|_| "a command name is not Unicode")
        })
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
    let source = command::models::ModelSource::from_payload(view, data_path, &expand)?;
    let tables = command::models::dump::tables(&source, &commands);
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, &tables)?;
    writeln!(stdout)?;
    Ok(())
}
