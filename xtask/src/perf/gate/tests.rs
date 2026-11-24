use super::super::record::{Binary, Host, Summary};
use super::*;

fn spread(p50: f64, mad: f64) -> Spread {
    Spread {
        p50,
        p95: p50,
        mad,
        n: 10,
    }
}

fn key(route: &str) -> Key {
    Key {
        case: "query-single".into(),
        scheme: "https".into(),
        latency_ms: 0,
        route: route.into(),
    }
}

/// A series with a 100 ms wall time, 20 ms of CPU and 20 MiB of RSS, each
/// with a deviation of `mad` of its median, on `connections` connections.
fn series(route: &str, scale: f64, mad: f64, connections: Exact) -> Series {
    Series {
        key: key(route),
        pages: 1,
        load_average: [1.0, 1.0],
        summary: Summary {
            wall_s: spread(0.1 * scale, 0.1 * mad),
            cpu_s: spread(0.02 * scale, 0.02 * mad),
            user_s: spread(0.015, 0.0),
            sys_s: spread(0.005, 0.0),
            max_rss_bytes: spread(20.0 * 1024.0 * 1024.0 * scale, 0.0),
            connections,
            requests: Exact { min: 1, max: 1 },
            response_bytes: 100.0,
            stdout_bytes: 10.0,
            requests_per_s: 10.0,
            pages_per_s: 10.0,
            mb_per_s: 0.001,
        },
        runs: Vec::new(),
    }
}

fn record(cpu: &str, series: Vec<Series>) -> Record {
    Record {
        format: 1,
        platform: "macos-aarch64".into(),
        host: Host {
            system: "Darwin".into(),
            release: "27".into(),
            machine: "arm64".into(),
            os: "macOS".into(),
            cpu: cpu.into(),
            cpus: 12,
        },
        binary: Binary {
            path: "aws".into(),
            commit: None,
            dirty: None,
        },
        started_unix_s: 0.0,
        warmup: 2,
        runs: 10,
        series,
    }
}

const ONE: Exact = Exact { min: 1, max: 1 };

fn verdicts(findings: &[Finding]) -> Vec<(Metric, Verdict)> {
    findings
        .iter()
        .map(|finding| (finding.metric, finding.verdict.clone()))
        .collect()
}

#[test]
fn a_series_like_its_baseline_is_within_every_bound() {
    let base = record("M4", vec![series("native", 1.0, 0.01, ONE)]);
    let now = record("M4", vec![series("native", 1.05, 0.01, ONE)]);
    let findings = judge(&base, &now);
    assert_eq!(findings.len(), 5);
    assert!(
        findings
            .iter()
            .all(|finding| finding.verdict == Verdict::Within)
    );
}

#[test]
fn growth_past_both_bounds_regresses_and_shrinkage_improves() {
    let wall = |p50| spread(p50, 0.0);
    assert_eq!(
        median(wall(0.1), wall(0.125), WALL),
        Verdict::Regressed {
            baseline: 0.1,
            now: 0.125,
            limit: 0.020000000000000004
        }
    );
    assert_eq!(median(wall(0.1), wall(0.118), WALL), Verdict::Within);
    assert!(matches!(
        median(wall(0.1), wall(0.075), WALL),
        Verdict::Improved { .. }
    ));
    // 20% of 20 ms is under the 5 ms floor: a 4 ms growth is within it.
    assert_eq!(median(wall(0.02), wall(0.024), WALL), Verdict::Within);
    assert!(median(wall(0.02), wall(0.026), WALL).fails());
}

#[test]
fn runs_too_spread_to_resolve_the_bound_are_too_noisy_to_judge() {
    // A 4 ms deviation over 10 runs: each median is good to about 2.35 ms,
    // the difference to 6.65 ms at 95%, past the 5 ms floor.
    let verdict = median(spread(0.02, 0.004), spread(0.02, 0.004), WALL);
    assert!(matches!(verdict, Verdict::Noisy { limit, .. } if limit == 0.005));
    assert!(verdict.fails());
    assert_eq!(
        median(spread(0.02, 0.001), spread(0.02, 0.001), WALL),
        Verdict::Within
    );
}

#[test]
fn counts_must_equal_the_baseline_and_not_vary() {
    let base = record("M4", vec![series("native", 1.0, 0.0, ONE)]);
    let more = record(
        "M4",
        vec![series("native", 1.0, 0.0, Exact { min: 2, max: 2 })],
    );
    let varied = record(
        "M4",
        vec![series("native", 1.0, 0.0, Exact { min: 1, max: 2 })],
    );
    for now in [more, varied] {
        let findings = judge(&base, &now);
        let (metric, verdict) = &verdicts(&findings)[0];
        assert_eq!(*metric, Metric::Connections);
        assert!(matches!(verdict, Verdict::Changed { baseline, .. } if *baseline == ONE));
        assert!(verdict.fails());
    }
}

#[test]
fn another_cpu_judges_only_the_counts() {
    let base = record("M1", vec![series("native", 1.0, 0.0, ONE)]);
    let now = record("M4", vec![series("native", 3.0, 0.0, ONE)]);
    assert_eq!(
        verdicts(&judge(&base, &now)),
        [
            (Metric::Connections, Verdict::Within),
            (Metric::Requests, Verdict::Within),
            (Metric::Wall, Verdict::OtherCpu),
            (Metric::Cpu, Verdict::OtherCpu),
            (Metric::Rss, Verdict::OtherCpu),
        ]
    );
}

#[test]
fn a_series_missing_from_the_baseline_is_new_and_does_not_fail() {
    let base = record("M4", vec![series("native", 1.0, 0.0, ONE)]);
    let now = record("M4", vec![series("python", 1.0, 0.0, ONE)]);
    let findings = judge(&base, &now);
    assert_eq!(verdicts(&findings), [(Metric::Series, Verdict::New)]);
    assert!(!findings[0].verdict.fails());
}

#[test]
fn cpu_time_is_not_gated_on_a_series_with_injected_latency() {
    let with_latency = |scale| {
        let mut series = series("native", scale, 0.0, ONE);
        series.key.latency_ms = 80;
        series
    };
    let base = record("M4", vec![with_latency(1.0)]);
    let now = record("M4", vec![with_latency(1.0)]);
    let metrics: Vec<Metric> = judge(&base, &now)
        .iter()
        .map(|finding| finding.metric)
        .collect();
    assert_eq!(
        metrics,
        [
            Metric::Connections,
            Metric::Requests,
            Metric::Wall,
            Metric::Rss
        ]
    );
}
