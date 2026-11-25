use super::*;

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn the_options_are_the_binary_and_the_result_file() -> Result<(), TaskError> {
    assert_eq!(options(&[])?, Options::default());
    assert_eq!(
        options(&args(&["--aws-binary", "old/aws", "--out", "old.json"]))?,
        Options {
            binary: args(&["--aws-binary", "old/aws"]),
            out: Some(PathBuf::from("old.json")),
        }
    );
    assert_eq!(
        options(&args(&["--check"]))
            .err()
            .map(|error| error.to_string()),
        Some("unknown perf option: --check".to_owned())
    );
    assert_eq!(
        options(&args(&["--out"]))
            .err()
            .map(|error| error.to_string()),
        Some("--out requires a value".to_owned())
    );
    Ok(())
}

#[test]
fn the_median_of_an_odd_or_an_even_count() {
    assert_eq!(median(vec![5.0, 1.0, 3.0]), 3.0);
    assert_eq!(median(vec![4.0, 1.0, 3.0, 2.0]), 2.5);
    assert_eq!(median(Vec::new()), 0.0);
}

/// measure.py's samples for one paged case: three native runs and three
/// Python runs.
const SAMPLES: &str = r#"{
  "format": 1, "cpu": "Apple M4 Pro", "cpus": 12, "warmup": 1, "runs": 3,
  "series": [
    {"case": "json-pages-100", "scheme": "https", "latency_ms": 0, "route": "native",
     "pages": 100, "runs": [
       {"wall_s": 0.030, "user_s": 0.015, "sys_s": 0.005, "max_rss_bytes": 22.5e6,
        "connections": 1, "requests": 100, "load_average": 1.5},
       {"wall_s": 0.032, "user_s": 0.016, "sys_s": 0.005, "max_rss_bytes": 22.6e6,
        "connections": 1, "requests": 100, "load_average": 1.25},
       {"wall_s": 0.031, "user_s": 0.016, "sys_s": 0.004, "max_rss_bytes": 22.4e6,
        "connections": 1, "requests": 100, "load_average": 2.0}]},
    {"case": "json-pages-100", "scheme": "https", "latency_ms": 0, "route": "python",
     "pages": 100, "runs": [
       {"wall_s": 0.142, "user_s": 0.120, "sys_s": 0.010, "max_rss_bytes": 76.3e6,
        "connections": 1, "requests": 100, "load_average": 1.5},
       {"wall_s": 0.141, "user_s": 0.121, "sys_s": 0.010, "max_rss_bytes": 76.4e6,
        "connections": 2, "requests": 100, "load_average": 1.5},
       {"wall_s": 0.143, "user_s": 0.122, "sys_s": 0.010, "max_rss_bytes": 76.2e6,
        "connections": 1, "requests": 100, "load_average": 1.5}]}
  ]
}"#;

#[test]
fn the_table_sets_each_route_s_medians_side_by_side() -> Result<(), TaskError> {
    let text = table("target/aws", &parse(SAMPLES)?);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[0],
        "perf: target/aws on Apple M4 Pro (12 CPUs); medians of 3 runs after 1 warm-up; \
         load average 1.25 to 2.00"
    );
    assert!(lines[1].starts_with("case            scheme  rtt ms  native ms"));
    let cells: Vec<&str> = lines[2].split_whitespace().collect();
    assert_eq!(
        cells,
        [
            "json-pages-100",
            "https",
            "0",
            "31.0",
            "142.0",
            "20.0",
            "131.0",
            "22.5",
            "76.3",
            "1/100",
            "1/100",
            "varied"
        ]
    );
    assert_eq!(lines.len(), 3);
    Ok(())
}

#[test]
fn samples_of_another_format_are_refused() {
    let other = SAMPLES.replacen("\"format\": 1", "\"format\": 2", 1);
    assert_eq!(
        parse(&other).err().map(|error| error.to_string()),
        Some("perf samples have format 2, expected 1".to_owned())
    );
}
