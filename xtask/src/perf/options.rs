//! `cargo xtask perf` options.

use std::path::PathBuf;

use crate::{TaskError, deps};

/// What to do with the result besides writing and printing it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Mode {
    /// Report only.
    #[default]
    Report,
    /// Judge the result against its platform's baseline.
    Check,
    /// Write the result as its platform's baseline.
    WriteBaseline,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Options {
    pub(super) mode: Mode,
    /// `--aws-binary PATH`: measure this binary instead of the default
    /// artifact.
    pub(super) binary: Option<String>,
    /// `--commit REV`: the commit an explicit binary was built from.
    pub(super) commit: Option<String>,
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
    /// file's run counts, as a baseline must.
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
const VALUED: [&str; 7] = [
    "--aws-binary",
    "--commit",
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
            "--check" => Some(Mode::Check),
            "--write-baseline" => Some(Mode::WriteBaseline),
            _ => None,
        };
        if let Some(mode) = mode {
            if options.mode != Mode::Report {
                return Err(deps::error(
                    "--check and --write-baseline exclude each other",
                ));
            }
            options.mode = mode;
            continue;
        }
        if !VALUED.contains(&flag.as_str()) {
            return Err(deps::error(format!("unknown perf option: {flag}")));
        }
        let value = args
            .next()
            .cloned()
            .ok_or_else(|| deps::error(format!("{flag} requires a value")))?;
        set(&mut options, flag, value)?;
    }
    Ok(options)
}

fn set(options: &mut Options, flag: &str, value: String) -> Result<(), TaskError> {
    match flag {
        "--aws-binary" => options.binary = Some(value),
        "--commit" => options.commit = Some(value),
        "--case" => options.cases.push(value),
        "--route" if matches!(value.as_str(), "native" | "python") => options.routes.push(value),
        "--route" => {
            return Err(deps::error(format!(
                "--route is native or python, not {value}"
            )));
        }
        "--runs" => options.runs = Some(count(flag, &value, 1)?),
        "--warmup" => options.warmup = Some(count(flag, &value, 0)?),
        "--out" => options.out = Some(PathBuf::from(value)),
        _ => return Err(deps::error(format!("unknown perf option: {flag}"))),
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
