//! `cargo xtask perf --calibrate`: a tier's tolerances, derived from
//! repeated comparisons of one binary with itself.
//!
//! Each repeat measures the binary twice in the same rounds, as a base and
//! as a head. For each series and metric it gives two kinds of deviation,
//! where any is noise, since the code is the same:
//!
//! - **comparison:** what the gate's estimate of the head against the base
//!   comes to (the median log ratio of paired runs for wall and CPU time,
//!   the difference of the least peaks for RSS);
//! - **baseline:** how far the medians (least peaks for RSS) of every
//!   measurement of the series, both labels of every repeat, spread.
//!
//! Each tolerance is three times the largest deviation: three, because a
//! check sets one estimate against another that each carry the error, and
//! a few repeats understate the spread. The absolute floor is three times
//! the largest deviation on a small series (under 50 ms; for RSS any),
//! rounded up to the next half millisecond or half MiB; the relative bound
//! is three times the largest relative deviation among the series the
//! floor does not cover, rounded up to the next 1%, and at least 1%.

use super::gate::{self, Metric, Tolerance, Tolerances};
use super::record::{Record, Run, Series};
use super::stats::{Spread, percentile};

/// The series a time's floor is drawn from: those under 50 ms.
const SMALL_SECONDS: f64 = 0.05;
const TIME_STEP: f64 = 0.0005;
const RSS_STEP: f64 = 512.0 * 1024.0;
/// The tolerance's multiple of the largest deviation seen.
const MARGIN: f64 = 3.0;

/// One comparison of a binary with itself: the base's record and the head's.
pub(super) type Repeat = (Record, Record);

/// The deviations of one kind in `metric` over the repeats.
type Deviations = fn(&[Repeat], Metric) -> Vec<Deviation>;

/// One series' deviation in one metric, and what it deviated from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Deviation {
    pub(super) amount: f64,
    pub(super) reference: f64,
}

/// The comparison's and the baseline check's tolerances from `repeats`,
/// each a base and a head record of one binary.
pub(super) fn derive(repeats: &[Repeat]) -> (Tolerances, Tolerances) {
    let tolerances = |deviations: Deviations| Tolerances {
        wall: tolerance(&deviations(repeats, Metric::Wall), SMALL_SECONDS, TIME_STEP),
        cpu: tolerance(&deviations(repeats, Metric::Cpu), SMALL_SECONDS, TIME_STEP),
        rss: tolerance(&deviations(repeats, Metric::Rss), f64::INFINITY, RSS_STEP),
    };
    (
        tolerances(comparison_deviations),
        tolerances(baseline_deviations),
    )
}

/// The tolerance that covers `MARGIN` times every deviation: a floor from
/// the deviations of references under `small`, and a relative bound for
/// the rest.
pub(super) fn tolerance(deviations: &[Deviation], small: f64, step: f64) -> Tolerance {
    let floor = deviations
        .iter()
        .filter(|deviation| deviation.reference < small)
        .map(|deviation| MARGIN * deviation.amount)
        .fold(0.0, f64::max);
    let absolute = (up(floor / step) * step).max(step);
    let relative = deviations
        .iter()
        .filter(|deviation| MARGIN * deviation.amount > absolute && deviation.reference > 0.0)
        .map(|deviation| MARGIN * deviation.amount / deviation.reference)
        .fold(0.0, f64::max);
    Tolerance {
        relative: (up(relative * 100.0) / 100.0).max(0.01),
        absolute,
    }
}

/// `value` rounded up to a whole number, a whole number staying itself
/// despite the rounding of the arithmetic that produced it.
fn up(value: f64) -> f64 {
    (value - 1e-9).ceil()
}

/// Whether `metric` is judged on `series`: CPU only without latency.
fn judged(series: &Series, metric: Metric) -> bool {
    metric != Metric::Cpu || series.key.latency_ms == 0
}

/// What the gate's estimate of each head series against its base came to.
pub(super) fn comparison_deviations(repeats: &[Repeat], metric: Metric) -> Vec<Deviation> {
    repeats
        .iter()
        .flat_map(|(base, head)| {
            head.series.iter().filter_map(move |now| {
                let reference = base.series.iter().find(|series| series.key == now.key)?;
                judged(now, metric).then(|| estimate(reference, now, metric))?
            })
        })
        .collect()
}

fn estimate(base: &Series, head: &Series, metric: Metric) -> Option<Deviation> {
    let measure: fn(&Run) -> f64 = match metric {
        Metric::Rss => {
            let (base, head) = (
                base.summary.max_rss_bytes.min,
                head.summary.max_rss_bytes.min,
            );
            return Some(Deviation {
                amount: (head - base).abs(),
                reference: base,
            });
        }
        Metric::Cpu => gate::cpu,
        _ => gate::wall,
    };
    let ratios: Vec<f64> = base
        .runs
        .iter()
        .zip(&head.runs)
        .map(|(b, h)| (measure(h) / measure(b)).ln())
        .collect();
    let reference = percentile(&base.runs.iter().map(measure).collect::<Vec<_>>(), 0.5)?;
    let ratio = Spread::of(&ratios)?.p50;
    Some(Deviation {
        amount: (reference * ratio.exp_m1()).abs(),
        reference,
    })
}

/// How far each series' medians (least peaks for RSS) spread over every
/// record of every repeat.
pub(super) fn baseline_deviations(repeats: &[Repeat], metric: Metric) -> Vec<Deviation> {
    let records: Vec<&Record> = repeats
        .iter()
        .flat_map(|(base, head)| [base, head])
        .collect();
    let Some(first) = records.first() else {
        return Vec::new();
    };
    first
        .series
        .iter()
        .filter(|series| judged(series, metric))
        .filter_map(|series| {
            let values: Vec<f64> = records
                .iter()
                .filter_map(|record| record.series.iter().find(|other| other.key == series.key))
                .map(|other| match metric {
                    Metric::Rss => other.summary.max_rss_bytes.min,
                    Metric::Cpu => other.summary.cpu_s.p50,
                    _ => other.summary.wall_s.p50,
                })
                .collect();
            let spread = Spread::of(&values)?;
            Some(Deviation {
                amount: spread.max - spread.min,
                reference: values.iter().sum::<f64>()
                    / f64::from(u32::try_from(values.len()).ok()?),
            })
        })
        .collect()
}

/// The least and greatest load average before any run of `repeats`.
pub(super) fn load_range(repeats: &[Repeat]) -> [f64; 2] {
    let loads: Vec<f64> = repeats
        .iter()
        .flat_map(|(base, head)| [base, head])
        .flat_map(|record| &record.series)
        .flat_map(|series| &series.runs)
        .map(|run| run.load_average)
        .collect();
    Spread::of(&loads).map_or([0.0; 2], |spread| [spread.min, spread.max])
}

#[cfg(test)]
mod tests;
