//! `cargo xtask perf --calibrate`: a tier's tolerances, derived from
//! repeated comparisons of one binary with a byte-identical copy of it.
//!
//! Each repeat measures the binary and its copy, which lies at another
//! path, as base and head in the same rounds, exactly as a check measures
//! two builds. For each series and metric it gives two kinds of
//! deviation, where any is noise, since the code is the same:
//!
//! - **comparison:** what the gate's estimate of the head against the base
//!   comes to (the [`gate::crossover`] estimate for wall and CPU time, the
//!   shift both ends of the peaks share for RSS), and how wide that
//!   estimate's resolution is;
//! - **baseline:** how far the medians (both ends of the peaks for RSS) of
//!   every measurement of the series, both labels of every repeat, spread.
//!
//! Each tolerance covers three times the largest deviation and one and a
//! half times the widest resolution: three, because a check sets one
//! estimate against another that each carry the error, and a few repeats
//! understate the spread; one and a half, so that a check of as many runs
//! on as quiet a machine resolves its tolerance rather than reporting the
//! runs too noisy to judge. The absolute floor covers the deviations and
//! resolutions of small series (under 50 ms; for RSS any), rounded up to
//! the next half millisecond or half MiB; the relative bound covers those
//! the floor does not, rounded up to the next 1%, and at least 1%.

use super::gate::{self, Metric, Tolerance, Tolerances};
use super::record::{Record, Run, Series};
use super::stats::Spread;

/// The series a time's floor is drawn from: those under 50 ms.
const SMALL_SECONDS: f64 = 0.05;
const TIME_STEP: f64 = 0.0005;
const RSS_STEP: f64 = 512.0 * 1024.0;
/// The tolerance's multiple of the largest deviation seen.
const MARGIN: f64 = 3.0;
/// The tolerance's multiple of the widest resolution seen.
const RESOLUTION_MARGIN: f64 = 1.5;

/// One comparison of a binary with itself: the base's record and the head's.
pub(super) type Repeat = (Record, Record);

/// The deviations of one kind in `metric` over the repeats.
type Deviations = fn(&[Repeat], Metric) -> Vec<Deviation>;

/// One series' deviation in one metric, what it deviated from, and how
/// wide the estimate it came from resolves (zero where none is estimated).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Deviation {
    pub(super) amount: f64,
    pub(super) reference: f64,
    pub(super) resolution: f64,
}

impl Deviation {
    /// What a tolerance must cover for this deviation.
    fn covered(self) -> f64 {
        (MARGIN * self.amount).max(RESOLUTION_MARGIN * self.resolution)
    }
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

/// The tolerance that covers every deviation ([`Deviation::covered`]): a
/// floor from the deviations of references under `small`, and a relative
/// bound for the rest.
pub(super) fn tolerance(deviations: &[Deviation], small: f64, step: f64) -> Tolerance {
    let floor = deviations
        .iter()
        .filter(|deviation| deviation.reference < small)
        .map(|deviation| deviation.covered())
        .fold(0.0, f64::max);
    let absolute = (up(floor / step) * step).max(step);
    let relative = deviations
        .iter()
        .filter(|deviation| deviation.covered() > absolute && deviation.reference > 0.0)
        .map(|deviation| deviation.covered() / deviation.reference)
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
            let (base, head) = (base.summary.max_rss_bytes, head.summary.max_rss_bytes);
            let shift = gate::common_shift(head.min - base.min, head.max - base.max);
            return Some(Deviation {
                amount: shift.abs(),
                reference: base.min,
                resolution: 0.0,
            });
        }
        Metric::Cpu => gate::cpu,
        _ => gate::wall,
    };
    let estimate = gate::crossover(&base.runs, &head.runs, measure)?;
    Some(Deviation {
        amount: (estimate.head() - estimate.reference).abs(),
        reference: estimate.reference,
        resolution: estimate.resolution(),
    })
}

/// How far each series' medians spread over every record of every repeat;
/// for RSS, the smaller of how far its least and its greatest peaks
/// spread, since a check needs both to move.
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
            let measurements: Vec<&Spread> = records
                .iter()
                .filter_map(|record| record.series.iter().find(|other| other.key == series.key))
                .map(|other| match metric {
                    Metric::Rss => &other.summary.max_rss_bytes,
                    Metric::Cpu => &other.summary.cpu_s,
                    _ => &other.summary.wall_s,
                })
                .collect();
            spread_deviation(&measurements, metric)
        })
        .collect()
}

/// The spread over `measurements` of their medians, or, for RSS, of
/// their least and greatest values, whichever spreads less.
fn spread_deviation(measurements: &[&Spread], metric: Metric) -> Option<Deviation> {
    let range = |end: fn(&Spread) -> f64| {
        let values: Vec<f64> = measurements.iter().map(|spread| end(spread)).collect();
        Spread::of(&values).map(|spread| (spread.max - spread.min, spread.p50))
    };
    let (amount, reference) = if metric == Metric::Rss {
        let (least, reference) = range(|spread| spread.min)?;
        let (greatest, _) = range(|spread| spread.max)?;
        (least.min(greatest), reference)
    } else {
        range(|spread| spread.p50)?
    };
    Some(Deviation {
        amount,
        reference,
        resolution: 0.0,
    })
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
