//! `check-argparse --derived [command...]`: the command tables native
//! derives from the models, against the AWS CLI's own, built the way a
//! command's parse builds them (`tests/argparse/argtables.py --derived`).
use super::*;

/// The roots checked when none is named: the fake models, `sts`, and a
/// sample covering the query, json, rest-json, rest-xml and ec2 protocols, a
/// renamed root and paginators.
pub const DEFAULT_COMMANDS: &[&str] = &[
    "zquery", "zjson", "sts", "sns", "dynamodb", "lambda", "s3api", "ec2",
];

/// Derive the tables of `commands` (the default set when empty) and diff
/// them with Python's.
pub fn check(root: &Path, commands: &[String]) -> Result<(), TaskError> {
    let commands: Vec<String> = if commands.is_empty() {
        DEFAULT_COMMANDS
            .iter()
            .map(|&name| name.to_owned())
            .collect()
    } else {
        commands.to_vec()
    };
    let environment = deps::native_environment(root, false)?;
    process::run(Command::new("cargo").current_dir(root).args([
        "build",
        "--locked",
        "--package",
        "command",
        "--bin",
        "derive-tables",
    ]))?;
    let tool = root
        .join("target/debug/derive-tables")
        .with_extension(std::env::consts::EXE_EXTENSION);
    let directory = root.join("target/argtables/derived");
    let home = directory.join("home");
    fs::create_dir_all(&home)?;
    let mut derive = Command::new(tool);
    derive.arg("--home").arg(&home).args(&commands);
    environment.apply(&mut derive);
    isolate(&mut derive, root);
    let tables = process::output(&mut derive)?;
    let table_file = directory.join("tables.json");
    fs::write(&table_file, tables.stdout)?;
    let source = deps::aws_cli_source(root)?;
    let mut oracle = table_check_command(root, &source, &table_file);
    oracle
        .arg("--home")
        .arg(&home)
        .arg("--derived")
        .args(&commands);
    isolate(&mut oracle, root);
    process::run(&mut oracle)
}

/// Both sides read only the repository's fake models besides the payload's
/// or the source's: no `AWS_*` setting of the caller's, and the empty home
/// each is given (`--home`), so neither reads the caller's config, aliases,
/// plugins or models.
fn isolate(command: &mut Command, root: &Path) {
    for (name, _) in std::env::vars_os() {
        if name
            .to_str()
            .is_some_and(|name| name.starts_with("AWS_") && name != "AWS_NATIVE_FALLBACK_PREFIX")
        {
            command.env_remove(&name);
        }
    }
    command.env("AWS_DATA_PATH", root.join("tests/models"));
}
