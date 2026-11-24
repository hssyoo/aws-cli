//! The performance record: what `tests/perf/measure.py` measured, and the
//! result file `cargo xtask perf` writes from it.
//!
//! measure.py writes raw [`Samples`]: one series of runs per case, scheme,
//! latency and route, each run with its wall time, CPU user and system
//! time and peak RSS (from `wait4`), and the stub's counts. [`Record::new`]
//! summarises each series ([`Summary`]) and adds what the harness knows:
//! the platform key the baselines are filed under and the binary's
//! commit. The result file is a `Record` with its runs; a baseline is one
//! without ([`Record::without_runs`]).

use serde::{Deserialize, Serialize};

use super::stats::Spread;
use crate::{TaskError, deps};

/// The version of both files' layout; a reader refuses any other.
pub(super) const FORMAT: u32 = 1;

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
    #[serde(flatten)]
    pub(super) key: Key,
    pub(super) pages: u32,
    /// The least and greatest one-minute load average seen before the
    /// series' runs.
    pub(super) load_average: [f64; 2],
    pub(super) runs: Vec<Run>,
}

/// measure.py's output file.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(super) struct Samples {
    pub(super) format: u32,
    pub(super) host: Host,
    pub(super) started_unix_s: f64,
    pub(super) warmup: u32,
    pub(super) runs: u32,
    pub(super) series: Vec<SampleSeries>,
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
    pub(super) started_unix_s: f64,
    pub(super) warmup: u32,
    pub(super) runs: u32,
    pub(super) series: Vec<Series>,
}

impl Record {
    /// The record of `samples`, which must be measure.py's current format
    /// and hold at least one run in every series.
    pub(super) fn new(samples: Samples, platform: &str, binary: Binary) -> Result<Self, TaskError> {
        if samples.format != FORMAT {
            return Err(deps::error(format!(
                "perf samples have format {}, expected {FORMAT}",
                samples.format
            )));
        }
        let series = samples
            .series
            .into_iter()
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
        Ok(Self {
            format: FORMAT,
            platform: platform.to_owned(),
            host: samples.host,
            binary,
            started_unix_s: samples.started_unix_s,
            warmup: samples.warmup,
            runs: samples.runs,
            series,
        })
    }

    /// The record read from `text`, refusing another format.
    pub(super) fn parse(text: &str, origin: &str) -> Result<Self, TaskError> {
        let record: Self = serde_json::from_str(text)
            .map_err(|error| deps::error(format!("{origin} is not a perf record: {error}")))?;
        if record.format != FORMAT {
            return Err(deps::error(format!(
                "{origin} has format {}, expected {FORMAT}",
                record.format
            )));
        }
        Ok(record)
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

#[cfg(test)]
mod tests;
