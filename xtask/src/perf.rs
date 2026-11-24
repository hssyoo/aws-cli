//! `cargo xtask perf`: the AWS CLI's performance, measured against loopback
//! stub services, and the gate that holds it.
//!
//! `tests/perf/run.py` provisions its venv and runs `tests/perf/measure.py`,
//! which starts an HTTP and an HTTPS stub (`tests/perf/stub.py`, with a
//! test CA the CLI trusts through `AWS_CA_BUNDLE`) and runs the cases of
//! `tests/perf/cases.json` on the native and the Python route, round-robin,
//! `warmup` rounds unrecorded and `runs` recorded. The quick tier, the
//! default, runs the few series each case names as quick; `--full` runs
//! every scheme and injected latency of every case. No command can reach
//! anything but the stubs:
//! each runs with the stub as `--endpoint-url` and `AWS_ENDPOINT_URL`, fake
//! keys and a scratch HOME.
//!
//! This command measures the default artifact, which must be present and
//! current as for `blackbox` (`harness::binary`), or `--aws-binary PATH`
//! (with `--commit REV` naming what it was built from). It summarises the
//! runs ([`record`]), writes the result file (`target/perf/result.json`, or
//! `--out PATH`) and prints a table ([`table`]). `--check` judges the result
//! against the committed baseline of its platform,
//! `tests/perf/baselines/<os>-<arch>.json` ([`gate`]); a platform without
//! one is reported and not judged. `--write-baseline` writes a full-tier
//! result as that baseline, which both tiers are judged against; it takes
//! a complete run of a committed head.
//! `tests/perf/README.md` holds the procedure and the thresholds' reasons.

mod gate;
mod options;
mod record;
mod stats;
mod table;

use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

use crate::{TaskError, deps, harness, process};
use options::{Mode, Options};
use record::{Baseline, Binary, Record, Samples, Tier};

pub fn run(root: &Path, args: &[String]) -> Result<(), TaskError> {
    let options = options::parse(args)?;
    if options.mode == Mode::WriteBaseline && !options.is_complete() {
        return Err(deps::error(
            "a baseline takes every case on both routes with the case file's run counts: \
             drop --case, --route, --runs and --warmup",
        ));
    }
    let binary = harness::binary(root, &options.binary_args())?;
    let identity = identity(root, &binary, &options)?;
    if options.mode == Mode::WriteBaseline
        && (identity.commit.is_none() || identity.dirty != Some(false))
    {
        return Err(deps::error(
            "a baseline is measured on the default artifact of a committed head, \
             with no uncommitted changes",
        ));
    }
    let out = options.out.as_ref().map_or_else(
        || root.join("target/perf/result.json"),
        |path| root.join(path),
    );
    let baseline = root
        .join("tests/perf/baselines")
        .join(format!("{}.json", platform()));
    let take = |tier, out: &Path| take(root, &binary, &options, &identity, tier, out);
    match options.mode {
        Mode::Report => take(options.tier, &out).map(drop),
        Mode::Check => check(&baseline, &take(options.tier, &out)?),
        Mode::WriteBaseline => {
            let full = take(Tier::Full, &out)?;
            let quick = take(Tier::Quick, &out.with_extension("quick.json"))?;
            if let Some(directory) = baseline.parent() {
                fs::create_dir_all(directory)?;
            }
            fs::write(&baseline, Baseline::new(&quick, &full).to_json()?)?;
            writeln!(
                io::stdout(),
                "perf: baseline written to {}",
                baseline.display()
            )?;
            Ok(())
        }
    }
}

/// One measurement of `binary` at `tier`: the record, written to `out`
/// and printed as a table.
fn take(
    root: &Path,
    binary: &Path,
    options: &Options,
    identity: &Binary,
    tier: Tier,
    out: &Path,
) -> Result<Record, TaskError> {
    let samples = measure(root, binary, options, tier)?;
    let record = Record::new(samples, &platform(), identity.clone(), tier)?;
    fs::write(out, record.to_json()?)?;
    let mut stdout = io::stdout().lock();
    write!(stdout, "{}", table::render(&record))?;
    writeln!(stdout, "perf: result in {}", out.display())?;
    Ok(record)
}

/// `<os>-<arch>` as Rust names them: the key a baseline is filed under.
fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// What the result says was measured. The default artifact is current
/// with the worktree, so it was built from `HEAD` and whatever tracked
/// changes are uncommitted; an explicit binary's commit is `--commit`'s.
fn identity(root: &Path, binary: &Path, options: &Options) -> Result<Binary, TaskError> {
    let path = shown(root, binary);
    if options.binary.is_some() {
        return Ok(Binary {
            path,
            commit: options.commit.clone(),
            dirty: None,
        });
    }
    if options.commit.is_some() {
        return Err(deps::error(
            "--commit names the commit of an explicit --aws-binary",
        ));
    }
    let head = git(root, &["rev-parse", "HEAD"])?;
    let changes = git(root, &["status", "--porcelain", "--untracked-files=no"])?;
    Ok(Binary {
        path,
        commit: Some(head),
        dirty: Some(!changes.is_empty()),
    })
}

/// `binary` as the result records it: relative to `root` when it lies
/// under it, so a committed baseline names no checkout's location.
fn shown(root: &Path, binary: &Path) -> String {
    binary
        .strip_prefix(root)
        .unwrap_or(binary)
        .display()
        .to_string()
}

fn git(root: &Path, args: &[&str]) -> Result<String, TaskError> {
    let output = process::output(Command::new("git").current_dir(root).args(args))?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The samples measure.py takes of `binary`. The runs hold the local CLI
/// test lock, so no other CLI suite of this worktree loads the machine or
/// contends for the CRT's process lock meanwhile.
fn measure(
    root: &Path,
    binary: &Path,
    options: &Options,
    tier: Tier,
) -> Result<Samples, TaskError> {
    let directory = root.join("target/perf");
    fs::create_dir_all(&directory)?;
    let samples = directory.join("samples.json");
    match fs::remove_file(&samples) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    let _cli_test_guard = process::cli_test_guard(root)?;
    let paths = [binary, &directory, &samples];
    process::run(&mut measure_command(root, paths, options, tier))?;
    let text = fs::read_to_string(&samples)?;
    serde_json::from_str(&text).map_err(|error| {
        deps::error(format!(
            "{} is not perf samples: {error}",
            samples.display()
        ))
    })
}

/// measure.py's command: `binary` measured at `tier`, with its venv and
/// scratch under `directory` and its samples written to `samples`.
fn measure_command(
    root: &Path,
    [binary, directory, samples]: [&Path; 3],
    options: &Options,
    tier: Tier,
) -> Command {
    let mut command = Command::new("python3");
    // `-B` keeps the runner's imports from writing bytecode under `tests/`.
    command
        .current_dir(root)
        .arg("-B")
        .arg(root.join("tests/perf/run.py"))
        .arg("--venv")
        .arg(directory.join("venv"))
        .arg("--aws-binary")
        .arg(binary)
        .arg("--cases")
        .arg(root.join("tests/perf/cases.json"))
        .arg("--models")
        .arg(root.join("tests/models"))
        .arg("--scratch")
        .arg(directory.join("scratch"))
        .arg("--out")
        .arg(samples)
        .arg("--tier")
        .arg(tier.name());
    for (flag, value) in [("--runs", options.runs), ("--warmup", options.warmup)] {
        if let Some(value) = value {
            command.arg(flag).arg(value.to_string());
        }
    }
    for case in &options.cases {
        command.arg("--case").arg(case);
    }
    for route in &options.routes {
        command.arg("--route").arg(route);
    }
    command
}

/// Judge `record` against the baseline at `path`, if there is one.
fn check(path: &Path, record: &Record) -> Result<(), TaskError> {
    let mut stdout = io::stdout().lock();
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            writeln!(
                stdout,
                "perf: no baseline for {} ({}): the result is reported, not judged",
                record.platform,
                path.display()
            )?;
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    let baselines = Baseline::parse(&text, &path.display().to_string())?;
    let baseline = baselines.section(record.tier);
    let findings = gate::judge(baseline, record);
    write!(stdout, "{}", table::findings(&findings))?;
    let failed = findings
        .iter()
        .filter(|finding| finding.verdict.fails())
        .count();
    writeln!(
        stdout,
        "perf check: {} series against the {} baseline of {} at {}; {failed} findings fail",
        record.series.len(),
        record.tier.name(),
        baseline.platform,
        baseline
            .binary
            .commit
            .as_deref()
            .unwrap_or("an unknown commit"),
    )?;
    if failed == 0 {
        Ok(())
    } else {
        Err(deps::error(format!("perf check failed: {failed} findings")))
    }
}

#[cfg(test)]
mod tests;
