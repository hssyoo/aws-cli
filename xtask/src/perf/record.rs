//! The performance record: what `tests/perf/measure.py` measured, and the
//! result file `cargo xtask perf` writes from it.
//!
//! measure.py writes raw [`Samples`]: for each binary it measured, one
//! series of runs per case, scheme, latency and route, each run with its
//! wall time, CPU user and system time and peak RSS (from `wait4`), the
//! load average before it, and the stub's counts. [`Record::new`] takes
//! one binary's series and summarises each ([`Summary`]), adding what the
//! harness knows: the platform key baselines are filed under and the
//! binary's commit. A result file holds one `Record` with its runs, or two
//! for a comparison (`ab`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::stats::Spread;
use crate::{TaskError, deps};

/// The version of the result file's layout; a reader refuses any other.
pub(super) const FORMAT: u32 = 3;
/// The version of measure.py's samples.
pub(super) const SAMPLES_FORMAT: u32 = 2;

/// Which matrix a run measured: the quick tier, a subset of the cases'
/// series that the default check runs, or the full tier, every series,
/// which a baseline holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Tier {
    #[default]
    Quick,
    Full,
}

impl Tier {
    /// The name measure.py's `--tier` takes.
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Quick => "quick",
            Self::Full => "full",
        }
    }
}

/// One run of a command, as measure.py reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Run {
    pub(super) wall_s: f64,
    pub(super) user_s: f64,
    pub(super) sys_s: f64,
    pub(super) max_rss_bytes: f64,
    pub(super) connections: u32,
    pub(super) requests: u32,
    pub(super) response_bytes: f64,
    pub(super) stdout_bytes: f64,
    /// The one-minute load average just before the run.
    pub(super) load_average: f64,
}

/// The machine the runs were on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Host {
    pub(super) system: String,
    pub(super) release: String,
    pub(super) machine: String,
    pub(super) os: String,
    pub(super) cpu: String,
    pub(super) cpus: u32,
}

/// What a series measured: one case, over one scheme at one injected
/// latency, on one route.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(super) struct Key {
    pub(super) case: String,
    pub(super) scheme: String,
    pub(super) latency_ms: u32,
    pub(super) route: String,
}

impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            case,
            scheme,
            latency_ms,
            route,
        } = self;
        write!(f, "{case} {scheme} {latency_ms} ms {route}")
    }
}

/// One series as measure.py reports it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(super) struct SampleSeries {
    /// The name measure.py's `--binary` gave the binary it ran.
    pub(super) binary: String,
    #[serde(flatten)]
    pub(super) key: Key,
    pub(super) pages: u32,
    /// The least and greatest one-minute load average seen before the
    /// series' runs.
    pub(super) load_average: [f64; 2],
    pub(super) runs: Vec<Run>,
}

/// A series a binary refused in its first round, which a comparison
/// reports and does not judge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Refusal {
    #[serde(flatten)]
    pub(super) key: Key,
    /// The refused run's first line of diagnosis.
    pub(super) reason: String,
}

/// A refusal as measure.py reports it, with its binary's name.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(super) struct SampleRefusal {
    pub(super) binary: String,
    #[serde(flatten)]
    pub(super) refusal: Refusal,
}

/// measure.py's output file.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(super) struct Samples {
    pub(super) format: u32,
    pub(super) tier: Tier,
    pub(super) host: Host,
    pub(super) started_unix_s: f64,
    pub(super) warmup: u32,
    pub(super) runs: u32,
    pub(super) series: Vec<SampleSeries>,
    pub(super) refused: Vec<SampleRefusal>,
    /// Each binary's first run, which the warm-up rounds absorb.
    pub(super) first_run_s: BTreeMap<String, f64>,
}

/// A count that must not vary: its least and greatest value over the runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Exact {
    pub(super) min: u32,
    pub(super) max: u32,
}

impl Exact {
    fn of(values: impl Iterator<Item = u32> + Clone) -> Self {
        Self {
            min: values.clone().min().unwrap_or_default(),
            max: values.max().unwrap_or_default(),
        }
    }
}

/// A series' measurements, summarised. Throughputs are over the median
/// wall time; MB are 10^6 bytes of response body and framing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Summary {
    pub(super) wall_s: Spread,
    /// CPU user plus system time.
    pub(super) cpu_s: Spread,
    pub(super) user_s: Spread,
    pub(super) sys_s: Spread,
    pub(super) max_rss_bytes: Spread,
    pub(super) connections: Exact,
    pub(super) requests: Exact,
    pub(super) response_bytes: f64,
    pub(super) stdout_bytes: f64,
    pub(super) requests_per_s: f64,
    pub(super) pages_per_s: f64,
    pub(super) mb_per_s: f64,
}

impl Summary {
    /// The summary of `runs` of a `pages`-page command; `None` without runs.
    fn of(runs: &[Run], pages: u32) -> Option<Self> {
        let spread =
            |measure: fn(&Run) -> f64| Spread::of(&runs.iter().map(measure).collect::<Vec<_>>());
        let wall_s = spread(|run| run.wall_s)?;
        let requests = Exact::of(runs.iter().map(|run| run.requests));
        let response_bytes = spread(|run| run.response_bytes)?.p50;
        let per_second = |amount: f64| amount / wall_s.p50;
        Some(Self {
            wall_s,
            cpu_s: spread(|run| run.user_s + run.sys_s)?,
            user_s: spread(|run| run.user_s)?,
            sys_s: spread(|run| run.sys_s)?,
            max_rss_bytes: spread(|run| run.max_rss_bytes)?,
            connections: Exact::of(runs.iter().map(|run| run.connections)),
            requests,
            response_bytes,
            stdout_bytes: spread(|run| run.stdout_bytes)?.p50,
            requests_per_s: per_second(f64::from(requests.max)),
            pages_per_s: per_second(f64::from(pages)),
            mb_per_s: per_second(response_bytes / 1e6),
        })
    }
}

/// One series in the result file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Series {
    #[serde(flatten)]
    pub(super) key: Key,
    pub(super) pages: u32,
    pub(super) load_average: [f64; 2],
    pub(super) summary: Summary,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) runs: Vec<Run>,
}

/// The binary measured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Binary {
    pub(super) path: String,
    /// The commit it was built from, when known: the worktree's `HEAD` for
    /// the default artifact (which must be current), `--commit` for an
    /// explicit one.
    pub(super) commit: Option<String>,
    /// Whether the worktree had uncommitted changes to tracked files.
    pub(super) dirty: Option<bool>,
}

/// The result file, and a baseline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Record {
    pub(super) format: u32,
    /// `<os>-<arch>` as Rust names them, the key baselines are filed under.
    pub(super) platform: String,
    pub(super) host: Host,
    pub(super) binary: Binary,
    pub(super) tier: Tier,
    pub(super) started_unix_s: f64,
    pub(super) warmup: u32,
    pub(super) runs: u32,
    /// The binary's first run, before the warm-up rounds ended.
    pub(super) first_run_s: Option<f64>,
    pub(super) series: Vec<Series>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) refused: Vec<Refusal>,
}

impl Record {
    /// The record of the binary measure.py named `name` in `samples`,
    /// which must be measure.py's current format, of `tier`, and hold at
    /// least one run in every series.
    pub(super) fn new(
        samples: &Samples,
        name: &str,
        platform: &str,
        binary: Binary,
        tier: Tier,
    ) -> Result<Self, TaskError> {
        if samples.format != SAMPLES_FORMAT {
            return Err(deps::error(format!(
                "perf samples have format {}, expected {SAMPLES_FORMAT}",
                samples.format
            )));
        }
        if samples.tier != tier {
            return Err(deps::error(format!(
                "perf samples are of the {} tier, not the {} tier",
                samples.tier.name(),
                tier.name()
            )));
        }
        let series = samples
            .series
            .iter()
            .filter(|series| series.binary == name)
            .cloned()
            .map(|series| {
                let summary = Summary::of(&series.runs, series.pages).ok_or_else(|| {
                    deps::error(format!("perf series {} has no runs", series.key))
                })?;
                Ok(Series {
                    key: series.key,
                    pages: series.pages,
                    load_average: series.load_average,
                    summary,
                    runs: series.runs,
                })
            })
            .collect::<Result<_, TaskError>>()?;
        let refused = samples
            .refused
            .iter()
            .filter(|refusal| refusal.binary == name)
            .map(|refusal| refusal.refusal.clone())
            .collect();
        Ok(Self {
            format: FORMAT,
            platform: platform.to_owned(),
            host: samples.host.clone(),
            binary,
            tier,
            started_unix_s: samples.started_unix_s,
            warmup: samples.warmup,
            runs: samples.runs,
            first_run_s: samples.first_run_s.get(name).copied(),
            series,
            refused,
        })
    }

    /// The record as a baseline: the summaries, without the runs.
    pub(super) fn without_runs(&self) -> Self {
        let mut baseline = self.clone();
        for series in &mut baseline.series {
            series.runs.clear();
        }
        baseline
    }

    /// The record as the pretty JSON both files hold.
    pub(super) fn to_json(&self) -> Result<String, TaskError> {
        serde_json::to_string_pretty(self)
            .map(|text| text + "\n")
            .map_err(|error| deps::error(format!("perf record: {error}")))
    }
}

/// A comparison's result file: the base's and the head's records, with
/// their runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Comparison {
    pub(super) format: u32,
    pub(super) base: Record,
    pub(super) head: Record,
}

impl Comparison {
    pub(super) fn new(base: Record, head: Record) -> Self {
        Self {
            format: FORMAT,
            base,
            head,
        }
    }

    /// The comparison as pretty JSON.
    pub(super) fn to_json(&self) -> Result<String, TaskError> {
        serde_json::to_string_pretty(self)
            .map(|text| text + "\n")
            .map_err(|error| deps::error(format!("perf comparison: {error}")))
    }
}

#[cfg(test)]
mod tests;
