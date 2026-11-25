//! `cargo xtask perf`: a quick look at how the AWS CLI's commands perform
//! on the native route and on the Python route, against loopback stub
//! services. It informs; it judges nothing.
//!
//! `tests/perf/run.py` provisions its venv and runs `tests/perf/measure.py`,
//! which starts a stub for each scheme the cases use (`tests/perf/stub.py`;
//! over HTTPS with a test CA the CLI trusts through `AWS_CA_BUNDLE`) and
//! runs each case of `tests/perf/cases.json` on both routes, round-robin,
//! after warm-up rounds. No command can reach anything but the stubs: each
//! runs with the stub as `--endpoint-url` and `AWS_ENDPOINT_URL`, fake keys
//! and a scratch HOME. A run that exits non-zero or makes other than its
//! case's page count of requests stops the measurement, since its numbers
//! would not be of the command meant.
//!
//! The binary is the default artifact, which must be present and current
//! as for `blackbox` (`harness::binary`), or `--aws-binary PATH`. The
//! samples are written to `target/perf/result.json`, or `--out PATH`, and
//! the medians printed as a table of native beside Python.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::{TaskError, deps, harness, process};

/// The version of measure.py's samples.
const FORMAT: u32 = 1;

/// measure.py's output file.
#[derive(Debug, Deserialize)]
struct Samples {
    format: u32,
    cpu: String,
    cpus: u32,
    warmup: u32,
    runs: u32,
    series: Vec<Series>,
}

/// One case at one scheme and latency, on one route.
#[derive(Debug, Deserialize)]
struct Series {
    case: String,
    scheme: String,
    latency_ms: u32,
    route: String,
    runs: Vec<Run>,
}

/// One run: its wall time, its CPU time and peak RSS from `wait4`, what
/// the stub counted, and the load average before it.
#[derive(Debug, Deserialize)]
struct Run {
    wall_s: f64,
    user_s: f64,
    sys_s: f64,
    max_rss_bytes: f64,
    connections: u32,
    requests: u32,
    load_average: f64,
}

/// `--aws-binary PATH` as `harness::binary` takes it, and `--out PATH`.
#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    binary: Vec<String>,
    out: Option<PathBuf>,
}

fn options(args: &[String]) -> Result<Options, TaskError> {
    let mut options = Options::default();
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .cloned()
            .ok_or_else(|| deps::error(format!("{flag} requires a value")));
        match flag.as_str() {
            "--aws-binary" => options.binary = vec![flag.clone(), value?],
            "--out" => options.out = Some(PathBuf::from(value?)),
            _ => return Err(deps::error(format!("unknown perf option: {flag}"))),
        }
    }
    Ok(options)
}

pub fn run(root: &Path, args: &[String]) -> Result<(), TaskError> {
    let options = options(args)?;
    let binary = harness::binary(root, &options.binary)?;
    let directory = root.join("target/perf");
    fs::create_dir_all(&directory)?;
    let out = options
        .out
        .map_or_else(|| directory.join("result.json"), |path| root.join(path));
    // The runs hold the local CLI test lock, so no other CLI suite of this
    // worktree loads the machine or contends for the CRT's process lock.
    let _cli_test_guard = process::cli_test_guard(root)?;
    let mut command = Command::new("python3");
    // `-B` keeps the runner's imports from writing bytecode under `tests/`.
    command
        .current_dir(root)
        .arg("-B")
        .arg(root.join("tests/perf/run.py"))
        .arg("--venv")
        .arg(directory.join("venv"))
        .arg("--aws-binary")
        .arg(&binary)
        .arg("--cases")
        .arg(root.join("tests/perf/cases.json"))
        .arg("--models")
        .arg(root.join("tests/models"))
        .arg("--scratch")
        .arg(directory.join("scratch"))
        .arg("--out")
        .arg(&out);
    process::run(&mut command)?;
    let samples = parse(&fs::read_to_string(&out)?)?;
    let mut stdout = io::stdout().lock();
    write!(stdout, "{}", table(&binary.display().to_string(), &samples))?;
    writeln!(stdout, "perf: samples in {}", out.display())?;
    Ok(())
}

fn parse(text: &str) -> Result<Samples, TaskError> {
    let samples: Samples = serde_json::from_str(text)
        .map_err(|error| deps::error(format!("perf samples: {error}")))?;
    if samples.format != FORMAT {
        return Err(deps::error(format!(
            "perf samples have format {}, expected {FORMAT}",
            samples.format
        )));
    }
    Ok(samples)
}

/// The median of `values`, or zero when there are none.
fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    match values.len() {
        0 => 0.0,
        count if count % 2 == 1 => values[count / 2],
        count => f64::midpoint(values[count / 2 - 1], values[count / 2]),
    }
}

const HEADER: [&str; 10] = [
    "case",
    "scheme",
    "rtt ms",
    "native ms",
    "python ms",
    "native cpu",
    "python cpu",
    "native MB",
    "python MB",
    "conns/reqs",
];

/// The samples as a table: for each case, scheme and latency, the medians
/// of the native route beside the Python route's (wall and CPU time in ms,
/// peak RSS in MB), and the counts of each route's runs.
fn table(binary: &str, samples: &Samples) -> String {
    let loads: Vec<f64> = samples
        .series
        .iter()
        .flat_map(|series| series.runs.iter().map(|run| run.load_average))
        .collect();
    let low = loads.iter().copied().fold(f64::INFINITY, f64::min);
    let high = loads.iter().copied().fold(0.0, f64::max);
    let mut rows = vec![HEADER.map(str::to_owned).to_vec()];
    for native in samples
        .series
        .iter()
        .filter(|series| series.route == "native")
    {
        let python = samples.series.iter().find(|series| {
            series.route == "python"
                && (&series.case, &series.scheme, series.latency_ms)
                    == (&native.case, &native.scheme, native.latency_ms)
        });
        rows.push(row(native, python));
    }
    format!(
        "perf: {binary} on {} ({} CPUs); medians of {} runs after {} warm-up; load \
         average {low:.2} to {high:.2}\n{}",
        samples.cpu,
        samples.cpus,
        samples.runs,
        samples.warmup,
        layout(&rows)
    )
}

/// A route's median of `measure`, scaled, or a dash for a missing route.
fn cell(series: Option<&Series>, measure: fn(&Run) -> f64, scale: f64) -> String {
    series.map_or_else(
        || "-".to_owned(),
        |series| {
            format!(
                "{:.1}",
                median(series.runs.iter().map(measure).collect()) * scale
            )
        },
    )
}

/// A route's connections and requests, said to vary if its runs differ.
fn counts(series: Option<&Series>) -> String {
    let Some(series) = series else {
        return "-".to_owned();
    };
    let pairs: Vec<(u32, u32)> = series
        .runs
        .iter()
        .map(|run| (run.connections, run.requests))
        .collect();
    let (connections, requests) = pairs.first().copied().unwrap_or_default();
    let varied = pairs.iter().any(|pair| *pair != (connections, requests));
    format!(
        "{connections}/{requests}{}",
        if varied { " varied" } else { "" }
    )
}

fn row(native: &Series, python: Option<&Series>) -> Vec<String> {
    let wall = |run: &Run| run.wall_s;
    let cpu = |run: &Run| run.user_s + run.sys_s;
    let rss = |run: &Run| run.max_rss_bytes;
    vec![
        native.case.clone(),
        native.scheme.clone(),
        native.latency_ms.to_string(),
        cell(Some(native), wall, 1e3),
        cell(python, wall, 1e3),
        cell(Some(native), cpu, 1e3),
        cell(python, cpu, 1e3),
        cell(Some(native), rss, 1e-6),
        cell(python, rss, 1e-6),
        format!("{} {}", counts(Some(native)), counts(python)),
    ]
}

/// `rows` aligned: the first three columns to the left, the rest to the
/// right.
fn layout(rows: &[Vec<String>]) -> String {
    let widths: Vec<usize> = (0..HEADER.len())
        .map(|column| rows.iter().map(|row| row[column].len()).max().unwrap_or(0))
        .collect();
    let mut text = String::new();
    for row in rows {
        let cells: Vec<String> = row
            .iter()
            .zip(&widths)
            .enumerate()
            .map(|(column, (cell, width))| {
                if column < 3 {
                    format!("{cell:<width$}")
                } else {
                    format!("{cell:>width$}")
                }
            })
            .collect();
        text.push_str(cells.join("  ").trim_end());
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests;
