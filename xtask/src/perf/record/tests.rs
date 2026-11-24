use super::*;
use serde_json::json;

fn json_error(error: serde_json::Error) -> TaskError {
    deps::error(error.to_string())
}

fn run(wall_s: f64, connections: u32) -> serde_json::Value {
    json!({
        "wall_s": wall_s, "user_s": 0.01, "sys_s": 0.005, "max_rss_bytes": 2.0e7,
        "connections": connections, "requests": 4, "response_bytes": 2.0e6,
        "stdout_bytes": 100.0, "load_average": 1.5
    })
}

/// measure.py's file for one four-page series of a head (and a base that
/// refused a second series), in `tier`.
fn samples(format: u32, tier: &str, runs: &[serde_json::Value]) -> Result<Samples, TaskError> {
    let value = json!({
        "format": format,
        "tier": tier,
        "host": {
            "system": "Darwin", "release": "27.2.0", "machine": "arm64",
            "os": "macOS-27.2.1-arm64", "cpu": "Apple M4 Pro", "cpus": 12
        },
        "started_unix_s": 1.0,
        "warmup": 2,
        "runs": 3,
        "series": [
            {
                "binary": "head", "case": "json-pages-4", "scheme": "https",
                "latency_ms": 20, "route": "native", "pages": 4,
                "load_average": [1.5, 1.75], "runs": runs
            },
            {
                "binary": "base", "case": "json-pages-4", "scheme": "https",
                "latency_ms": 20, "route": "native", "pages": 4,
                "load_average": [1.5, 1.75], "runs": [run(9.0, 9)]
            }
        ],
        "refused": [{
            "binary": "base", "case": "rest-json-single", "scheme": "https",
            "latency_ms": 0, "route": "native", "reason": "exited 255"
        }],
        "first_run_s": {"head": 1.3, "base": 1.2}
    });
    serde_json::from_value(value).map_err(json_error)
}

fn binary() -> Binary {
    Binary {
        path: "target/aws".into(),
        commit: Some("abc".into()),
        dirty: Some(false),
    }
}

fn record() -> Result<Record, TaskError> {
    let runs = [run(0.5, 1), run(0.4, 1), run(0.6, 2)];
    let samples = samples(SAMPLES_FORMAT, "full", &runs)?;
    Record::new(&samples, "head", "macos-aarch64", binary(), Tier::Full)
}

#[test]
fn a_series_is_summarised_over_its_runs() -> Result<(), TaskError> {
    let record = record()?;
    let series = &record.series[0];
    assert_eq!(series.key.to_string(), "json-pages-4 https 20 ms native");
    let summary = &series.summary;
    assert_eq!((summary.wall_s.p50, summary.wall_s.n), (0.5, 3));
    assert_eq!((summary.wall_s.min, summary.wall_s.max), (0.4, 0.6));
    assert!((summary.cpu_s.p50 - 0.015).abs() < 1e-12);
    assert_eq!(summary.connections, Exact { min: 1, max: 2 });
    assert_eq!(summary.requests, Exact { min: 4, max: 4 });
    // Four requests and four pages in a median 0.5 s, carrying 2 MB.
    assert_eq!(
        (
            summary.requests_per_s,
            summary.pages_per_s,
            summary.mb_per_s
        ),
        (8.0, 8.0, 4.0)
    );
    Ok(())
}

#[test]
fn a_record_keeps_its_runs_load_platform_and_tier() -> Result<(), TaskError> {
    let record = record()?;
    let series = &record.series[0];
    assert_eq!(series.load_average, [1.5, 1.75]);
    assert_eq!(series.runs.len(), 3);
    assert_eq!(
        (record.platform.as_str(), record.tier),
        ("macos-aarch64", Tier::Full)
    );
    Ok(())
}

#[test]
fn a_record_takes_its_own_binary_s_series_refusals_and_first_run() -> Result<(), TaskError> {
    let runs = [run(0.5, 1)];
    let samples = samples(SAMPLES_FORMAT, "quick", &runs)?;
    let head = Record::new(&samples, "head", "macos-aarch64", binary(), Tier::Quick)?;
    let base = Record::new(&samples, "base", "macos-aarch64", binary(), Tier::Quick)?;
    assert_eq!(head.series[0].summary.wall_s.p50, 0.5);
    assert_eq!(base.series[0].summary.wall_s.p50, 9.0);
    assert!(head.refused.is_empty());
    assert_eq!(base.refused.len(), 1);
    assert_eq!(
        base.refused[0].key.to_string(),
        "rest-json-single https 0 ms native"
    );
    assert_eq!((head.first_run_s, base.first_run_s), (Some(1.3), Some(1.2)));
    Ok(())
}

#[test]
fn samples_of_another_tier_or_format_are_refused() -> Result<(), TaskError> {
    let runs = [run(0.5, 1)];
    let full = samples(SAMPLES_FORMAT, "full", &runs)?;
    let error = Record::new(&full, "head", "linux-x86_64", binary(), Tier::Quick);
    assert_eq!(
        error.err().map(|error| error.to_string()),
        Some("perf samples are of the full tier, not the quick tier".to_owned())
    );
    let old = samples(1, "quick", &runs)?;
    let error = Record::new(&old, "head", "linux-x86_64", binary(), Tier::Quick);
    assert_eq!(
        error.err().map(|error| error.to_string()),
        Some("perf samples have format 1, expected 3".to_owned())
    );
    Ok(())
}

#[test]
fn the_result_file_reads_back_as_written_and_a_baseline_drops_the_runs() -> Result<(), TaskError> {
    let record = record()?;
    let read: Record = serde_json::from_str(&record.to_json()?).map_err(json_error)?;
    assert_eq!(read, record);
    let text = record.without_runs().to_json()?;
    assert!(!text.contains("\"runs\": ["), "{text}");
    let read: Record = serde_json::from_str(&text).map_err(json_error)?;
    assert_eq!(read.series[0].summary, record.series[0].summary);
    assert!(read.series[0].runs.is_empty());
    let comparison = Comparison::new(record.clone(), record);
    let read: Comparison = serde_json::from_str(&comparison.to_json()?).map_err(json_error)?;
    assert_eq!(read, comparison);
    Ok(())
}

#[test]
fn a_series_without_runs_is_refused() -> Result<(), TaskError> {
    let samples = samples(SAMPLES_FORMAT, "full", &[])?;
    let error = Record::new(&samples, "head", "linux-x86_64", binary(), Tier::Full);
    assert_eq!(
        error.err().map(|error| error.to_string()),
        Some("perf series json-pages-4 https 20 ms native has no runs".to_owned())
    );
    Ok(())
}

fn placed(position: u32) -> Run {
    Run {
        wall_s: 0.1,
        user_s: 0.0,
        sys_s: 0.0,
        max_rss_bytes: 1.0,
        connections: 1,
        requests: 1,
        response_bytes: 1.0,
        stdout_bytes: 1.0,
        load_average: 1.0,
        position,
    }
}

#[test]
fn a_comparison_s_runs_must_pair_up_one_of_each_order() -> Result<(), TaskError> {
    let record = record()?;
    let with_runs = |positions: &[u32]| {
        let mut record = record.clone();
        record.series[0].runs = positions.iter().copied().map(placed).collect();
        record
    };
    let paired = Comparison::new(with_runs(&[0, 1, 0, 1]), with_runs(&[1, 0, 1, 0]));
    assert!(paired.check_pairs().is_ok());
    let odd = Comparison::new(with_runs(&[0, 1, 0]), with_runs(&[1, 0, 1]));
    assert_eq!(
        odd.check_pairs().err().map(|error| error.to_string()),
        Some(
            "perf series json-pages-4 https 20 ms native: its 3 base and 3 head runs do not \
             pair up round by round, one of each order, so the comparison cannot judge them"
                .to_owned()
        )
    );
    let unalternated = Comparison::new(with_runs(&[0, 0, 1, 1]), with_runs(&[1, 1, 0, 0]));
    assert!(unalternated.check_pairs().is_err());
    Ok(())
}
