//! The performance verdicts: a head binary judged against a base binary
//! measured in the same rounds ([`judge_comparison`], the gate for a
//! change), or a result judged against the platform's stored baseline
//! ([`judge_baseline`], absolute tracking).
//!
//! Counts are judged exactly, in both: connections opened and requests
//! made must equal the reference's in every run, since a count has no
//! noise and a changed one is a changed behaviour. Times and memory are
//! judged against [`Tolerances`] that `cargo xtask perf --calibrate` wrote
//! into the platform's baseline file:
//!
//! - **Wall and CPU time** (CPU on series without injected latency only:
//!   a process that idles through a round trip is rescheduled onto a
//!   slower core or a lower clock, so its CPU time follows the machine).
//!   A comparison pairs each head run with the base run of the same
//!   round, so drift on the machine, which moves both, cancels, and
//!   judges the median of the runs' log ratios. A baseline check judges
//!   the median against the stored one. Either way, an estimate whose 95%
//!   resolution (twice its standard error, from the deviation) is wider
//!   than the tolerance cannot tell a regression from noise and is
//!   reported as too noisy to judge.
//! - **Peak RSS** on the least peak of the runs. A regression in peak
//!   memory (a larger buffer, pages kept alive, a leak) adds to every run's
//!   peak, so it raises the least one; memory that depends on timing (a
//!   buffer that grows while its reader lags, a reallocation that cannot
//!   grow in place) adds to some runs only, and moves the median or p95
//!   between modes without the code changing. The least peak keeps the
//!   first and ignores the second. It misses a regression that occurs in
//!   only some runs.
//!
//! **Load.** A series whose runs saw a median load average above the
//! calibrated limit ([`LoadLimit`]) has its times and memory reported as
//! measured on a loaded machine, not judged; its counts are still judged.
//!
//! A verdict either passes, fails ([`Verdict::fails`]) or cannot judge
//! ([`Verdict::inconclusive`]), and the check exits accordingly.

use serde::{Deserialize, Serialize};

use super::record::{Exact, Key, Record, Run, Series};
use super::stats::{Spread, percentile};

/// How far a measurement may grow: by the larger of `relative` times the
/// reference and `absolute`, in the metric's unit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(super) struct Tolerance {
    pub(super) relative: f64,
    pub(super) absolute: f64,
}

impl Tolerance {
    pub(super) fn limit(self, reference: f64) -> f64 {
        (self.relative * reference).max(self.absolute)
    }
}

/// One mode's tolerances for a tier: wall time and CPU user plus system
/// time (seconds), and the least peak RSS (bytes).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(super) struct Tolerances {
    pub(super) wall: Tolerance,
    pub(super) cpu: Tolerance,
    pub(super) rss: Tolerance,
}

/// The load average above which times and memory are not judged: the
/// highest the calibration saw, plus `headroom` times the CPU count.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct LoadLimit {
    pub(super) calibrated: f64,
    pub(super) cpus: u32,
    pub(super) headroom: f64,
}

impl LoadLimit {
    pub(super) fn value(self) -> f64 {
        self.calibrated + self.headroom * f64::from(self.cpus)
    }
}

/// A comparison's headroom: one runnable process per CPU above the
/// calibration's load. The paired ratios cancel what load does to both
/// binaries alike; the proof under load in `tests/perf/README.md` holds at
/// this limit.
pub(super) const COMPARISON_HEADROOM: f64 = 1.0;
/// A baseline check's headroom: a quarter of the CPUs. A stored baseline
/// has no same-run reference, so load moves the medians it compares.
pub(super) const BASELINE_HEADROOM: f64 = 0.25;

/// Why a measurement was not judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Unjudged {
    /// The baseline was taken on another CPU model.
    OtherCpu,
    /// The platform's baseline file holds no calibration for the tier.
    Uncalibrated,
}

/// One judged measurement's outcome.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Verdict {
    Within,
    /// Grew past its limit.
    Regressed {
        baseline: f64,
        now: f64,
        limit: f64,
    },
    /// Shrank past its limit.
    Improved {
        baseline: f64,
        now: f64,
        limit: f64,
    },
    /// The runs cannot resolve the limit.
    Noisy {
        resolution: f64,
        limit: f64,
    },
    /// The machine was loaded past the limit: not judged.
    Loaded {
        load: f64,
        limit: f64,
    },
    /// A count differs from the reference's, or varied between runs.
    Changed {
        baseline: Exact,
        now: Exact,
    },
    NotJudged(Unjudged),
    /// The reference has no such series.
    New,
    /// A complete run lacks a series the baseline has.
    Missing,
    /// The base binary refused the series, with its reason.
    RefusedByBase(String),
}

impl Verdict {
    /// Whether the verdict fails the check.
    pub(super) fn fails(&self) -> bool {
        matches!(
            self,
            Self::Regressed { .. } | Self::Changed { .. } | Self::Missing
        )
    }

    /// Whether the check could not judge the measurement.
    pub(super) fn inconclusive(&self) -> bool {
        matches!(self, Self::Noisy { .. } | Self::Loaded { .. })
    }
}

/// The verdict on one measurement of one series.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Finding {
    pub(super) key: Key,
    pub(super) metric: Metric,
    pub(super) verdict: Verdict,
    /// The least and greatest load average seen before the series' runs.
    pub(super) load_average: [f64; 2],
}

/// The judged measurements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Metric {
    Series,
    Connections,
    Requests,
    Wall,
    Cpu,
    Rss,
}

impl Metric {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Series => "series",
            Self::Connections => "connections",
            Self::Requests => "requests",
            Self::Wall => "wall p50",
            Self::Cpu => "cpu p50",
            Self::Rss => "least peak RSS",
        }
    }
}

/// How a timed or memory metric is judged once the series is.
#[derive(Debug, Clone, Copy)]
enum Judge {
    /// With these tolerances, unless the load passed this limit.
    By(Tolerances, Option<f64>),
    /// Not at all, for this reason.
    Not(Unjudged),
}

/// Every finding on `head` against `base`, two binaries measured in the
/// same rounds, judged with `tolerances` (none when uncalibrated) and the
/// load `limit`.
pub(super) fn judge_comparison(
    base: &Record,
    head: &Record,
    tolerances: Option<Tolerances>,
    limit: Option<f64>,
) -> Vec<Finding> {
    let judge = tolerances.map_or(Judge::Not(Unjudged::Uncalibrated), |tolerances| {
        Judge::By(tolerances, limit)
    });
    let mut findings: Vec<Finding> = head
        .series
        .iter()
        .filter(|now| !base.refused.iter().any(|refusal| refusal.key == now.key))
        .flat_map(|now| {
            let reference = base.series.iter().find(|series| series.key == now.key);
            judge_series(reference, now, judge, Reference::Runs)
        })
        .collect();
    findings.extend(base_refusals(base, head));
    findings
}

/// A finding for each series the head ran and the base refused.
fn base_refusals<'a>(base: &'a Record, head: &'a Record) -> impl Iterator<Item = Finding> + 'a {
    base.refused.iter().map(move |refusal| Finding {
        key: refusal.key.clone(),
        metric: Metric::Series,
        verdict: Verdict::RefusedByBase(refusal.reason.clone()),
        load_average: head
            .series
            .iter()
            .find(|series| series.key == refusal.key)
            .map_or([0.0; 2], |series| series.load_average),
    })
}

/// Every finding on `current` against the stored `baseline` of its tier.
/// Times and memory are judged with `tolerances` (none when the platform
/// is uncalibrated) only on the baseline's CPU model; a `complete` run
/// must have every series the baseline has.
pub(super) fn judge_baseline(
    baseline: &Record,
    current: &Record,
    tolerances: Option<Tolerances>,
    limit: Option<f64>,
    complete: bool,
) -> Vec<Finding> {
    let judge = match tolerances {
        _ if baseline.host.cpu != current.host.cpu => Judge::Not(Unjudged::OtherCpu),
        None => Judge::Not(Unjudged::Uncalibrated),
        Some(tolerances) => Judge::By(tolerances, limit),
    };
    let mut findings: Vec<Finding> = current
        .series
        .iter()
        .flat_map(|now| {
            let base = baseline.series.iter().find(|base| base.key == now.key);
            judge_series(base, now, judge, Reference::Summary)
        })
        .collect();
    if complete {
        let missing = baseline
            .series
            .iter()
            .filter(|base| !current.series.iter().any(|now| now.key == base.key))
            .map(|base| Finding {
                key: base.key.clone(),
                metric: Metric::Series,
                verdict: Verdict::Missing,
                load_average: base.load_average,
            });
        findings.extend(missing);
    }
    findings
}

/// What a series is compared with: the base's runs of the same rounds, or
/// a stored summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reference {
    Runs,
    Summary,
}

fn judge_series(
    base: Option<&Series>,
    now: &Series,
    judge: Judge,
    reference: Reference,
) -> Vec<Finding> {
    let finding = |metric, verdict| Finding {
        key: now.key.clone(),
        metric,
        verdict,
        load_average: now.load_average,
    };
    let Some(base) = base else {
        return vec![finding(Metric::Series, Verdict::New)];
    };
    let (b, n) = (&base.summary, &now.summary);
    let mut findings = vec![
        finding(Metric::Connections, exact(b.connections, n.connections)),
        finding(Metric::Requests, exact(b.requests, n.requests)),
    ];
    let idle = now.key.latency_ms != 0;
    let metrics = [Metric::Wall, Metric::Cpu, Metric::Rss];
    for metric in metrics
        .into_iter()
        .filter(|metric| !(*metric == Metric::Cpu && idle))
    {
        let verdict = match judge {
            Judge::Not(reason) => Verdict::NotJudged(reason),
            Judge::By(tolerances, limit) => loaded(now, limit)
                .unwrap_or_else(|| measured(base, now, metric, tolerances, reference)),
        };
        findings.push(finding(metric, verdict));
    }
    findings
}

/// The verdict on a loaded series: its runs' median load average passed
/// `limit`.
fn loaded(series: &Series, limit: Option<f64>) -> Option<Verdict> {
    let limit = limit?;
    let loads: Vec<f64> = series.runs.iter().map(|run| run.load_average).collect();
    let load = percentile(&loads, 0.5)?;
    (load > limit).then_some(Verdict::Loaded { load, limit })
}

fn measured(
    base: &Series,
    now: &Series,
    metric: Metric,
    tolerances: Tolerances,
    reference: Reference,
) -> Verdict {
    let (b, n) = (&base.summary, &now.summary);
    match (metric, reference) {
        (Metric::Rss, _) => least(b.max_rss_bytes, n.max_rss_bytes, tolerances.rss),
        (Metric::Cpu, Reference::Runs) => paired(&base.runs, &now.runs, cpu, tolerances.cpu),
        (Metric::Cpu, Reference::Summary) => median(b.cpu_s, n.cpu_s, tolerances.cpu),
        (_, Reference::Runs) => paired(&base.runs, &now.runs, wall, tolerances.wall),
        (_, Reference::Summary) => median(b.wall_s, n.wall_s, tolerances.wall),
    }
}

/// A run's wall time.
pub(super) fn wall(run: &Run) -> f64 {
    run.wall_s
}

/// A run's CPU user plus system time.
pub(super) fn cpu(run: &Run) -> f64 {
    run.user_s + run.sys_s
}

/// A count's verdict: it must not vary, and must equal the reference's.
fn exact(base: Exact, now: Exact) -> Verdict {
    if base == now && now.min == now.max {
        Verdict::Within
    } else {
        Verdict::Changed {
            baseline: base,
            now,
        }
    }
}

/// The verdict on a change from `baseline` to `now` against `limit`, for
/// a measurement that resolves `resolution`.
fn growth(baseline: f64, now: f64, limit: f64, resolution: f64) -> Verdict {
    if resolution > limit {
        Verdict::Noisy { resolution, limit }
    } else if now - baseline > limit {
        Verdict::Regressed {
            baseline,
            now,
            limit,
        }
    } else if baseline - now > limit {
        Verdict::Improved {
            baseline,
            now,
            limit,
        }
    } else {
        Verdict::Within
    }
}

/// A median's verdict against a stored one, under `tolerance`.
pub(super) fn median(base: Spread, now: Spread, tolerance: Tolerance) -> Verdict {
    let resolution = 2.0 * base.median_error().hypot(now.median_error());
    growth(base.p50, now.p50, tolerance.limit(base.p50), resolution)
}

/// The least peak's verdict, under `tolerance`. No resolution is
/// estimated: the least of the runs does not move with the modes timing
/// gives the rest.
pub(super) fn least(base: Spread, now: Spread, tolerance: Tolerance) -> Verdict {
    growth(base.min, now.min, tolerance.limit(base.min), 0.0)
}

/// The verdict on the head's runs paired with the base's runs of the same
/// rounds: the median of their log ratios, scaled to the base's median,
/// under `tolerance`.
pub(super) fn paired(
    base: &[Run],
    head: &[Run],
    measure: fn(&Run) -> f64,
    tolerance: Tolerance,
) -> Verdict {
    let ratios: Vec<f64> = base
        .iter()
        .zip(head)
        .map(|(base, head)| (measure(head) / measure(base)).ln())
        .collect();
    let base_values: Vec<f64> = base.iter().map(measure).collect();
    let (Some(spread), Some(reference)) = (Spread::of(&ratios), percentile(&base_values, 0.5))
    else {
        // No pair of runs: nothing resolves the limit.
        return Verdict::Noisy {
            resolution: f64::INFINITY,
            limit: tolerance.absolute,
        };
    };
    let now = reference * spread.p50.exp();
    let resolution = reference * ((2.0 * spread.median_error()).exp() - 1.0);
    growth(reference, now, tolerance.limit(reference), resolution)
}

#[cfg(test)]
mod tests;
