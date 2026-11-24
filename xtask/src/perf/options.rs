//! `cargo xtask perf` options.

use std::path::PathBuf;

use super::record::Tier;
use crate::{TaskError, deps};

/// What the command does besides measuring, writing and printing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Mode {
    /// Report the binary's numbers.
    #[default]
    Report,
    /// `--check`: compare the binary with a base in the same rounds, the
    /// gate for a change.
    Check,
    /// `--check-baseline`: judge the binary against the stored baseline.
    CheckBaseline,
    /// `--write-baseline`: store the binary's numbers as the baseline.
    WriteBaseline,
    /// `--calibrate`: derive the tier's tolerances.
    Calibrate,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Options {
    pub(super) mode: Mode,
    /// `--full`: every series; without it, the quick tier. A baseline
    /// measures both, whatever this says.
    pub(super) tier: Tier,
    /// `--aws-binary PATH`: measure this binary instead of the default
    /// artifact.
    pub(super) binary: Option<String>,
    /// `--commit REV`: the commit an explicit binary was built from.
    pub(super) commit: Option<String>,
    /// `--base REV`: the commit a check builds its base from.
    pub(super) base: Option<String>,
    /// `--base-binary PATH`, with `--base-commit REV` naming its commit.
    pub(super) base_binary: Option<String>,
    pub(super) base_commit: Option<String>,
    /// `--mainline REF`: the main line whose merge-base is the base.
    pub(super) mainline: Option<String>,
    /// `--repeats N`: a calibration's repeats, over the tier's default.
    pub(super) repeats: Option<u32>,
    /// `--case NAME`, repeatable: only these cases.
    pub(super) cases: Vec<String>,
    /// `--route native|python`, repeatable: only these routes.
    pub(super) routes: Vec<String>,
    /// `--runs N` and `--warmup N`, over the case file's.
    pub(super) runs: Option<u32>,
    pub(super) warmup: Option<u32>,
    /// `--out PATH`: the result file, `target/perf/result.json` by default.
    pub(super) out: Option<PathBuf>,
}

impl Options {
    /// Whether the run measures every case on every route with the case
    /// file's run counts, as a baseline and a calibration must.
    pub(super) fn is_complete(&self) -> bool {
        self.cases.is_empty()
            && self.routes.is_empty()
            && self.runs.is_none()
            && self.warmup.is_none()
    }

    /// The `--aws-binary` arguments `harness::binary` takes.
    pub(super) fn binary_args(&self) -> Vec<String> {
        self.binary
            .iter()
            .flat_map(|path| ["--aws-binary".to_owned(), path.clone()])
            .collect()
    }
}

/// The options that take a value.
const VALUED: [&str; 12] = [
    "--aws-binary",
    "--commit",
    "--base",
    "--base-binary",
    "--base-commit",
    "--mainline",
    "--repeats",
    "--case",
    "--route",
    "--runs",
    "--warmup",
    "--out",
];

pub(super) fn parse(args: &[String]) -> Result<Options, TaskError> {
    let mut options = Options::default();
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let mode = match flag.as_str() {
            "--full" => {
                options.tier = Tier::Full;
                continue;
            }
            "--check" => Mode::Check,
            "--check-baseline" => Mode::CheckBaseline,
            "--write-baseline" => Mode::WriteBaseline,
            "--calibrate" => Mode::Calibrate,
            flag if VALUED.contains(&flag) => {
                let value = args
                    .next()
                    .cloned()
                    .ok_or_else(|| deps::error(format!("{flag} requires a value")))?;
                set(&mut options, flag, value)?;
                continue;
            }
            _ => return Err(deps::error(format!("unknown perf option: {flag}"))),
        };
        if options.mode != Mode::Report {
            return Err(deps::error(
                "--check, --check-baseline, --write-baseline and --calibrate exclude each other",
            ));
        }
        options.mode = mode;
    }
    validate(&options)?;
    Ok(options)
}

fn set(options: &mut Options, flag: &str, value: String) -> Result<(), TaskError> {
    match flag {
        "--aws-binary" => options.binary = Some(value),
        "--commit" => options.commit = Some(value),
        "--base" => options.base = Some(value),
        "--base-binary" => options.base_binary = Some(value),
        "--base-commit" => options.base_commit = Some(value),
        "--mainline" => options.mainline = Some(value),
        "--repeats" => options.repeats = Some(count(flag, &value, 2)?),
        "--case" => options.cases.push(value),
        "--route" if matches!(value.as_str(), "native" | "python") => options.routes.push(value),
        "--route" => {
            return Err(deps::error(format!(
                "--route is native or python, not {value}"
            )));
        }
        "--runs" => options.runs = Some(count(flag, &value, 1)?),
        "--warmup" => options.warmup = Some(count(flag, &value, 0)?),
        _ => options.out = Some(PathBuf::from(value)),
    }
    Ok(())
}

/// The options that only make sense together.
fn validate(options: &Options) -> Result<(), TaskError> {
    let base_named = options.base.is_some() || options.mainline.is_some();
    if (base_named || options.base_binary.is_some()) && options.mode != Mode::Check {
        return Err(deps::error(
            "--base, --base-binary and --mainline choose the base of --check",
        ));
    }
    if options.base_binary.is_some() && base_named {
        return Err(deps::error(
            "--base-binary is the base itself: drop --base and --mainline",
        ));
    }
    if options.base_commit.is_some() && options.base_binary.is_none() {
        return Err(deps::error(
            "--base-commit names the commit of a --base-binary",
        ));
    }
    if options.repeats.is_some() && options.mode != Mode::Calibrate {
        return Err(deps::error("--repeats sets the repeats of --calibrate"));
    }
    let judged = matches!(
        options.mode,
        Mode::Check | Mode::CheckBaseline | Mode::Calibrate
    );
    if judged && options.warmup == Some(0) {
        return Err(deps::error(
            "a judged run needs a warm-up round: the first run of a new binary is slow",
        ));
    }
    Ok(())
}

fn count(flag: &str, value: &str, least: u32) -> Result<u32, TaskError> {
    value
        .parse()
        .ok()
        .filter(|count| *count >= least)
        .ok_or_else(|| deps::error(format!("{flag} takes a whole number of at least {least}")))
}

#[cfg(test)]
mod tests;
