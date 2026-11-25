//! `cargo xtask diff --self-test`: the harness must catch a seeded defect
//! and name it, and find nothing when the candidate meets itself.
//!
//! The seeded binary is a shell wrapper of the candidate that changes one
//! request header, `amz-sdk-request: max=` (by `AWS_MAX_ATTEMPTS=7`, which
//! changes nothing else on a request that succeeds), and one stdout byte:
//! the last character of a string value ending the second line. Every row
//! of the seed corpus succeeds and prints such a line, so each row must
//! differ by exactly those two fingerprints.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use super::options::Route;
use super::report::Report;
use super::{Run, Side};
use crate::{TaskError, deps, harness};

const SEED: &str = "tests/diff/corpus/seed.jsonl";
/// The fingerprints the seeded defect must be named by, sorted.
pub const EXPECTED: [&str; 2] = [
    "request[*].header:amz-sdk-request:value",
    "stdout:$.ThingId",
];

pub fn run(root: &Path, aws_binary: &[String]) -> Result<(), TaskError> {
    let candidate = Side {
        label: "candidate".to_owned(),
        binary: harness::binary(root, aws_binary)?,
        route: Route::Native,
    };
    let directory = root.join("target/diff/self-test");
    let wrapper = directory.join("seeded/aws");
    write_wrapper(&wrapper, &candidate.binary)?;
    let seeded = Side {
        label: "seeded".to_owned(),
        binary: wrapper,
        route: Route::Native,
    };
    let against = |b: &Side, name: &str| Run {
        a: candidate.clone(),
        b: b.clone(),
        corpus: vec![PathBuf::from(SEED)],
        filter: None,
        known: None,
        out: directory.join(name),
    };
    let caught = super::execute(root, &against(&seeded, "seeded"))?;
    check_caught(&caught)?;
    let itself = super::execute(root, &against(&candidate, "determinism"))?;
    if itself.mismatched != 0 {
        return Err(deps::error(format!(
            "diff self-test: the candidate differs from itself\n{}",
            itself.summary()
        )));
    }
    let mut stdout = io::stdout().lock();
    write!(stdout, "{}{}", caught.summary(), itself.summary())?;
    writeln!(
        stdout,
        "diff: self-test passed: the seeded defect is named by {} in each of {} rows, \
         and the candidate does not differ from itself",
        EXPECTED.join(" and "),
        caught.rows
    )?;
    Ok(())
}

/// Fails unless every row differs, by the expected fingerprints alone.
pub fn check_caught(report: &Report) -> Result<(), TaskError> {
    let mut named = report.fingerprints();
    named.sort_unstable();
    if named == EXPECTED && report.mismatched == report.rows && report.verdict().is_err() {
        return Ok(());
    }
    Err(deps::error(format!(
        "diff self-test: the seeded defect must make every row differ by {} alone\n{}",
        EXPECTED.join(" and "),
        report.summary()
    )))
}

/// The seeded binary's script, running `binary`.
pub fn wrapper_script(binary: &Path) -> String {
    let quoted = binary.display().to_string().replace('\'', r"'\''");
    format!(
        "#!/bin/bash\n\
         # cargo xtask diff --self-test's seeded defect: the CLI below with\n\
         # amz-sdk-request max=7, and the last character of a string value\n\
         # ending stdout's second line changed to X.\n\
         set -o pipefail\n\
         export AWS_MAX_ATTEMPTS=7\n\
         '{quoted}' \"$@\" | sed -e '2s/[[:alnum:]]\",$/X\",/' -e '2s/[[:alnum:]]\"$/X\"/'\n"
    )
}

fn write_wrapper(path: &Path, binary: &Path) -> Result<(), TaskError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, wrapper_script(binary))?;
    executable(path)
}

#[cfg(unix)]
fn executable(path: &Path) -> Result<(), TaskError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(not(unix))]
fn executable(_: &Path) -> Result<(), TaskError> {
    Err(deps::error("diff --self-test needs a Unix shell"))
}

#[cfg(test)]
mod tests;
