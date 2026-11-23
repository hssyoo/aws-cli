//! `cargo xtask parity [--service NAME]... [--filter TEXT]`: the
//! differential test against botocore.
//!
//! `tests/parity/run.py`, on the managed CPython the product embeds,
//! provisions the harness venv from the in-tree AWS CLI's runtime lock,
//! captures botocore's results for every case under `tests/parity/cases`
//! into `target/parity/vectors` (keyed by every input, so a stale capture
//! is never reused), and names the capture and the venv interpreter. The
//! tests that need that venv are ignored under `xtask test` and run here
//! with `--ignored`; a run that executes none fails.
use crate::{TaskError, deps, process};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    services: Vec<String>,
    filter: Option<String>,
}

fn options(args: &[String]) -> Result<Options, TaskError> {
    let mut options = Options::default();
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let value = || {
            args.clone()
                .next()
                .cloned()
                .ok_or_else(|| deps::error(format!("{flag} requires a value")))
        };
        match flag.as_str() {
            "--service" => options.services.push(value()?),
            "--filter" => options.filter = Some(value()?),
            other => return Err(deps::error(format!("unknown parity option: {other}"))),
        }
        args.next();
    }
    Ok(options)
}

pub fn run(root: &Path, args: &[String]) -> Result<(), TaskError> {
    let options = options(args)?;
    deps::native_host()?;
    let environment = deps::native_environment(root, !cfg!(windows))?;
    // The capture runs, and its venv is built, on the interpreter the
    // product embeds, so version-dependent texts are the embedded CLI's.
    let python = environment.cpython().join(if cfg!(windows) {
        "python.exe"
    } else {
        "bin/python3"
    });
    let capture = capture(root, &python)?;
    let mut command = Command::new("cargo");
    command.current_dir(root).args([
        "test",
        "--locked",
        "--target",
        deps::TARGET,
        "-p",
        "customizations",
        "-p",
        "format",
        "--",
        "--ignored",
    ]);
    command
        .env("PARITY_VECTORS", &capture.vectors)
        .env("PARITY_PYTHON", &capture.python)
        .env("PARITY_SERVICE", options.services.join(","))
        .env("PARITY_FILTER", options.filter.unwrap_or_default())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    environment.apply(&mut command);
    environment.apply_test_tools(&mut command);
    environment.apply_link(&mut command);
    // libtest prints a failure's diff on stdout, which is shown either way.
    let output = process::spawn(&mut command)?.wait_with_output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    std::io::stdout().write_all(&output.stdout)?;
    if !output.status.success() {
        return Err(deps::error(format!(
            "the parity tests failed with {}",
            output.status
        )));
    }
    let ran = passed(&stdout);
    if ran == 0 {
        return Err(deps::error("cargo xtask parity ran no parity test"));
    }
    writeln!(std::io::stdout(), "parity: {ran} tests passed")?;
    Ok(())
}

struct Capture {
    vectors: String,
    python: String,
}

/// Runs the capture and reads where it wrote.
fn capture(root: &Path, python: &Path) -> Result<Capture, TaskError> {
    let mut command = Command::new(python);
    command
        .current_dir(root)
        .arg("-B")
        .arg(root.join("tests/parity/run.py"))
        .arg("--aws-cli-source")
        .arg(root.join("aws-cli"))
        .arg("--cache-dir")
        .arg(root.join("target/parity"))
        .arg("--cases")
        .arg(root.join("tests/parity/cases"))
        .arg("--models")
        .arg(root.join("tests/models"))
        .stderr(Stdio::inherit());
    let output = process::output(&mut command)?;
    let text = String::from_utf8_lossy(&output.stdout);
    let field = |name: &str| {
        let start = format!("\"{name}\": \"");
        let begin = text.find(&start).map(|at| at + start.len())?;
        let end = text[begin..].find('"')?;
        Some(text[begin..begin + end].to_owned())
    };
    match (field("vectors"), field("python")) {
        (Some(vectors), Some(python)) => Ok(Capture { vectors, python }),
        _ => Err(deps::error(format!(
            "the parity capture printed no location: {text}"
        ))),
    }
}

/// The tests libtest reports passed across every test binary.
fn passed(stdout: &str) -> usize {
    stdout
        .lines()
        .filter_map(|line| line.strip_prefix("test result: ok. "))
        .filter_map(|rest| rest.split(' ').next()?.parse::<usize>().ok())
        .sum()
}

#[cfg(test)]
mod tests;
