use super::*;
use serde_json::json;

fn json_error(error: serde_json::Error) -> TaskError {
    deps::error(error.to_string())
}

fn run(wall_s: f64, connections: u32) -> serde_json::Value {
    json!({
        "wall_s": wall_s, "user_s": 0.01, "sys_s": 0.005, "max_rss_bytes": 2.0e7,
        "connections": connections, "requests": 4, "response_bytes": 2.0e6,
        "stdout_bytes": 100.0
    })
}

/// measure.py's file for one four-page series of three runs.
fn samples(format: u32, runs: &[serde_json::Value]) -> Result<Samples, TaskError> {
    let value = json!({
        "format": format,
        "host": {
            "system": "Darwin", "release": "27.2.0", "machine": "arm64",
            "os": "macOS-27.2.1-arm64", "cpu": "Apple M4 Pro", "cpus": 12
        },
        "started_unix_s": 1.0,
        "warmup": 2,
        "runs": 3,
        "series": [{
            "case": "json-pages-4", "scheme": "https", "latency_ms": 20,
            "route": "native", "pages": 4, "load_average": [1.5, 1.75],
            "runs": runs
        }]
    });
    serde_json::from_value(value).map_err(json_error)
}

fn binary() -> Binary {
    Binary {
        path: "/target/aws".into(),
        commit: Some("abc".into()),
        dirty: Some(false),
    }
}

fn record() -> Result<Record, TaskError> {
    let runs = [run(0.5, 1), run(0.4, 1), run(0.6, 2)];
    Record::new(
        samples(SAMPLES_FORMAT, &runs)?,
        "macos-aarch64",
        binary(),
        Tier::Full,
    )
}

#[test]
fn a_series_is_summarised_over_its_runs() -> Result<(), TaskError> {
    let record = record()?;
    let series = &record.series[0];
    assert_eq!(series.key.to_string(), "json-pages-4 https 20 ms native");
    let summary = &series.summary;
    assert_eq!((summary.wall_s.p50, summary.wall_s.n), (0.5, 3));
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
    assert_eq!(series.load_average, [1.5, 1.75]);
    assert_eq!(series.runs.len(), 3);
    assert_eq!(
        (record.platform.as_str(), record.tier),
        ("macos-aarch64", Tier::Full)
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
    Ok(())
}

#[test]
fn another_format_is_refused_in_samples_and_baselines() -> Result<(), TaskError> {
    let error = Record::new(
        samples(2, &[run(0.5, 1)])?,
        "linux-x86_64",
        binary(),
        Tier::Quick,
    );
    assert_eq!(
        error.err().map(|error| error.to_string()),
        Some("perf samples have format 2, expected 1".to_owned())
    );
    let record = record()?;
    let text =
        Baseline::new(&record, &record)
            .to_json()?
            .replacen("\"format\": 2", "\"format\": 9", 1);
    assert_eq!(
        Baseline::parse(&text, "the baseline")
            .err()
            .map(|error| error.to_string()),
        Some("the baseline has format 9, expected 2".to_owned())
    );
    assert!(Baseline::parse("{}", "the baseline").is_err());
    Ok(())
}

#[test]
fn a_series_without_runs_is_refused() -> Result<(), TaskError> {
    let error = Record::new(
        samples(SAMPLES_FORMAT, &[])?,
        "linux-x86_64",
        binary(),
        Tier::Quick,
    );
    assert_eq!(
        error.err().map(|error| error.to_string()),
        Some("perf series json-pages-4 https 20 ms native has no runs".to_owned())
    );
    Ok(())
}

#[test]
fn a_baseline_holds_a_record_of_each_tier_and_reads_back() -> Result<(), TaskError> {
    let full = record()?;
    let mut quick = record()?;
    quick.tier = Tier::Quick;
    let baseline = Baseline::new(&quick, &full);
    let text = baseline.to_json()?;
    let read = Baseline::parse(&text, "baseline")?;
    assert_eq!(read, baseline);
    assert_eq!(read.section(Tier::Quick).tier, Tier::Quick);
    assert_eq!(read.section(Tier::Full).tier, Tier::Full);
    assert!(read.section(Tier::Full).series[0].runs.is_empty());
    Ok(())
}

#[test]
fn a_baseline_with_a_section_of_the_wrong_tier_is_refused() -> Result<(), TaskError> {
    let full = record()?;
    let text = Baseline::new(&full, &full).to_json()?;
    assert_eq!(
        Baseline::parse(&text, "the baseline")
            .err()
            .map(|error| error.to_string()),
        Some("the baseline: its quick section is not a format 2 quick record".to_owned())
    );
    assert!(Baseline::parse(&full.to_json()?, "a result").is_err());
    Ok(())
}
