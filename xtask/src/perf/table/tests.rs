use super::super::gate::Finding;
use super::super::record::{Binary, Key, Samples, Tier};
use super::*;
use crate::{TaskError, deps};
use serde_json::json;

fn record() -> Result<Record, TaskError> {
    let run = json!({
        "wall_s": 0.0254, "user_s": 0.0081, "sys_s": 0.0035, "max_rss_bytes": 2.24e7,
        "connections": 1, "requests": 1, "response_bytes": 1000.0, "stdout_bytes": 117.0
    });
    let samples: Samples = serde_json::from_value(json!({
        "format": 1,
        "host": {"system": "Darwin", "release": "27", "machine": "arm64",
                 "os": "macOS", "cpu": "Apple M4 Pro", "cpus": 12},
        "started_unix_s": 0.0, "warmup": 2, "runs": 1,
        "series": [{"case": "query-single", "scheme": "https", "latency_ms": 0,
                    "route": "native", "pages": 1, "load_average": [1.0, 1.0],
                    "runs": [run]}]
    }))
    .map_err(|error| deps::error(error.to_string()))?;
    let binary = Binary {
        path: "target/aws".into(),
        commit: Some("abc123".into()),
        dirty: Some(true),
    };
    Record::new(samples, "macos-aarch64", binary, Tier::Quick)
}

#[test]
fn the_table_has_a_row_per_series_under_the_binary_and_host() -> Result<(), TaskError> {
    let text = render(&record()?);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[0],
        "perf: target/aws at abc123 with uncommitted changes on macos-aarch64 \
         (Apple M4 Pro, 12 CPUs); quick tier, 1 runs after 2 warm-up"
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
            Verdict::Noisy {
                resolution: 8.0 * 1024.0 * 1024.0,
                limit: 4.0 * 1024.0 * 1024.0,
            },
        ),
        finding(
            Metric::Connections,
            Verdict::Changed {
                baseline: Exact { min: 1, max: 1 },
                now: Exact { min: 10, max: 10 },
            },
        ),
    ]);
    assert_eq!(
        listed.lines().collect::<Vec<_>>(),
        [
            "query-single https 20 ms native wall p50: REGRESSED: 120.0 ms against a \
             baseline of 100.0 ms, past the 10.0 ms allowed",
            "query-single https 20 ms native peak RSS p50: TOO NOISY TO JUDGE: the runs \
             resolve 8.0 MiB at 95%, wider than the 4.0 MiB allowed (load average 2.50 to \
             3.00); rerun on a quieter machine or with more --runs",
            "query-single https 20 ms native connections: CHANGED: 10 against a baseline \
             of 1 (counts must not vary)",
        ]
    );
}
