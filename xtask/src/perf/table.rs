//! The readable forms of a performance record, a comparison, a
//! calibration and a check's findings.

use super::baseline::Calibration;
use super::gate::{Finding, Metric, Tolerance, Tolerances, Unjudged, Verdict};
use super::record::{Exact, Record, Series, Tier};

const HEADER: [&str; 13] = [
    "case",
    "scheme",
    "rtt ms",
    "route",
    "p50 ms",
    "p95 ms",
    "req/s",
    "pages/s",
    "MB/s",
    "RSS MB",
    "user ms",
    "sys ms",
    "conns/reqs",
];

const COMPARISON_HEADER: [&str; 11] = [
    "case",
    "scheme",
    "rtt ms",
    "route",
    "base ms",
    "head ms",
    "base cpu",
    "head cpu",
    "base RSS",
    "head RSS",
    "conns/reqs",
];

/// `rows` under `header`, aligned: the first four columns to the left,
/// the rest to the right.
fn layout(header: &[&str], rows: impl Iterator<Item = Vec<String>>) -> String {
    let header: Vec<String> = header.iter().map(|cell| (*cell).to_owned()).collect();
    let rows: Vec<Vec<String>> = std::iter::once(header).chain(rows).collect();
    let widths: Vec<usize> = (0..rows[0].len())
        .map(|column| rows.iter().map(|row| row[column].len()).max().unwrap_or(0))
        .collect();
    let mut text = String::new();
    for row in &rows {
        let cells: Vec<String> = row
            .iter()
            .zip(&widths)
            .enumerate()
            .map(|(column, (cell, width))| {
                if column < 4 {
                    format!("{cell:<width$}")
                } else {
                    format!("{cell:>width$}")
                }
            })
            .collect();
        text.push_str(cells.join("  ").trim_end());
        text.push('\n');
    }
    text
}

/// The line naming a record's binary, host, tier, runs and first run.
fn heading(record: &Record) -> String {
    let binary = &record.binary;
    let first = record.first_run_s.map_or_else(String::new, |seconds| {
        format!("; first run {:.0} ms", seconds * 1e3)
    });
    format!(
        "perf: {} at {}{} on {} ({}, {} CPUs); {} tier, {} runs after {} warm-up{first}\n",
        binary.path,
        binary.commit.as_deref().unwrap_or("an unknown commit"),
        if binary.dirty == Some(true) {
            " with uncommitted changes"
        } else {
            ""
        },
        record.platform,
        record.host.cpu,
        record.host.cpus,
        record.tier.name(),
        record.runs,
        record.warmup,
    )
}

/// The record as a table: one row per series, after its heading.
pub(super) fn render(record: &Record) -> String {
    heading(record) + &layout(&HEADER, record.series.iter().map(row))
}

/// A comparison as a table: one row per series of the head, with the
/// base's medians beside its own, and the least peak RSS of each.
pub(super) fn render_comparison(base: &Record, head: &Record) -> String {
    let rows = head.series.iter().map(|now| {
        let reference = base.series.iter().find(|series| series.key == now.key);
        comparison_row(reference, now)
    });
    heading(base) + &heading(head) + &layout(&COMPARISON_HEADER, rows)
}

fn ms(seconds: f64) -> String {
    format!("{:.1}", seconds * 1e3)
}

fn mb(bytes: f64) -> String {
    format!("{:.1}", bytes / 1e6)
}

fn key_cells(series: &Series) -> Vec<String> {
    vec![
        series.key.case.clone(),
        series.key.scheme.clone(),
        series.key.latency_ms.to_string(),
        series.key.route.clone(),
    ]
}

fn row(series: &Series) -> Vec<String> {
    let summary = &series.summary;
    let mut cells = key_cells(series);
    cells.extend([
        ms(summary.wall_s.p50),
        ms(summary.wall_s.p95),
        format!("{:.1}", summary.requests_per_s),
        format!("{:.1}", summary.pages_per_s),
        format!("{:.2}", summary.mb_per_s),
        mb(summary.max_rss_bytes.p50),
        ms(summary.user_s.p50),
        ms(summary.sys_s.p50),
        format!("{}/{}", count(summary.connections), count(summary.requests)),
    ]);
    cells
}

fn comparison_row(base: Option<&Series>, head: &Series) -> Vec<String> {
    let n = &head.summary;
    let refused = || "refused".to_owned();
    let mut cells = key_cells(head);
    cells.extend([
        base.map_or_else(refused, |base| ms(base.summary.wall_s.p50)),
        ms(n.wall_s.p50),
        base.map_or_else(refused, |base| ms(base.summary.cpu_s.p50)),
        ms(n.cpu_s.p50),
        base.map_or_else(refused, |base| mb(base.summary.max_rss_bytes.min)),
        mb(n.max_rss_bytes.min),
        format!("{}/{}", count(n.connections), count(n.requests)),
    ]);
    cells
}

fn count(value: Exact) -> String {
    if value.min == value.max {
        value.min.to_string()
    } else {
        format!("{}-{}", value.min, value.max)
    }
}

fn time(tolerance: Tolerance) -> String {
    format!(
        "{:.0}%, at least {:.1} ms",
        tolerance.relative * 100.0,
        tolerance.absolute * 1e3
    )
}

fn mode_line(mode: &str, tolerances: &Tolerances) -> String {
    format!(
        "  {mode}: wall {}; cpu {}; least peak RSS {:.0}%, at least {:.1} MiB\n",
        time(tolerances.wall),
        time(tolerances.cpu),
        tolerances.rss.relative * 100.0,
        tolerances.rss.absolute / (1024.0 * 1024.0)
    )
}

/// A calibration's tolerances.
pub(super) fn calibration(tier: Tier, calibration: &Calibration) -> String {
    let [low, high] = calibration.load_average;
    format!(
        "perf: {} calibration over {} repeats on {} ({} CPUs), load average {low:.2} to \
         {high:.2}:\n{}{}",
        tier.name(),
        calibration.repeats,
        calibration.host.cpu,
        calibration.host.cpus,
        mode_line("comparison", &calibration.comparison),
        mode_line("baseline", &calibration.baseline)
    )
}

/// One line for each finding that is not within its bounds.
pub(super) fn findings(findings: &[Finding]) -> String {
    findings
        .iter()
        .filter(|finding| finding.verdict != Verdict::Within)
        .map(|finding| format!("{}\n", line(finding)))
        .collect()
}

fn line(finding: &Finding) -> String {
    let value = |amount: f64| match finding.metric {
        Metric::Rss => format!("{:.1} MiB", amount / (1024.0 * 1024.0)),
        _ => format!("{:.1} ms", amount * 1e3),
    };
    let what = match &finding.verdict {
        Verdict::Regressed {
            baseline,
            now,
            limit,
        } => format!(
            "REGRESSED: {} against {}, past the {} allowed",
            value(*now),
            value(*baseline),
            value(*limit)
        ),
        Verdict::Improved {
            baseline,
            now,
            limit,
        } => format!(
            "improved: {} against {}, by more than {}",
            value(*now),
            value(*baseline),
            value(*limit)
        ),
        Verdict::Noisy { resolution, limit } => {
            let [before, after] = finding.load_average;
            format!(
                "TOO NOISY TO JUDGE: the runs resolve {} at 95%, wider than the {} allowed \
                 (load average {before:.2} to {after:.2}); rerun on a quieter machine or \
                 with more --runs",
                value(*resolution),
                value(*limit)
            )
        }
        verdict => unmeasured(verdict),
    };
    format!("{} {}: {what}", finding.key, finding.metric.name())
}

/// The words for a verdict that carries no measurement to show.
fn unmeasured(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Loaded { load, limit } => format!(
            "MACHINE LOADED, not judged: median load average {load:.2}, above the {limit:.2} \
             this check tolerates; rerun on a quieter machine"
        ),
        Verdict::Changed { baseline, now } => format!(
            "CHANGED: {} against {} (counts must match in every run)",
            count(*now),
            count(*baseline)
        ),
        Verdict::NotJudged(Unjudged::OtherCpu) => {
            "not judged: the baseline was taken on another CPU".to_owned()
        }
        Verdict::NotJudged(Unjudged::Uncalibrated) => {
            "not judged: the platform is not calibrated".to_owned()
        }
        Verdict::New => "new: the reference has no such series".to_owned(),
        Verdict::Missing => "MISSING: the baseline has this series and the run did not".to_owned(),
        Verdict::RefusedByBase(reason) => format!("not judged: the base refused it ({reason})"),
        _ => "within bounds".to_owned(),
    }
}

#[cfg(test)]
mod tests;
