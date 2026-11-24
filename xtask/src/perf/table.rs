//! The readable forms of a performance record and of a check's findings.

use super::gate::{Finding, Metric, Verdict};
use super::record::{Exact, Record, Series};

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

/// The record as a table: one row per series, after a line naming the
/// binary and the host.
pub(super) fn render(record: &Record) -> String {
    let mut rows = vec![HEADER.map(str::to_owned).to_vec()];
    rows.extend(record.series.iter().map(row));
    let widths: Vec<usize> = (0..HEADER.len())
        .map(|column| rows.iter().map(|row| row[column].len()).max().unwrap_or(0))
        .collect();
    let binary = &record.binary;
    let mut text = format!(
        "perf: {} at {}{} on {} ({}, {} CPUs); {} runs after {} warm-up\n",
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
        record.runs,
        record.warmup,
    );
    for row in rows {
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

fn row(series: &Series) -> Vec<String> {
    let summary = &series.summary;
    let ms = |seconds: f64| format!("{:.1}", seconds * 1e3);
    vec![
        series.key.case.clone(),
        series.key.scheme.clone(),
        series.key.latency_ms.to_string(),
        series.key.route.clone(),
        ms(summary.wall_s.p50),
        ms(summary.wall_s.p95),
        format!("{:.1}", summary.requests_per_s),
        format!("{:.1}", summary.pages_per_s),
        format!("{:.2}", summary.mb_per_s),
        format!("{:.1}", summary.max_rss_bytes.p50 / 1e6),
        ms(summary.user_s.p50),
        ms(summary.sys_s.p50),
        format!("{}/{}", count(summary.connections), count(summary.requests)),
    ]
}

fn count(value: Exact) -> String {
    if value.min == value.max {
        value.min.to_string()
    } else {
        format!("{}-{}", value.min, value.max)
    }
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
    let [before, after] = finding.load_average;
    let what = match &finding.verdict {
        Verdict::Within => "within bounds".to_owned(),
        Verdict::Regressed {
            baseline,
            now,
            limit,
        } => format!(
            "REGRESSED: {} against a baseline of {}, past the {} allowed",
            value(*now),
            value(*baseline),
            value(*limit)
        ),
        Verdict::Improved {
            baseline,
            now,
            limit,
        } => format!(
            "improved: {} against a baseline of {}, by more than {}; refresh the baseline",
            value(*now),
            value(*baseline),
            value(*limit)
        ),
        Verdict::Noisy { resolution, limit } => format!(
            "TOO NOISY TO JUDGE: the runs resolve {} at 95%, wider than the {} allowed \
             (load average {before:.2} to {after:.2}); rerun on a quieter machine or \
             with more --runs",
            value(*resolution),
            value(*limit)
        ),
        Verdict::Changed { baseline, now } => format!(
            "CHANGED: {} against a baseline of {} (counts must not vary)",
            count(*now),
            count(*baseline)
        ),
        Verdict::OtherCpu => "not judged: the baseline was taken on another CPU".to_owned(),
        Verdict::New => "new: the baseline has no such series".to_owned(),
    };
    format!("{} {}: {what}", finding.key, finding.metric.name())
}

#[cfg(test)]
mod tests;
