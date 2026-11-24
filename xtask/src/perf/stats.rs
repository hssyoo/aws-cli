//! The order statistics the performance record summarises runs with.
//!
//! A series of runs is summarised by its median (p50), its 95th
//! percentile and its median absolute deviation, which a few outlying runs
//! barely move. Percentiles interpolate linearly between the two nearest
//! ranks, as numpy's default (Hyndman and Fan's type 7) does, so p50 is
//! the median.

use serde::{Deserialize, Serialize};

/// The spread of one measurement over a series' runs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(super) struct Spread {
    pub(super) p50: f64,
    pub(super) p95: f64,
    /// The median absolute deviation from `p50`.
    pub(super) mad: f64,
    /// The least and the greatest value.
    pub(super) min: f64,
    pub(super) max: f64,
    /// How many runs the spread is over.
    pub(super) n: usize,
}

impl Spread {
    /// The spread of `values`; `None` when there are none.
    pub(super) fn of(values: &[f64]) -> Option<Self> {
        let p50 = percentile(values, 0.5)?;
        let deviations: Vec<f64> = values.iter().map(|value| (value - p50).abs()).collect();
        Some(Self {
            p50,
            p95: percentile(values, 0.95)?,
            mad: percentile(&deviations, 0.5)?,
            min: percentile(values, 0.0)?,
            max: percentile(values, 1.0)?,
            n: values.len(),
        })
    }

    /// The standard error of `p50`, estimated from the deviation: the
    /// median of n normal draws has a standard error of about
    /// 1.2533 σ / √n, and σ is about 1.4826 times the MAD.
    pub(super) fn median_error(&self) -> f64 {
        let runs = f64::from(u32::try_from(self.n).unwrap_or(u32::MAX).max(1));
        1.4826 * 1.2533 * self.mad / runs.sqrt()
    }
}

/// The `fraction` quantile of `values` (0 to 1), interpolated linearly
/// between the nearest ranks; `None` when there are no values. NaNs sort
/// last.
pub(super) fn percentile(values: &[f64], fraction: f64) -> Option<f64> {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let last = sorted.len().checked_sub(1)?;
    let rank = fraction.clamp(0.0, 1.0) * f64::from(u32::try_from(last).ok()?);
    let below = rank.floor();
    // `below` is a whole number within 0..=last, so the casts are exact.
    let lower = sorted[below as usize];
    let upper = sorted[(rank.ceil() as usize).min(last)];
    Some(lower + (rank - below) * (upper - lower))
}

#[cfg(test)]
mod tests;
