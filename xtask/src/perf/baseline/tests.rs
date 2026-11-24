use super::super::gate::Tolerance;
use super::super::record::Binary;
use super::*;

fn host() -> Host {
    Host {
        system: "Darwin".into(),
        release: "27".into(),
        machine: "arm64".into(),
        os: "macOS".into(),
        cpu: "Apple M4 Pro".into(),
        cpus: 12,
    }
}

fn record(tier: Tier) -> Record {
    Record {
        format: FORMAT,
        platform: "macos-aarch64".into(),
        host: host(),
        binary: Binary {
            path: "target/aws".into(),
            commit: Some("abc".into()),
            dirty: Some(false),
        },
        tier,
        started_unix_s: 0.0,
        warmup: 2,
        runs: 10,
        first_run_s: Some(1.3),
        series: Vec::new(),
        refused: Vec::new(),
    }
}

fn calibration() -> Calibration {
    let tolerance = Tolerance {
        relative: 0.03,
        absolute: 0.0005,
    };
    let tolerances = Tolerances {
        wall: tolerance,
        cpu: tolerance,
        rss: Tolerance {
            relative: 0.01,
            absolute: 512.0 * 1024.0,
        },
    };
    Calibration {
        unix_s: 1.0,
        commit: Some("abc".into()),
        host: host(),
        load_average: [1.1, 1.6],
        repeats: 5,
        comparison: tolerances,
        baseline: tolerances,
    }
}

fn scratch(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "awscli-perf-baseline-{}-{name}",
        std::process::id()
    ))
}

#[test]
fn a_baseline_file_reads_back_with_each_tier_s_calibration_and_record() -> Result<(), TaskError> {
    let mut baseline = Baseline::default();
    baseline.section_mut(Tier::Quick).calibration = Some(calibration());
    baseline.section_mut(Tier::Quick).record = Some(record(Tier::Quick));
    baseline.section_mut(Tier::Full).record = Some(record(Tier::Full));
    let path = scratch("round-trip").join("macos-aarch64.json");
    baseline.write(&path)?;
    let read = Baseline::read(&path)?;
    fs::remove_dir_all(scratch("round-trip"))?;
    assert_eq!(read.as_ref(), Some(&baseline));
    assert!(read.is_some_and(|read| read.full.calibration.is_none()));
    Ok(())
}

#[test]
fn no_file_is_no_baseline_and_a_partial_one_reads() -> Result<(), TaskError> {
    assert_eq!(Baseline::read(&scratch("absent"))?, None);
    let read = Baseline::parse("{\"format\": 3}", "partial")?;
    assert_eq!(read, Baseline::default());
    Ok(())
}

#[test]
fn another_format_or_a_record_of_the_other_tier_is_refused() {
    let text = "{\"format\": 2}";
    assert_eq!(
        Baseline::parse(text, "the baseline")
            .err()
            .map(|error| error.to_string()),
        Some("the baseline has format 2, expected 3".to_owned())
    );
    let mut baseline = Baseline::default();
    baseline.quick.record = Some(record(Tier::Full));
    let text = serde_json::to_string(&baseline).unwrap_or_default();
    assert_eq!(
        Baseline::parse(&text, "the baseline")
            .err()
            .map(|error| error.to_string()),
        Some("the baseline: its quick record is not a format 3 quick record".to_owned())
    );
}
