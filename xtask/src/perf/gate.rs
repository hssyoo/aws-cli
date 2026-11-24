//! `cargo xtask perf --check`: a result judged against its platform's
//! baseline.
//!
//! Only the stable measurements are gated. Connections opened and
//! requests made must equal the baseline's in every run: they are counts,
//! with no noise. Median wall time, median CPU (user plus system) time and
//! median peak RSS may grow by their [`Tolerance`]: a growth past both its
//! relative and its absolute bound is a regression. CPU time is gated only
//! on series without injected latency: a process that idles through a
//! round trip is rescheduled, on a slower core or a lower clock, so its CPU
//! time follows the machine more than the code. The p95, throughputs and
//! output sizes are reported, not gated: the tail of a few runs is too
//! noisy to judge, and throughputs follow from the median wall time.
//!
//! A median is only as good as its runs. Each median's standard error is
//! estimated from its deviation ([`Spread::median_error`]), and a
//! comparison whose 95% resolution, twice the standard error of the
//! difference, is wider than the tolerance cannot tell a regression from
//! noise: it is reported as too noisy to judge, and fails the check, since
//! the gate cannot vouch for it. Times and RSS are only judged against a
//! baseline taken on the same CPU model; counts are judged on any.
//! The thresholds' reasons are in `tests/perf/README.md`.

use super::record::{Exact, Key, Record, Series};
use super::stats::Spread;

/// How far a median may grow: by the larger of `relative` times the
/// baseline and `absolute`, in the metric's unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Tolerance {
    pub(super) relative: f64,
    pub(super) absolute: f64,
}

impl Tolerance {
    fn limit(self, baseline: f64) -> f64 {
        (self.relative * baseline).max(self.absolute)
    }
}

/// Median wall time: 20%, and at least 5 ms.
pub(super) const WALL: Tolerance = Tolerance {
    relative: 0.20,
    absolute: 0.005,
};
/// Median CPU user plus system time: 20%, and at least 5 ms.
pub(super) const CPU: Tolerance = Tolerance {
    relative: 0.20,
    absolute: 0.005,
};
/// Median peak RSS: 10%, and at least 4 MiB.
pub(super) const RSS: Tolerance = Tolerance {
    relative: 0.10,
    absolute: 4.0 * 1024.0 * 1024.0,
};

/// One gated measurement's outcome.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Verdict {
    Within,
    /// The median grew past its limit.
    Regressed {
        baseline: f64,
        now: f64,
        limit: f64,
    },
    /// The median shrank past its limit: the baseline wants refreshing.
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
    /// A count differs from the baseline's, or varied between runs.
    Changed {
        baseline: Exact,
        now: Exact,
    },
    /// The baseline was taken on another CPU model, so times and RSS are
    /// not comparable.
    OtherCpu,
    /// The baseline has no such series.
    New,
}

impl Verdict {
    /// Whether the verdict fails the check.
    pub(super) fn fails(&self) -> bool {
        matches!(
            self,
            Self::Regressed { .. } | Self::Noisy { .. } | Self::Changed { .. }
        )
    }
}

/// The verdict on one measurement of one series.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Finding {
    pub(super) key: Key,
    pub(super) metric: Metric,
    pub(super) verdict: Verdict,
    /// The least and greatest one-minute load average seen before the
    /// series' runs.
    pub(super) load_average: [f64; 2],
}

/// The gated measurements.
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
            Self::Rss => "peak RSS p50",
        }
    }
}

/// Every finding on `current` against `baseline`, series by series.
pub(super) fn judge(baseline: &Record, current: &Record) -> Vec<Finding> {
    let same_cpu = baseline.host.cpu == current.host.cpu;
    current
        .series
        .iter()
        .flat_map(|now| {
            let base = baseline.series.iter().find(|base| base.key == now.key);
            judge_series(base, now, same_cpu)
        })
        .collect()
}

fn judge_series(base: Option<&Series>, now: &Series, same_cpu: bool) -> Vec<Finding> {
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
    let timed = [
        (Metric::Wall, b.wall_s, n.wall_s, WALL),
        (Metric::Cpu, b.cpu_s, n.cpu_s, CPU),
        (Metric::Rss, b.max_rss_bytes, n.max_rss_bytes, RSS),
    ];
    let idle = now.key.latency_ms != 0;
    for (metric, base, now, tolerance) in timed {
        if metric == Metric::Cpu && idle {
            continue;
        }
        let verdict = if same_cpu {
            median(base, now, tolerance)
        } else {
            Verdict::OtherCpu
        };
        findings.push(finding(metric, verdict));
    }
    findings
}

/// A count's verdict: it must not vary, and must equal the baseline's.
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

/// A median's verdict under `tolerance`.
pub(super) fn median(base: Spread, now: Spread, tolerance: Tolerance) -> Verdict {
    let limit = tolerance.limit(base.p50);
    let resolution = 2.0 * base.median_error().hypot(now.median_error());
    if resolution > limit {
        return Verdict::Noisy { resolution, limit };
    }
    let (baseline, now) = (base.p50, now.p50);
    if now - baseline > limit {
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

#[cfg(test)]
mod tests;
