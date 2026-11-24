use super::super::record::{Binary, Exact, FORMAT, Host, Key, Summary, Tier};
use super::*;

const MIB: f64 = 1024.0 * 1024.0;

fn run(wall: f64, rss_mib: f64, load: f64) -> Run {
    Run {
        wall_s: wall,
        user_s: wall / 2.0,
        sys_s: 0.0,
        max_rss_bytes: rss_mib * MIB,
        connections: 1,
        requests: 1,
        response_bytes: 1.0,
        stdout_bytes: 1.0,
        load_average: load,
        position: 0,
    }
}

fn series(latency_ms: u32, runs: Vec<Run>) -> Series {
    let of = |measure: fn(&Run) -> f64| {
        Spread::of(&runs.iter().map(measure).collect::<Vec<_>>()).unwrap_or(Spread {
            p50: 0.0,
            p95: 0.0,
            mad: 0.0,
            min: 0.0,
            max: 0.0,
            n: 0,
        })
    };
    Series {
        key: Key {
            case: "query-single".into(),
            scheme: "https".into(),
            latency_ms,
            route: "native".into(),
        },
        pages: 1,
        load_average: [1.0, 1.0],
        summary: Summary {
            wall_s: of(gate::wall),
            cpu_s: of(gate::cpu),
            user_s: of(|run| run.user_s),
            sys_s: of(|run| run.sys_s),
            max_rss_bytes: of(|run| run.max_rss_bytes),
            connections: Exact { min: 1, max: 1 },
            requests: Exact { min: 1, max: 1 },
            response_bytes: 1.0,
            stdout_bytes: 1.0,
            requests_per_s: 1.0,
            pages_per_s: 1.0,
            mb_per_s: 1.0,
        },
        runs,
    }
}

fn record(series: Vec<Series>) -> Record {
    Record {
        format: FORMAT,
        platform: "macos-aarch64".into(),
        host: Host {
            system: "Darwin".into(),
            release: "27".into(),
            machine: "arm64".into(),
            os: "macOS".into(),
            cpu: "M4".into(),
            cpus: 12,
        },
        binary: Binary {
            path: "aws".into(),
            commit: None,
            dirty: None,
        },
        tier: Tier::Quick,
        started_unix_s: 0.0,
        warmup: 2,
        runs: 3,
        first_run_s: None,
        series,
        refused: Vec::new(),
    }
}

/// A repeat whose head runs `head` times the base's 14 ms wall time, with
/// least peaks of 20 MiB and `head_rss` MiB.
fn repeat(head: f64, head_rss: f64, load: f64) -> (Record, Record) {
    let runs = |scale: f64, rss: f64, first: u32| {
        (0..4)
            .map(|round| Run {
                position: (round + first) % 2,
                ..run(0.014 * scale, rss, load)
            })
            .collect()
    };
    (
        record(vec![series(0, runs(1.0, 20.0, 0))]),
        record(vec![series(0, runs(head, head_rss, 1))]),
    )
}

fn close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 1e-9
}

#[test]
fn a_comparison_deviation_is_the_estimate_of_the_head_against_the_base() {
    let repeats = [repeat(1.02, 20.25, 1.0)];
    let wall = comparison_deviations(&repeats, Metric::Wall);
    assert_eq!(wall.len(), 1);
    assert!(close(wall[0].reference, 0.014) && close(wall[0].amount, 0.014 * 0.02));
    let rss = comparison_deviations(&repeats, Metric::Rss);
    assert!(close(rss[0].amount, 0.25 * MIB) && close(rss[0].reference, 20.0 * MIB));
}

#[test]
fn a_baseline_deviation_is_the_spread_of_every_measurement_s_median() {
    let repeats = [repeat(1.02, 20.0, 1.0), repeat(0.99, 20.0, 1.0)];
    let wall = baseline_deviations(&repeats, Metric::Wall);
    // Four medians, 14 ms, 14.28 ms, 14 ms and 13.86 ms.
    assert_eq!(wall.len(), 1);
    assert!(close(wall[0].amount, 0.014 * 0.03));
}

#[test]
fn a_tolerance_covers_three_times_every_deviation() {
    let deviations = [
        // A 14 ms series off by 0.4 ms: the floor, 1.2 ms rounded up.
        Deviation {
            amount: 0.0004,
            reference: 0.014,
            resolution: 0.0,
        },
        // A 2 s series off by 20 ms: 3% of it.
        Deviation {
            amount: 0.02,
            reference: 2.0,
            resolution: 0.0,
        },
    ];
    let tolerance = tolerance(&deviations, SMALL_SECONDS, TIME_STEP);
    assert!(close(tolerance.absolute, 0.0015));
    assert!(close(tolerance.relative, 0.03));
    let none = super::tolerance(&[], SMALL_SECONDS, TIME_STEP);
    assert_eq!((none.relative, none.absolute), (0.01, TIME_STEP));
}

#[test]
fn cpu_is_calibrated_only_on_series_without_latency() {
    let (base, head) = repeat(1.0, 20.0, 1.0);
    let latent = |record: &Record| {
        let mut record = record.clone();
        record.series[0].key.latency_ms = 20;
        record
    };
    let repeats = [(latent(&base), latent(&head))];
    assert!(comparison_deviations(&repeats, Metric::Cpu).is_empty());
    assert_eq!(comparison_deviations(&repeats, Metric::Wall).len(), 1);
}

#[test]
fn derived_tolerances_and_the_load_range_come_from_every_repeat() {
    let repeats = [repeat(1.0, 20.0, 1.2), repeat(1.1, 20.0, 1.6)];
    let (comparison, _) = derive(&repeats);
    // The second head is 1.4 ms off a 14 ms base: under 50 ms, so the floor.
    assert!(close(comparison.wall.absolute, 0.0045));
    assert_eq!(load_range(&repeats), [1.2, 1.6]);
}

#[test]
fn a_tolerance_also_covers_one_and_a_half_times_the_widest_resolution() {
    // No deviation at all, but the estimate resolved only 2 ms: a check
    // must be allowed 3 ms to resolve its bound.
    let deviations = [Deviation {
        amount: 0.0,
        reference: 0.03,
        resolution: 0.002,
    }];
    let tolerance = tolerance(&deviations, SMALL_SECONDS, TIME_STEP);
    assert!(close(tolerance.absolute, 0.003));
}

#[test]
fn a_comparison_deviation_carries_its_estimate_s_resolution() {
    let repeats = [repeat(1.0, 20.0, 1.0)];
    let wall = comparison_deviations(&repeats, Metric::Wall);
    // Identical runs: no deviation, and nothing unresolved.
    assert_eq!(wall[0].amount, 0.0);
    assert_eq!(wall[0].resolution, 0.0);
}
