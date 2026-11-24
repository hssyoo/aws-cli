//! `check-argparse --derived [command...]`: the command tables native
//! derives from the models, against the AWS CLI's own, built the way a
//! command's parse builds them (`tests/argparse/argtables.py --derived`).
use super::*;

/// The roots checked when none is named: the fake models, `sts`, and a
/// sample covering the query, json, rest-json, rest-xml and ec2 protocols, a
/// renamed root, paginators on each implemented protocol (lambda's and
/// zrestjson's on rest-json) and the table editors' removals and `wizard`
/// (lambda's table).
pub const DEFAULT_COMMANDS: &[&str] = &[
    "zquery",
    "zjson",
    "zrestjson",
    "sts",
    "sns",
    "dynamodb",
    "lambda",
    "s3api",
    "ec2",
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
    let tool = build_tool(root, &environment)?;
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

/// `derive-tables [--] <args>`: the tool as the derived check runs it, over
/// the payload and the fake models, with an empty home, its output on
/// stdout. Its own `--home` is supplied.
pub fn run_tool(root: &Path, args: &[String]) -> Result<(), TaskError> {
    let args = args.strip_prefix(&["--".to_owned()]).unwrap_or(args);
    let environment = deps::native_environment(root, false)?;
    let tool = build_tool(root, &environment)?;
    let home = root.join("target/argtables/derived/home");
    fs::create_dir_all(&home)?;
    let mut derive = Command::new(tool);
    derive.arg("--home").arg(&home).args(args);
    environment.apply(&mut derive);
    isolate(&mut derive, root);
    process::run(&mut derive)
}

/// Build `derive-tables`, which derives the tables and parses over them.
pub fn build_tool(
    root: &Path,
    environment: &deps::NativeEnvironment,
) -> Result<PathBuf, TaskError> {
    build_bin(root, environment, "derive-tables")
}

/// Build one of `model-tools`' binaries, a target the product build never
/// compiles, with the native dependencies its crates' client links.
fn build_bin(
    root: &Path,
    environment: &deps::NativeEnvironment,
    bin: &str,
) -> Result<PathBuf, TaskError> {
    let mut build = Command::new("cargo");
    build.current_dir(root).args([
        "build",
        "--locked",
        "--package",
        "model-tools",
        "--bin",
        bin,
    ]);
    environment.apply(&mut build);
    process::run(&mut build)?;
    Ok(product::target_dir(root)
        .join("debug")
        .join(bin)
        .with_extension(std::env::consts::EXE_EXTENSION))
}

/// `cargo xtask census`: the eligibility census over the shipped models
/// alone (no `AWS_DATA_PATH`, an empty home), printed and written to
/// `target/service-eligibility.txt`.
pub fn census(root: &Path) -> Result<(), TaskError> {
    deps::native_host()?;
    let environment = deps::native_environment(root, false)?;
    let tool = build_bin(root, &environment, "eligibility")?;
    let home = root.join("target/census/home");
    fs::create_dir_all(&home)?;
    let mut census = Command::new(tool);
    census.arg("--home").arg(&home);
    environment.apply(&mut census);
    isolate(&mut census, root);
    census.env_remove("AWS_DATA_PATH");
    let output = process::output(&mut census)?;
    fs::write(root.join("target/service-eligibility.txt"), &output.stdout)?;
    io::stdout().write_all(&output.stdout)?;
    Ok(())
}

/// The parse oracle's derived cases (`parse_oracle.py --derived-parser`):
/// `derive-tables parse` over the fake models and the payload, which only
/// that tool reads.
pub fn oracle_arguments(root: &Path, oracle: &mut Command) -> Result<(), TaskError> {
    let environment = deps::native_environment(root, false)?;
    oracle
        .arg("--derived-parser")
        .arg(build_tool(root, &environment)?)
        .arg("--models")
        .arg(root.join("tests/models"))
        .arg("--fallback-prefix")
        .arg(environment.fallback());
    Ok(())
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
