//! `eligibility --home <dir>`: the eligibility census over the shipped
//! models alone (the payload's, no `AWS_DATA_PATH`; `<dir>` should hold no
//! `.aws/models`), printed as `cargo xtask census` reports it.
#![forbid(unsafe_code)]
use std::io::Write;
use {awscli_argparse as _, cli_core as _, client as _, command as _};
use {pyload as _, serde_json as _, service_command as _};

fn main() -> Result<(), model_tools::Error> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some("--home".as_ref()) {
        return Err("expected --home <dir>".into());
    }
    let home = args.next().ok_or("--home requires a directory")?;
    let table = &customizations::TABLE[..];
    let counts = model_tools::with_source(home, None, |source| {
        model_tools::eligibility::census(source, table)
    })?;
    std::io::stdout().write_all(model_tools::eligibility::report(&counts).as_bytes())?;
    Ok(())
}
