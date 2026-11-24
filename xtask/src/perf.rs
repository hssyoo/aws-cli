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
//! anything but the stubs: each runs with the stub as `--endpoint-url` and
//! `AWS_ENDPOINT_URL`, fake keys and a scratch HOME.
//!
//! The binary measured is the default artifact, which must be present and
//! current as for `blackbox` (`harness::binary`), or `--aws-binary PATH`
//! (with `--commit REV` naming what it was built from). Its result file is
//! `target/perf/result.json`, or `--out PATH`, and a table is printed
//! ([`table`]). The modes:
//!
//! - `--check`, **the gate for a change**: the binary (the head) and a
//!   base ([`base`]) run interleaved in the same rounds, and the head is
//!   judged on its ratio to the base ([`gate::judge_comparison`]), so what
//!   the machine does to both cancels.
//! - `--check-baseline`: the binary judged against the platform's stored
//!   record ([`gate::judge_baseline`]), for absolute tracking.
//! - `--write-baseline`: the full tier and then the quick tier stored as
//!   that record; it takes a complete run of a committed head.
//! - `--calibrate`: the tier's tolerances derived from comparisons of the
//!   binary with a byte-identical copy of it ([`calibrate`]).
//!
//! The stored records and the tolerances live in the platform's baseline
//! file, `tests/perf/baselines/<os>-<arch>.json` ([`baseline`]).
//! `tests/perf/README.md` holds the procedures and what the gate catches.

mod base;
mod baseline;
mod calibrate;
mod gate;
mod options;
mod record;
mod stats;
mod table;

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{TaskError, deps, harness, process};
use base::Base;
use baseline::{Baseline, Calibration};
use gate::{BASELINE_HEADROOM, COMPARISON_HEADROOM, Finding, LoadLimit};
use options::{Mode, Options};
use record::{Binary, Comparison, Record, Samples, Tier};

/// What every mode works with.
struct Context<'a> {
    root: &'a Path,
    options: &'a Options,
    /// The binary measured and what it was built from.
    binary: PathBuf,
    identity: Binary,
    /// The result file.
    out: PathBuf,
    /// The platform's baseline file.
    baseline: PathBuf,
}

pub fn run(root: &Path, args: &[String]) -> Result<(), TaskError> {
    let options = options::parse(args)?;
    let stores = matches!(options.mode, Mode::WriteBaseline | Mode::Calibrate);
    if stores && !options.is_complete() {
        return Err(deps::error(
            "a baseline or a calibration takes every case on both routes with the case \
             file's run counts: drop --case, --route, --runs and --warmup",
        ));
    }
    let binary = harness::binary(root, &options.binary_args())?;
    let identity = identity(root, &binary, &options)?;
    if stores && (identity.commit.is_none() || identity.dirty != Some(false)) {
        return Err(deps::error(
            "a baseline or a calibration is measured on the default artifact of a committed \
             head, with no uncommitted changes",
        ));
    }
    let context = Context {
        root,
        options: &options,
        binary,
        identity,
        out: options.out.as_ref().map_or_else(
            || root.join("target/perf/result.json"),
            |path| root.join(path),
        ),
        baseline: root
            .join("tests/perf/baselines")
            .join(format!("{}.json", platform())),
    };
    match options.mode {
        Mode::Report => take(&context, options.tier, &context.out).map(drop),
        Mode::Check => check(&context),
        Mode::CheckBaseline => check_baseline(&context),
        Mode::WriteBaseline => write_baseline(&context),
        Mode::Calibrate => calibration(&context),
    }
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

/// One measurement of the binary at `tier`: the record, written to `out`
/// and printed as a table.
fn take(context: &Context, tier: Tier, out: &Path) -> Result<Record, TaskError> {
    let binaries = [("head", context.binary.as_path())];
    let samples = measure(context.root, &binaries, context.options, tier)?;
    let record = Record::new(
        &samples,
        "head",
        &platform(),
        context.identity.clone(),
        tier,
    )?;
    fs::write(out, record.to_json()?)?;
    let mut stdout = io::stdout().lock();
    write!(stdout, "{}", table::render(&record))?;
    writeln!(stdout, "perf: result in {}", out.display())?;
    Ok(record)
}

/// The binary and `base` measured in the same rounds at `tier`: their
/// records, written to `out` and printed as a table.
fn compare(
    context: &Context,
    base: &Base,
    tier: Tier,
    out: &Path,
) -> Result<(Record, Record), TaskError> {
    let binaries = [
        ("base", base.binary.as_path()),
        ("head", context.binary.as_path()),
    ];
    let samples = measure(context.root, &binaries, context.options, tier)?;
    let base_identity = Binary {
        path: shown(context.root, &base.binary),
        commit: base.commit.clone(),
        dirty: None,
    };
    let base = Record::new(&samples, "base", &platform(), base_identity, tier)?;
    let head = Record::new(
        &samples,
        "head",
        &platform(),
        context.identity.clone(),
        tier,
    )?;
    let comparison = Comparison::new(base, head);
    comparison.check_pairs()?;
    fs::write(out, comparison.to_json()?)?;
    let mut stdout = io::stdout().lock();
    write!(
        stdout,
        "{}",
        table::render_comparison(&comparison.base, &comparison.head)
    )?;
    writeln!(stdout, "perf: result in {}", out.display())?;
    Ok((comparison.base, comparison.head))
}

/// The samples measure.py takes of `binaries`, each with the name its
/// series carry; a base, if any, may refuse a series. The runs hold the
/// local CLI test lock, so no other CLI suite of this worktree loads the
/// machine or contends for the CRT's process lock meanwhile.
fn measure(
    root: &Path,
    binaries: &[(&str, &Path)],
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
    let mut command = measure_command(root, [&directory, &samples], options, tier);
    for (name, binary) in binaries {
        command
            .arg("--binary")
            .arg(format!("{name}={}", binary.display()));
        if *name == "base" {
            command.arg("--optional").arg(name);
        }
    }
    process::run(&mut command)?;
    let text = fs::read_to_string(&samples)?;
    serde_json::from_str(&text).map_err(|error| {
        deps::error(format!(
            "{} is not perf samples: {error}",
            samples.display()
        ))
    })
}

/// measure.py's command at `tier`, with its venv and scratch under
/// `directory` and its samples written to `samples`.
fn measure_command(
    root: &Path,
    [directory, samples]: [&Path; 2],
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

/// The tier's calibration in the platform's baseline file, saying so when
/// there is none.
fn calibrated(context: &Context, tier: Tier) -> Result<Option<Calibration>, TaskError> {
    let calibration = Baseline::read(&context.baseline)?
        .and_then(|baseline| baseline.section(tier).calibration.clone());
    if let Some(calibration) = &calibration {
        write!(io::stdout(), "{}", table::calibration(tier, calibration))?;
    } else {
        writeln!(
            io::stdout(),
            "perf: {} has no {} calibration: counts are judged, times and memory are not; \
             `cargo xtask perf --calibrate{}` calibrates it",
            context.baseline.display(),
            tier.name(),
            if tier == Tier::Full { " --full" } else { "" },
        )?;
    }
    Ok(calibration)
}

/// `--check`: the binary against the base, in the same rounds.
fn check(context: &Context) -> Result<(), TaskError> {
    let tier = context.options.tier;
    let base = base::resolve(context.root, context.options)?;
    let (base, head) = compare(context, &base, tier, &context.out)?;
    let calibration = calibrated(context, tier)?;
    let limit = calibration.as_ref().map(|calibration| LoadLimit {
        calibrated: calibration.load_average[1],
        cpus: head.host.cpus,
        headroom: COMPARISON_HEADROOM,
    });
    let tolerances = calibration.map(|calibration| calibration.comparison);
    let findings = gate::judge_comparison(&base, &head, tolerances, limit.map(LoadLimit::value));
    let against = format!(
        "the base {}",
        base.binary.commit.as_deref().unwrap_or(&base.binary.path)
    );
    conclude(&findings, head.series.len(), tier, &against)
}

/// `--check-baseline`: the binary against the stored record.
fn check_baseline(context: &Context) -> Result<(), TaskError> {
    let tier = context.options.tier;
    let record = take(context, tier, &context.out)?;
    let stored = Baseline::read(&context.baseline)?
        .and_then(|baseline| baseline.section(tier).record.clone());
    let Some(stored) = stored else {
        writeln!(
            io::stdout(),
            "perf: {} has no {} record: the result is reported, not judged",
            context.baseline.display(),
            tier.name()
        )?;
        return Ok(());
    };
    let calibration = calibrated(context, tier)?;
    let limit = calibration.as_ref().map(|calibration| LoadLimit {
        calibrated: calibration.load_average[1],
        cpus: record.host.cpus,
        headroom: BASELINE_HEADROOM,
    });
    let tolerances = calibration.map(|calibration| calibration.baseline);
    let complete = context.options.is_complete();
    let limit = limit.map(LoadLimit::value);
    let findings = gate::judge_baseline(&stored, &record, tolerances, limit, complete);
    let against = format!(
        "the stored record of {}",
        stored
            .binary
            .commit
            .as_deref()
            .unwrap_or("an unknown commit")
    );
    conclude(&findings, record.series.len(), tier, &against)
}

/// Print the findings that are not within bounds and the outcome: an
/// error when any fails, an inconclusive one when any could not be judged.
fn conclude(
    findings: &[Finding],
    series: usize,
    tier: Tier,
    against: &str,
) -> Result<(), TaskError> {
    let failed = findings
        .iter()
        .filter(|finding| finding.verdict.fails())
        .count();
    let unjudged = findings
        .iter()
        .filter(|finding| finding.verdict.inconclusive())
        .count();
    let mut stdout = io::stdout().lock();
    write!(stdout, "{}", table::findings(findings))?;
    writeln!(
        stdout,
        "perf check: {series} {} series against {against}; {failed} findings fail, \
         {unjudged} cannot be judged",
        tier.name()
    )?;
    if failed > 0 {
        Err(deps::error(format!("perf check failed: {failed} findings")))
    } else if unjudged > 0 {
        Err(TaskError::Inconclusive(format!(
            "perf check could not judge {unjudged} findings: the machine was loaded or the \
             runs too noisy; rerun on a quieter machine"
        )))
    } else {
        Ok(())
    }
}

/// `--write-baseline`: both tiers' records, stored.
fn write_baseline(context: &Context) -> Result<(), TaskError> {
    let full = take(context, Tier::Full, &context.out)?;
    let quick = take(
        context,
        Tier::Quick,
        &context.out.with_extension("quick.json"),
    )?;
    let mut baseline = Baseline::read(&context.baseline)?.unwrap_or_default();
    baseline.full.record = Some(full.without_runs());
    baseline.quick.record = Some(quick.without_runs());
    baseline.write(&context.baseline)?;
    writeln!(
        io::stdout(),
        "perf: baseline written to {}",
        context.baseline.display()
    )?;
    Ok(())
}

/// The repeats a calibration of `tier` runs by default.
fn default_repeats(tier: Tier) -> u32 {
    match tier {
        Tier::Quick => 5,
        Tier::Full => 3,
    }
}

/// `--calibrate`: comparisons of the binary with a copy of itself, and the
/// tier's tolerances derived from them, stored.
fn calibration(context: &Context) -> Result<(), TaskError> {
    let tier = context.options.tier;
    let repeats = context.options.repeats.unwrap_or(default_repeats(tier));
    let directory = context.root.join("target/perf/calibration");
    fs::create_dir_all(&directory)?;
    // A byte-identical copy at another path, as base: a calibration then
    // measures two binaries as a check does, not one file run twice.
    let copy = directory
        .join("aws")
        .with_extension(std::env::consts::EXE_EXTENSION);
    fs::copy(&context.binary, &copy)?;
    let itself = Base {
        binary: copy,
        commit: context.identity.commit.clone(),
    };
    let mut pairs = Vec::new();
    for repeat in 1..=repeats {
        let out = directory.join(format!("{}-{repeat}.json", tier.name()));
        pairs.push(compare(context, &itself, tier, &out)?);
    }
    let (comparison, baseline_tolerances) = calibrate::derive(&pairs);
    let host = pairs
        .first()
        .map(|(_, head)| head.host.clone())
        .ok_or_else(|| deps::error("a calibration needs a repeat"))?;
    let unix_s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64());
    let calibration = Calibration {
        unix_s,
        commit: context.identity.commit.clone(),
        host,
        load_average: calibrate::load_range(&pairs),
        repeats,
        comparison,
        baseline: baseline_tolerances,
    };
    let mut baseline = Baseline::read(&context.baseline)?.unwrap_or_default();
    write!(io::stdout(), "{}", table::calibration(tier, &calibration))?;
    baseline.section_mut(tier).calibration = Some(calibration);
    baseline.write(&context.baseline)?;
    writeln!(
        io::stdout(),
        "perf: calibration written to {}",
        context.baseline.display()
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
