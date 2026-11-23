//! What the binary harnesses share, and the harness self-tests.
//!
//! `blackbox` and `integ` drive one binary, which [`binary`] selects, and
//! the blackbox runner's JUnit reports pass only as [`verify_sessions`]
//! reads them. `cargo xtask harness-test` ([`self_test`]) runs the
//! dependency drivers' Python self-tests, `tests/deps`.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::product::options::DEFAULT_PROFILE;
use crate::{TaskError, deps, process, product};

/// The binary a harness drives: `--aws-binary PATH`, or the static
/// default-profile artifact. Only that default names `deps::TARGET`, so only
/// it takes the static host list; an explicit binary, such as a system
/// build's, runs on any host.
pub(crate) fn binary(root: &Path, args: &[String]) -> Result<PathBuf, TaskError> {
    let default = Default {
        target: static_target,
        build: "cargo xtask build",
    };
    select_binary(root, &product::target_dir(root), args, &default)
}

/// `binary` for a system build, whose default artifact lies under the
/// native glibc triple `product::system_target` names after its host check.
pub(crate) fn system_binary(root: &Path, args: &[String]) -> Result<PathBuf, TaskError> {
    let default = Default {
        target: product::system_target,
        build: "cargo xtask build --link-mode system, with the build's other options",
    };
    select_binary(root, &product::target_dir(root), args, &default)
}

fn static_target() -> Result<String, TaskError> {
    deps::native_host()?;
    Ok(deps::TARGET.to_owned())
}

/// A harness's default artifact: its target triple, which runs that mode's
/// host check, and the command that builds it.
struct Default {
    target: fn() -> Result<String, TaskError>,
    build: &'static str,
}

/// The binary `args` select. The default artifact lies under `target_dir`,
/// and must be present and current ([`require_current`]); an explicit
/// binary is the caller's, and never consults the default's target.
fn select_binary(
    root: &Path,
    target_dir: &Path,
    args: &[String],
    default: &Default,
) -> Result<PathBuf, TaskError> {
    match args {
        [] => {
            let path = target_dir
                .join((default.target)()?)
                .join(DEFAULT_PROFILE)
                .join("aws")
                .with_extension(std::env::consts::EXE_EXTENSION);
            if !path.is_file() {
                return Err(deps::error(format!(
                    "Rust CLI is absent: {}; run `{}` or supply --aws-binary",
                    path.display(),
                    default.build
                )));
            }
            require_current(root, &path, default.build)?;
            Ok(path)
        }
        [flag, value] if flag == "--aws-binary" => {
            let path = root.join(value);
            if !path.is_file() {
                return Err(deps::error(format!(
                    "Rust CLI is absent: {}",
                    path.display()
                )));
            }
            Ok(path)
        }
        _ => Err(deps::error(
            "expected only --aws-binary PATH; every case must run",
        )),
    }
}

/// Fails unless `binary` is at least as new as every file Cargo's dep-info
/// beside it (`aws.d`) records it was built from: each Rust source of every
/// crate it links, each file a crate includes, and each generated or staged
/// file its build scripts gave it, the Python payload included. That is the
/// rule Cargo itself applies to those files, so a binary this passes is
/// one Cargo would not relink for them.
///
/// The payload's own inputs (the AWS CLI sources, the pins, the stage
/// scripts) are keyed by content into the dependency store, not by time: a
/// new key names a new payload, which only `cargo xtask build` computes, at
/// the cost of hashing them all. This check does not recompute it.
fn require_current(root: &Path, binary: &Path, build: &str) -> Result<(), TaskError> {
    let record = binary.with_extension("d");
    let text = match fs::read_to_string(&record) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(deps::error(format!(
                "Rust CLI {} has no Cargo dep-info {}, so whether it is current \
                 is unknown; run `{build}`",
                binary.display(),
                record.display()
            )));
        }
        Err(error) => return Err(error.into()),
    };
    let sources = dep_info_sources(&text);
    if sources.is_empty() {
        return Err(deps::error(format!(
            "Cargo dep-info {} records no sources; run `{build}`",
            record.display()
        )));
    }
    let built = fs::metadata(binary)?.modified()?;
    for source in sources {
        let source = root.join(source);
        let why = match fs::metadata(&source) {
            Ok(metadata) if metadata.modified()? > built => "is newer than it",
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => "is gone",
            Err(error) => return Err(error.into()),
        };
        return Err(deps::error(format!(
            "Rust CLI is stale: {} was built from {}, which {why}; run `{build}`",
            binary.display(),
            source.display()
        )));
    }
    Ok(())
}

/// The prerequisites of the first rule of Cargo's dep-info `text`,
/// `target: source source ...`, in which a space within a path is escaped
/// as `\ `.
fn dep_info_sources(text: &str) -> Vec<PathBuf> {
    let line = text.lines().next().unwrap_or_default();
    let Some((_, prerequisites)) = line.split_once(": ") else {
        return Vec::new();
    };
    let mut sources = Vec::new();
    let mut current = String::new();
    let mut characters = prerequisites.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\\' if characters.peek() == Some(&' ') => {
                current.push(' ');
                characters.next();
            }
            ' ' if !current.is_empty() => sources.push(PathBuf::from(std::mem::take(&mut current))),
            ' ' => {}
            _ => current.push(character),
        }
    }
    if !current.is_empty() {
        sources.push(PathBuf::from(current));
    }
    sources
}

#[derive(Debug, PartialEq, Eq)]
struct Counts {
    tests: usize,
    failures: usize,
    errors: usize,
    skipped: usize,
    xfailed: usize,
    native_parity_failures: usize,
}

/// Require the JUnit reports of one run's pytest sessions to hold no failures
/// or errors and, together, at least one test: a selection may leave any one
/// session empty. Each session's counts print under its own name.
pub(crate) fn verify_sessions(reports: &[(&str, PathBuf)], suite: &str) -> Result<(), TaskError> {
    let mut tests = 0;
    let mut unsuccessful = Vec::new();
    for (session, path) in reports {
        let label = format!("{suite} {session}");
        let actual = reported_counts(path, &label)?;
        tests += actual.tests;
        if actual.failures != 0 || actual.errors != 0 {
            unsuccessful.push(format!("{label}: {actual:?}"));
        }
    }
    if tests == 0 {
        return Err(deps::error(format!("{suite} ran no tests")));
    }
    if !unsuccessful.is_empty() {
        return Err(deps::error(format!(
            "{suite} has unsuccessful results: {}",
            unsuccessful.join("; ")
        )));
    }
    Ok(())
}

/// The counts of the JUnit report at `path`, printed under `label`.
fn reported_counts(path: &Path, label: &str) -> Result<Counts, TaskError> {
    let actual = counts(&fs::read_to_string(path)?)?;
    writeln!(
        io::stdout(),
        "{label}: {} collected, {} failures, {} errors, {} skips, \
         {} expected failures; declared parity gaps observed: {}",
        actual.tests,
        actual.failures,
        actual.errors,
        actual.skipped,
        actual.xfailed,
        actual.native_parity_failures,
    )?;
    Ok(actual)
}

fn counts(xml: &str) -> Result<Counts, TaskError> {
    let (_, suite) = xml
        .split_once("<testsuite ")
        .ok_or_else(|| deps::error("JUnit report has no testsuite"))?;
    let (attributes, _) = suite
        .split_once('>')
        .ok_or_else(|| deps::error("JUnit testsuite header is truncated"))?;
    let total_skipped = attribute(attributes, "skipped")?;
    let xfailed = xml.matches("<skipped type=\"pytest.xfail\"").count();
    let native_parity_failures = xml
        .split("<skipped ")
        .skip(1)
        .filter_map(|element| element.split_once('>').map(|(header, _)| header))
        .filter(|header| {
            header.contains("type=\"pytest.xfail\"") && header.contains("message=\"parity-gap:")
        })
        .count();
    let skipped = total_skipped
        .checked_sub(xfailed)
        .ok_or_else(|| deps::error("JUnit xfail count exceeds total skipped"))?;
    Ok(Counts {
        tests: attribute(attributes, "tests")?,
        failures: attribute(attributes, "failures")?,
        errors: attribute(attributes, "errors")?,
        skipped,
        xfailed,
        native_parity_failures,
    })
}

fn attribute(attributes: &str, name: &str) -> Result<usize, TaskError> {
    let prefix = format!("{name}=\"");
    attributes
        .split_ascii_whitespace()
        .find_map(|field| field.strip_prefix(&prefix)?.strip_suffix('"'))
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| deps::error(format!("JUnit testsuite has no integer {name} attribute")))
}

/// Run the dependency drivers' self-tests, `tests/deps`, through their
/// runner, which provisions their venv. Arguments go to pytest.
///
/// # Errors
///
/// When the managed compiler cannot be prepared, or the tests fail.
pub fn self_test(root: &Path, args: &[String]) -> Result<(), TaskError> {
    let compiler = self_test_compiler(root)?;
    process::run(&mut self_test_command(root, compiler.as_deref(), args))
}

/// The managed compiler the dependency drivers' native compiler probes use.
/// It is the static toolchain, so on Linux it keeps the static host list
/// (`deps::toolchain`).
fn self_test_compiler(root: &Path) -> Result<Option<PathBuf>, TaskError> {
    if !cfg!(target_os = "linux") {
        return Ok(None);
    }
    Ok(Some(deps::toolchain(root, &deps::Pins::load(root)?)?.cc))
}

fn self_test_command(root: &Path, compiler: Option<&Path>, args: &[String]) -> Command {
    let mut command = Command::new("python3");
    // `-B` keeps the runner's own imports from writing bytecode under
    // `tests/`; the runner keeps the pytest child from writing any.
    command
        .current_dir(root)
        .arg("-B")
        .arg(root.join("tests/deps/run.py"))
        .arg("--cache-dir")
        .arg(root.join("target/harness-test"))
        .args(args)
        .env("AWS_EC2_METADATA_DISABLED", "true");
    if let Some(compiler) = compiler {
        command.env("AWS_NATIVE_TEST_CC", compiler);
    }
    command
}

#[cfg(test)]
mod tests;
