//! `cargo xtask diff --self-test`: the harness must catch a seeded defect
//! and name it on every row, find nothing when the candidate meets itself,
//! and see what a stand-in CLI does on the capture's other paths.
//!
//! The seeded binary is a shell wrapper of the candidate that changes one
//! request header, `amz-sdk-request: max=` (by `AWS_MAX_ATTEMPTS=7`, which
//! changes nothing else on a request that succeeds), and one stdout byte:
//! the last character of a string value ending the second line. Every row
//! of the seed corpus succeeds and prints such a line, so each row must
//! differ by exactly those two fingerprints.
//!
//! The stand-in (`tests/diff/standin.py`, rows `tests/diff/standin.jsonl`)
//! runs against itself: a run that outlives its timeout and a request the
//! stub cannot parse must differ by their uncoverable fingerprints alone, as
//! must a row that sends no request undeclared and one that sends a request
//! though it declares none; a fresh UUID in a header must match once
//! normalized; and the base environment, a row's variables, the reply sequence, the
//! request limit and a chunked body's framing must reach the capture as
//! documented.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use super::capture::{self, Capture};
use super::options::Route;
use super::report::Report;
use super::{Run, Side};
use crate::{TaskError, deps, harness};

const SEED: &str = "tests/diff/corpus/seed.jsonl";
const STANDIN: &str = "tests/diff/standin.jsonl";
/// The fingerprints the seeded defect must name on each row, sorted.
pub const EXPECTED: [&str; 2] = [
    "request[*].header:amz-sdk-request:value",
    "stdout:$.ThingId",
];
/// The stand-in rows that must differ from themselves, and by what.
const STANDIN_DIFFERS: [(&str, &[&str]); 4] = [
    (
        "standin-declared-none",
        &["requests:declared:0:a", "requests:declared:0:b"],
    ),
    (
        "standin-malformed",
        &["harness:a:protocol_error", "harness:b:protocol_error"],
    ),
    (
        "standin-no-request",
        &["requests:declared:none:a", "requests:declared:none:b"],
    ),
    ("standin-timeout", &["timeout:both"]),
];
/// The stand-in row whose request carries a fresh UUID in a header, which
/// must differ between the runs as sent and match once normalized.
const STANDIN_HEADER_UUID: (&str, &str) = ("standin-header-uuid", "X-Token");
/// Lines the stand-in's environment must hold, and prefixes it must not.
const ENV_HOLDS: [&str; 6] = [
    "AWS_PAGER=",
    "STANDIN=1",
    "PATH=/usr/bin:/bin",
    "TZ=UTC",
    "AWS_EC2_METADATA_DISABLED=true",
    "AWS_ACCESS_KEY_ID=AKIDEXAMPLE",
];
const ENV_LACKS: [&str; 2] = ["AWS_DEFAULT_REGION=", "AWS_CLI_NATIVE_ROUTER="];

pub fn run(root: &Path, aws_binary: &[String]) -> Result<(), TaskError> {
    let candidate = Side {
        label: "candidate".to_owned(),
        binary: harness::binary(root, aws_binary)?,
        route: Route::Native,
    };
    let directory = root.join("target/diff/self-test");
    let seeded = side(
        &directory,
        "seeded",
        &wrapper_script(&candidate.binary),
        Route::Native,
    )?;
    let standin_script = standin_script(&root.join("tests/diff/standin.py"));
    let standin = side(&directory, "stand-in", &standin_script, Route::Unset)?;
    let against = |a: &Side, b: &Side, corpus: &str, name: &str| Run {
        a: a.clone(),
        b: b.clone(),
        corpus: vec![PathBuf::from(corpus)],
        filter: None,
        known: None,
        out: directory.join(name),
    };
    let caught = super::execute(root, &against(&candidate, &seeded, SEED, "seeded"))?;
    check_caught(&caught)?;
    let itself = super::execute(root, &against(&candidate, &candidate, SEED, "determinism"))?;
    if itself.mismatched != 0 {
        return Err(failure("the candidate differs from itself", &itself));
    }
    let paths = against(&standin, &standin, STANDIN, "stand-in");
    let report = super::execute(root, &paths)?;
    let captures = capture::parse(&fs::read_to_string(paths.out.join("captures.jsonl"))?)?;
    check_standin(&report, &captures)?;
    let mut stdout = io::stdout().lock();
    write!(
        stdout,
        "{}{}{}",
        caught.summary(),
        itself.summary(),
        report.summary()
    )?;
    writeln!(
        stdout,
        "diff: self-test passed: the seeded defect is named by {} in each of {} rows, \
         the candidate does not differ from itself, and the stand-in's {} rows \
         reach the capture as documented",
        EXPECTED.join(" and "),
        caught.rows,
        report.rows
    )?;
    Ok(())
}

fn failure(what: &str, report: &Report) -> TaskError {
    deps::error(format!("diff self-test: {what}\n{}", report.summary()))
}

/// Fails unless every row differs by the expected fingerprints alone.
pub fn check_caught(report: &Report) -> Result<(), TaskError> {
    let rows = report.by_row();
    if rows.len() == report.rows && rows.values().all(|names| *names == EXPECTED) {
        return Ok(());
    }
    Err(failure(
        &format!(
            "the seeded defect must make every row differ by {} alone",
            EXPECTED.join(" and ")
        ),
        report,
    ))
}

/// Fails unless the stand-in rows differ as `STANDIN_DIFFERS` says and its
/// runs on A saw what the capture promises.
pub fn check_standin(report: &Report, captures: &[Capture]) -> Result<(), TaskError> {
    let expected: BTreeMap<&str, Vec<&str>> = STANDIN_DIFFERS
        .iter()
        .map(|(id, names)| (*id, names.to_vec()))
        .collect();
    let mut problems = Vec::new();
    if report.by_row() != expected {
        problems.push(format!(
            "the rows differ by {:?}, not {expected:?}",
            report.by_row()
        ));
    }
    let run = |id: &str| {
        captures
            .iter()
            .find(|capture| capture.id == id && capture.side == "a")
    };
    let printed =
        |id: &str| run(id).map(|capture| String::from_utf8_lossy(&capture.stdout).into_owned());
    let sent = |id: &str| run(id).map(|capture| capture.requests.len());
    let limit = format!("{}{}", "200 x\n".repeat(64), "closed\n".repeat(2));
    for (id, stdout, requests) in [
        ("standin-replies", "200 one\n201 two\n201 two\n", 3),
        ("standin-limit", &limit, 66),
    ] {
        if printed(id).as_deref() != Some(stdout) || sent(id) != Some(requests) {
            problems.push(format!(
                "{id} printed {:?} after {:?} requests",
                printed(id),
                sent(id)
            ));
        }
    }
    let chunked = run("standin-chunked").and_then(|capture| capture.requests.first());
    let framing = chunked.map(|request| (request.body.as_slice(), request.framing.clone()));
    let expected = ["3", "2", "0"].map(str::to_owned).to_vec();
    if framing != Some((b"hello".as_slice(), Some(expected))) {
        problems.push(format!("standin-chunked sent {framing:?}"));
    }
    problems.extend(header_uuid(captures));
    let env = printed("standin-env").unwrap_or_default();
    let lines: Vec<&str> = env.lines().collect();
    let missing = ENV_HOLDS.iter().filter(|line| !lines.contains(line));
    let present = ENV_LACKS
        .iter()
        .filter(|prefix| lines.iter().any(|line| line.starts_with(*prefix)));
    if let Some(line) = missing.chain(present).next() {
        problems.push(format!(
            "the stand-in's environment is wrong at {line:?}:\n{env}"
        ));
    }
    if problems.is_empty() {
        return Ok(());
    }
    Err(failure(&problems.join("\n"), report))
}

/// A problem unless the header-UUID row sent a fresh UUID on each side.
fn header_uuid(captures: &[Capture]) -> Option<String> {
    let (id, name) = STANDIN_HEADER_UUID;
    let tokens: Vec<Option<&str>> = ["a", "b"]
        .into_iter()
        .map(|side| {
            let capture = captures
                .iter()
                .find(|capture| capture.id == id && capture.side == side)?;
            let request = capture.requests.first()?;
            let (_, value) = request.headers.iter().find(|(header, _)| header == name)?;
            Some(value.as_str())
        })
        .collect();
    let fresh = matches!(tokens.as_slice(), [Some(x), Some(y)]
        if x != y && [x, y].iter().all(|token| super::normalize::is_uuid(token.as_bytes())));
    if !fresh {
        return Some(format!(
            "{id} must send a fresh UUID in {name} on each side, not {tokens:?}"
        ));
    }
    None
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

/// The stand-in's script, running `standin` on the host's python3.
pub fn standin_script(standin: &Path) -> String {
    let quoted = standin.display().to_string().replace('\'', r"'\''");
    format!("#!/bin/bash\nexec python3 -I -B '{quoted}' \"$@\"\n")
}

/// A side whose binary is `script`, written under `directory/name`.
fn side(directory: &Path, name: &str, script: &str, route: Route) -> Result<Side, TaskError> {
    let path = directory.join(name).join("aws");
    write_script(&path, script)?;
    Ok(Side {
        label: name.to_owned(),
        binary: path,
        route,
    })
}

fn write_script(path: &Path, script: &str) -> Result<(), TaskError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, script)?;
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
