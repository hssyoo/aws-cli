use super::super::baseline::Calibration;
use super::super::gate::{Finding, Tolerance, Tolerances};
use super::super::record::{Binary, Key, Samples, Tier};
use super::*;
use crate::{TaskError, deps};
use serde_json::json;

fn samples() -> Result<Samples, TaskError> {
    let run = |wall: f64| {
        json!({
            "wall_s": wall, "user_s": 0.0081, "sys_s": 0.0035, "max_rss_bytes": 2.24e7,
            "connections": 1, "requests": 1, "response_bytes": 1000.0, "stdout_bytes": 117.0,
            "load_average": 1.0
        })
    };
    let series = |binary: &str, wall: f64| {
        json!({"binary": binary, "case": "query-single", "scheme": "https", "latency_ms": 0,
               "route": "native", "pages": 1, "load_average": [1.0, 1.0], "runs": [run(wall)]})
    };
    serde_json::from_value(json!({
        "format": 2,
        "tier": "quick",
        "host": {"system": "Darwin", "release": "27", "machine": "arm64",
                 "os": "macOS", "cpu": "Apple M4 Pro", "cpus": 12},
        "started_unix_s": 0.0, "warmup": 2, "runs": 1,
        "series": [series("head", 0.0254), series("base", 0.0301)],
        "refused": [],
        "first_run_s": {"head": 1.312}
    }))
    .map_err(|error| deps::error(error.to_string()))
}

fn binary(commit: &str) -> Binary {
    Binary {
        path: "target/aws".into(),
        commit: Some(commit.into()),
        dirty: Some(true),
    }
}

fn record(name: &str) -> Result<Record, TaskError> {
    Record::new(
        &samples()?,
        name,
        "macos-aarch64",
        binary(name),
        Tier::Quick,
    )
}

#[test]
fn the_table_has_a_row_per_series_under_the_binary_and_host() -> Result<(), TaskError> {
    let text = render(&record("head")?);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[0],
        "perf: target/aws at head with uncommitted changes on macos-aarch64 \
         (Apple M4 Pro, 12 CPUs); quick tier, 1 runs after 2 warm-up; first run 1312 ms"
    );
    assert!(lines[1].starts_with("case          scheme  rtt ms  route   p50 ms"));
    let cells: Vec<&str> = lines[2].split_whitespace().collect();
    assert_eq!(
        cells,
        [
            "query-single",
            "https",
            "0",
            "native",
            "25.4",
            "25.4",
            "39.4",
            "39.4",
            "0.04",
            "22.4",
            "8.1",
            "3.5",
            "1/1"
        ]
    );
    assert_eq!(lines.len(), 3);
    Ok(())
}

#[test]
fn a_comparison_shows_the_base_beside_the_head() -> Result<(), TaskError> {
    let text = render_comparison(&record("base")?, &record("head")?);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("perf: target/aws at base"));
    assert!(lines[1].starts_with("perf: target/aws at head"));
    assert!(lines[2].contains("base ms  head ms"));
    let cells: Vec<&str> = lines[3].split_whitespace().collect();
    assert_eq!(
        cells,
        [
            "query-single",
            "https",
            "0",
            "native",
            "30.1",
            "25.4",
            "11.6",
            "11.6",
            "22.4",
            "22.4",
            "1/1"
        ]
    );
    Ok(())
}

#[test]
fn a_calibration_lists_each_mode_s_tolerances() -> Result<(), TaskError> {
    let time = Tolerance {
        relative: 0.03,
        absolute: 0.0015,
    };
    let rss = Tolerance {
        relative: 0.01,
        absolute: 512.0 * 1024.0,
    };
    let tolerances = Tolerances {
        wall: time,
        cpu: time,
        rss,
    };
    let calibration = Calibration {
        unix_s: 0.0,
        commit: None,
        host: record("head")?.host,
        load_average: [1.1, 1.6],
        repeats: 5,
        comparison: tolerances,
        baseline: tolerances,
    };
    assert_eq!(
        super::calibration(Tier::Quick, &calibration)
            .lines()
            .collect::<Vec<_>>(),
        [
            "perf: quick calibration over 5 repeats on Apple M4 Pro (12 CPUs), load average \
             1.10 to 1.60:",
            "  comparison: wall 3%, at least 1.5 ms; cpu 3%, at least 1.5 ms; least peak RSS 1%, \
             at least 0.5 MiB",
            "  baseline: wall 3%, at least 1.5 ms; cpu 3%, at least 1.5 ms; least peak RSS 1%, \
             at least 0.5 MiB",
        ]
    );
    Ok(())
}

#[test]
fn only_findings_outside_their_bounds_are_listed() {
    let key = Key {
        case: "query-single".into(),
        scheme: "https".into(),
        latency_ms: 20,
        route: "native".into(),
    };
    let finding = |metric, verdict| Finding {
        key: key.clone(),
        metric,
        verdict,
        load_average: [2.5, 3.0],
    };
    let listed = findings(&[
        finding(Metric::Requests, Verdict::Within),
        finding(
            Metric::Wall,
            Verdict::Regressed {
                baseline: 0.1,
                now: 0.12,
                limit: 0.01,
            },
        ),
        finding(
            Metric::Rss,
            Verdict::Loaded {
                load: 9.5,
                limit: 4.6,
            },
        ),
        finding(
            Metric::Connections,
            Verdict::Changed {
                baseline: Exact { min: 1, max: 1 },
                now: Exact { min: 10, max: 10 },
            },
        ),
        finding(Metric::Series, Verdict::Missing),
        finding(Metric::Series, Verdict::RefusedByBase("exited 255".into())),
    ]);
    assert_eq!(
        listed.lines().collect::<Vec<_>>(),
        [
            "query-single https 20 ms native wall p50: REGRESSED: 120.0 ms against 100.0 ms, \
             past the 10.0 ms allowed",
            "query-single https 20 ms native least peak RSS: MACHINE LOADED, not judged: median \
             load average 9.50, above the 4.60 this check tolerates; rerun on a quieter machine",
            "query-single https 20 ms native connections: CHANGED: 10 against 1 (counts must \
             match in every run)",
            "query-single https 20 ms native series: MISSING: the baseline has this series and \
             the run did not",
            "query-single https 20 ms native series: not judged: the base refused it (exited 255)",
        ]
    );
}
