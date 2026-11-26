//! The capture: the job `tests/diff/capture.py` runs, and what it recorded.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use super::corpus::Row;
use super::{Run, Side};
use crate::{TaskError, deps, process};

/// The version of the job and capture formats.
const FORMAT: u32 = 1;

/// One run of one row on one side.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    pub id: String,
    pub side: String,
    /// What the capture could not see whole, each `<kind>: <detail>`: a
    /// request the stub could not parse, a connection the proxy refused,
    /// requests the recorder dropped.
    #[serde(default)]
    pub harness: Vec<String>,
    /// The exit status, or none when the run timed out.
    pub exit: Option<i32>,
    pub timed_out: bool,
    #[serde(deserialize_with = "latin1")]
    pub stdout: Vec<u8>,
    #[serde(deserialize_with = "latin1")]
    pub stderr: Vec<u8>,
    /// The secret key the run had (a fake one), which its signatures must
    /// be made with.
    #[serde(default)]
    pub secret_key: Option<String>,
    /// The host of each CONNECT the proxy accepted, sorted.
    #[serde(default)]
    pub connects: Vec<String>,
    pub requests: Vec<Request>,
}

/// A request as the stub received it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub version: String,
    /// In wire order; Latin-1 text, as the bytes were.
    pub headers: Vec<(String, String)>,
    #[serde(deserialize_with = "latin1")]
    pub body: Vec<u8>,
    /// A chunked body's chunk-size lines, then its trailer lines.
    #[serde(default)]
    pub framing: Option<Vec<String>>,
    pub complete: bool,
}

/// A byte string the capture wrote as Latin-1 text.
fn latin1<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
    String::deserialize(deserializer)?
        .chars()
        .map(|character| u8::try_from(character).map_err(D::Error::custom))
        .collect()
}

#[derive(Debug, Serialize)]
struct Job<'a> {
    format: u32,
    scratch: PathBuf,
    captures: PathBuf,
    sides: [JobSide<'a>; 2],
    rows: Vec<Row>,
}

#[derive(Debug, Serialize)]
struct JobSide<'a> {
    name: &'static str,
    binary: &'a Path,
    route: &'static str,
}

impl<'a> JobSide<'a> {
    fn new(name: &'static str, side: &'a Side) -> Self {
        Self {
            name,
            binary: &side.binary,
            route: side.route.as_str(),
        }
    }
}

/// Runs `rows` on both sides of `run` and reads what each run recorded.
pub fn run(root: &Path, run: &Run, rows: &[Row]) -> Result<Vec<Capture>, TaskError> {
    fs::create_dir_all(&run.out)?;
    let captures = run.out.join("captures.jsonl");
    let job = Job {
        format: FORMAT,
        scratch: run.out.join("scratch"),
        captures: captures.clone(),
        sides: [JobSide::new("a", &run.a), JobSide::new("b", &run.b)],
        rows: rows.iter().map(|row| absolute(root, row)).collect(),
    };
    let path = run.out.join("job.json");
    let text = serde_json::to_string_pretty(&job)
        .map_err(|error| deps::error(format!("diff job: {error}")))?;
    fs::write(&path, text)?;
    let mut command = runner(root, "capture.py");
    command.arg("--job").arg(&path);
    process::run(&mut command)?;
    parse(&fs::read_to_string(&captures)?)
}

/// Fails unless the parity corpus is what its converter writes now.
pub fn check_corpus(root: &Path) -> Result<(), TaskError> {
    let mut command = runner(root, "convert_parity.py");
    command
        .arg("--aws-cli-source")
        .arg(root.join("aws-cli"))
        .arg("--cases")
        .arg(root.join("tests/parity/cases"))
        .arg("--models")
        .arg(root.join("tests/models"))
        .arg("--check")
        .arg(root.join(super::PARITY_CORPUS));
    process::run(&mut command)
}

/// `tests/diff/run.py` running `script` in the harness venv.
fn runner(root: &Path, script: &str) -> Command {
    let mut command = Command::new("python3");
    // `-B` keeps the runner's imports from writing bytecode under `tests/`.
    command
        .current_dir(root)
        .arg("-B")
        .arg(root.join("tests/diff/run.py"))
        .arg("--venv")
        .arg(root.join("target/diff/venv"))
        .arg("--aws-cli-source")
        .arg(root.join("aws-cli"))
        .arg(script);
    command
}

/// `row` with its data path made absolute.
fn absolute(root: &Path, row: &Row) -> Row {
    let mut row = row.clone();
    row.data_path = row.data_path.map(|path| root.join(path));
    row
}

pub fn parse(text: &str) -> Result<Vec<Capture>, TaskError> {
    text.lines()
        .map(|line| {
            serde_json::from_str(line)
                .map_err(|error| deps::error(format!("diff capture: {error}")))
        })
        .collect()
}

/// Each row's capture on A and on B, in row order.
pub fn pair(rows: &[Row], captures: Vec<Capture>) -> Result<Vec<(Capture, Capture)>, TaskError> {
    let mut a = Vec::new();
    let mut b = Vec::new();
    for capture in captures {
        match capture.side.as_str() {
            "a" => a.push(capture),
            "b" => b.push(capture),
            other => return Err(deps::error(format!("diff capture of unknown side {other}"))),
        }
    }
    let ids = |captures: &[Capture]| captures.iter().map(|c| c.id.clone()).collect::<Vec<_>>();
    let expected: Vec<String> = rows.iter().map(|row| row.id.clone()).collect();
    if ids(&a) != expected || ids(&b) != expected {
        return Err(deps::error(
            "the diff capture does not hold each row once per side",
        ));
    }
    Ok(a.into_iter().zip(b).collect())
}

#[cfg(test)]
mod tests;
