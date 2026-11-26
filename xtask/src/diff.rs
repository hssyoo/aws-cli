//! `cargo xtask diff`: the differential harness. It runs a corpus of CLI
//! invocations on two sides, A and B, each a binary and a route
//! (`AWS_CLI_NATIVE_ROUTER`), against the blackbox TLS stub, and compares
//! per invocation the exit status, stdout, stderr and every request.
//!
//! The rows are `tests/diff/corpus/*.jsonl`, or `--corpus FILE`
//! ([`corpus`]). This module writes them with the two sides as a job, and
//! `tests/diff/run.py` provisions the harness venv and runs
//! `tests/diff/capture.py`, which executes every row on A then on B through
//! one `localstub` server and TLS intercept proxy, with a cleared
//! environment, fake keys and a scratch HOME ([`capture`]). The fields that
//! vary from run to run are normalized on both sides alike ([`normalize`]),
//! and what still differs is named by fingerprint ([`compare`]): the stage
//! and its first differing field. The run passes when every difference is
//! listed in `tests/diff/known-differences.json`, which starts empty
//! ([`report`]); the report proposes entries and never writes them.
//!
//! A side is `candidate` (the default artifact, which must be present and
//! current as for `blackbox`, or `--aws-binary PATH`) or a binary's path,
//! then `:native`, `:python` or `:unset`; both default to
//! `candidate:native`. `--baseline SHA|PATH [--route R]` sets A to a
//! commit's binary, built once and cached ([`baseline`]), or a file, and B
//! to the candidate, both on route R (native by default). `--self-test`
//! runs the harness on a seeded defect ([`seeded`]). The report, job and captures are written under
//! `target/diff/<mode>`, or `--out DIR`.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::{TaskError, deps, harness, process};

mod admission;
mod baseline;
mod capture;
mod compare;
mod corpus;
mod json;
mod normalize;
mod options;
mod report;
mod seeded;
mod sigv4;

use options::{Binary, Options, Route, SideSpec};

const CORPUS_DIRECTORY: &str = "tests/diff/corpus";
const KNOWN_DIFFERENCES: &str = "tests/diff/known-differences.json";
/// The corpus `tests/diff/convert_parity.py` makes from the parity cases.
pub const PARITY_CORPUS: &str = "tests/diff/corpus/parity.jsonl";

/// One side of a comparison: what the report calls it, its binary and its
/// route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Side {
    pub label: String,
    pub binary: PathBuf,
    pub route: Route,
}

impl Side {
    fn describe(&self) -> String {
        format!("{}:{}", self.label, self.route.as_str())
    }
}

/// What one comparison runs, and where it writes.
#[derive(Debug)]
pub struct Run {
    pub a: Side,
    pub b: Side,
    pub corpus: Vec<PathBuf>,
    pub filter: Option<String>,
    /// The known differences, or none at all.
    pub known: Option<PathBuf>,
    pub out: PathBuf,
}

impl Run {
    /// `oracle` when the routes differ, `baseline` when only the binaries
    /// do, and `determinism` when the sides are the same.
    pub fn mode(&self) -> &'static str {
        if self.a.route != self.b.route {
            "oracle"
        } else if self.a.binary != self.b.binary {
            "baseline"
        } else {
            "determinism"
        }
    }
}

pub fn run(root: &Path, args: &[String]) -> Result<(), TaskError> {
    let options = options::parse(args)?;
    if options.check_corpus {
        capture::check_corpus(root)?;
        return writeln!(
            io::stdout().lock(),
            "diff: {PARITY_CORPUS} is what its converter writes"
        )
        .map_err(TaskError::from);
    }
    // The runs hold the local CLI test lock, so no other CLI suite of this
    // worktree loads the machine or contends for the CRT's process lock.
    let _cli_test_guard = process::cli_test_guard(root)?;
    if options.self_test {
        return seeded::run(root, &options.aws_binary);
    }
    let run = plan(root, &options)?;
    let report = execute(root, &run)?;
    let mut stdout = io::stdout().lock();
    write!(stdout, "{}", report.summary())?;
    writeln!(
        stdout,
        "diff: report in {}",
        run.out.join("report.json").display()
    )?;
    report.verdict()
}

/// The comparison `options` ask for.
fn plan(root: &Path, options: &Options) -> Result<Run, TaskError> {
    let candidate = |route| SideSpec {
        binary: Binary::Candidate,
        route,
    };
    let (a, b) = if options.oracle {
        (candidate(Route::Native), candidate(Route::Python))
    } else {
        let default = || candidate(Route::Native);
        (
            options.a.clone().unwrap_or_else(default),
            options.b.clone().unwrap_or_else(default),
        )
    };
    let mut a = side(root, &a, &options.aws_binary)?;
    let mut b = side(root, &b, &options.aws_binary)?;
    if let Some(value) = &options.baseline {
        let (label, binary) = baseline::resolve(root, value)?;
        let route = options.route.unwrap_or(Route::Native);
        a = Side {
            label,
            binary,
            route,
        };
        b.route = route;
    }
    let corpus = if options.corpus.is_empty() {
        corpus::files(root, Path::new(CORPUS_DIRECTORY))?
    } else {
        options.corpus.clone()
    };
    let mut run = Run {
        a,
        b,
        corpus,
        filter: options.filter.clone(),
        known: Some(
            options
                .known
                .clone()
                .unwrap_or_else(|| PathBuf::from(KNOWN_DIFFERENCES)),
        ),
        out: PathBuf::new(),
    };
    run.out = options.out.as_ref().map_or_else(
        || root.join("target/diff").join(run.mode()),
        |out| root.join(out),
    );
    Ok(run)
}

fn side(root: &Path, spec: &SideSpec, aws_binary: &[String]) -> Result<Side, TaskError> {
    let (label, binary) = match &spec.binary {
        Binary::Candidate => ("candidate".to_owned(), harness::binary(root, aws_binary)?),
        Binary::Path(path) => (path.display().to_string(), root.join(path)),
    };
    if !binary.is_file() {
        return Err(deps::error(format!("no binary at {}", binary.display())));
    }
    Ok(Side {
        label,
        binary,
        route: spec.route,
    })
}

/// Runs `run` and writes its report beside the job and captures.
pub fn execute(root: &Path, run: &Run) -> Result<report::Report, TaskError> {
    // An earlier run's report must not outlive a capture that fails.
    for stale in ["report.json", "captures.jsonl"] {
        match fs::remove_file(run.out.join(stale)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
    }
    let rows = corpus::load(root, &run.corpus, run.filter.as_deref())?;
    let known = match &run.known {
        Some(path) => report::known(&fs::read_to_string(root.join(path))?)?,
        None => Vec::new(),
    };
    let captures = capture::run(root, run, &rows)?;
    let pairs = capture::pair(&rows, captures)?;
    let report = report::build(run, &rows, &pairs, &known);
    fs::write(run.out.join("report.json"), report.json()?)?;
    Ok(report)
}
