use super::super::record::{Binary, FORMAT, Host, Refusal, Summary, Tier};
use super::*;

const MIB: f64 = 1024.0 * 1024.0;
const ONE: Exact = Exact { min: 1, max: 1 };

/// 5% and at least 1 ms for times, 5% and at least 1 MiB for the least
/// peak: tolerances fixed for these tests.
const TOLERANCES: Tolerances = Tolerances {
    wall: Tolerance {
        relative: 0.05,
        absolute: 0.001,
    },
    cpu: Tolerance {
        relative: 0.05,
        absolute: 0.001,
    },
    rss: Tolerance {
        relative: 0.05,
        absolute: MIB,
    },
};

fn key(latency_ms: u32) -> Key {
    Key {
        case: "json-pages-100".into(),
        scheme: "https".into(),
        latency_ms,
        route: "native".into(),
    }
}

/// A run of `wall` seconds, a fifth of it CPU, peaking at `rss_mib`.
fn run(wall: f64, rss_mib: f64, load: f64) -> Run {
    Run {
        wall_s: wall,
        user_s: wall * 0.15,
        sys_s: wall * 0.05,
        max_rss_bytes: rss_mib * MIB,
        connections: 1,
        requests: 100,
        response_bytes: 1.0e6,
        stdout_bytes: 1.0e5,
        load_average: load,
        position: 0,
    }
}

/// `runs` placed as a comparison places them: the base first in even
/// rounds and second in odd ones, the head the other way round.
fn placed(runs: Vec<Run>, head: bool) -> Vec<Run> {
    runs.into_iter()
        .enumerate()
        .map(|(round, run)| Run {
            position: u32::from((round % 2 == 1) != head),
            ..run
        })
        .collect()
}

fn spread(values: &[f64]) -> Spread {
    Spread::of(values).unwrap_or(Spread {
        p50: 0.0,
        p95: 0.0,
        mad: 0.0,
        min: 0.0,
        max: 0.0,
        n: 0,
    })
}

fn series(latency_ms: u32, runs: Vec<Run>, connections: Exact) -> Series {
    let of = |measure: fn(&Run) -> f64| spread(&runs.iter().map(measure).collect::<Vec<_>>());
    Series {
        key: key(latency_ms),
        pages: 100,
        load_average: [1.0, 1.0],
        summary: Summary {
            wall_s: of(wall),
            cpu_s: of(cpu),
            user_s: of(|run| run.user_s),
            sys_s: of(|run| run.sys_s),
            max_rss_bytes: of(|run| run.max_rss_bytes),
            connections,
            requests: Exact { min: 100, max: 100 },
            response_bytes: 1.0e6,
            stdout_bytes: 1.0e5,
            requests_per_s: 1.0,
            pages_per_s: 1.0,
            mb_per_s: 1.0,
        },
        runs,
    }
}

/// Ten runs whose wall times drift up by 3% a round, scaled by `scale`.
fn drifting(scale: f64, rss_mib: &[f64], load: f64) -> Vec<Run> {
    (0..10)
        .map(|round| {
            let drift = 1.0 + 0.03 * f64::from(round);
            let rss = rss_mib[usize::try_from(round).unwrap_or(0) % rss_mib.len()];
            run(0.035 * drift * scale, rss, load)
        })
        .collect()
}

/// Ten runs of the same wall time, scaled by `scale`.
fn steady(scale: f64) -> Vec<Run> {
    (0..10).map(|_| run(0.035 * scale, 50.0, 1.0)).collect()
}

fn record(cpu: &str, series: Vec<Series>) -> Record {
    Record {
        format: FORMAT,
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
        tier: Tier::Quick,
        started_unix_s: 0.0,
        warmup: 2,
        runs: 10,
        first_run_s: None,
        series,
        refused: Vec::new(),
    }
}

fn verdicts(findings: &[Finding]) -> Vec<(Metric, Verdict)> {
    findings
        .iter()
        .map(|finding| (finding.metric, finding.verdict.clone()))
        .collect()
}

fn comparison(base: Vec<Run>, head: Vec<Run>, limit: Option<f64>) -> Vec<Finding> {
    let base = record("M4", vec![series(0, placed(base, false), ONE)]);
    let head = record("M4", vec![series(0, placed(head, true), ONE)]);
    judge_comparison(&base, &head, Some(TOLERANCES), limit)
}

fn metric(findings: &[Finding], metric: Metric) -> Option<Verdict> {
    findings
        .iter()
        .find(|finding| finding.metric == metric)
        .map(|finding| finding.verdict.clone())
}

#[test]
fn drift_that_moves_both_binaries_cancels_in_a_comparison() {
    // Both drift by 27% across the rounds; a stored median would not.
    let findings = comparison(
        drifting(1.0, &[50.0], 1.0),
        drifting(1.0, &[50.0], 1.0),
        None,
    );
    assert!(
        findings
            .iter()
            .all(|finding| finding.verdict == Verdict::Within),
        "{findings:?}"
    );
}

#[test]
fn a_head_slower_in_every_round_regresses_and_a_faster_one_improves() {
    let base = drifting(1.0, &[50.0], 1.0);
    let slower = comparison(base.clone(), drifting(1.2, &[50.0], 1.0), None);
    assert!(matches!(
        metric(&slower, Metric::Wall),
        Some(Verdict::Regressed { .. })
    ));
    assert!(matches!(
        metric(&slower, Metric::Cpu),
        Some(Verdict::Regressed { .. })
    ));
    let faster = comparison(base.clone(), drifting(0.9, &[50.0], 1.0), None);
    assert!(matches!(
        metric(&faster, Metric::Wall),
        Some(Verdict::Improved { .. })
    ));
    // 3% is within the 5% bound.
    let within = comparison(base, drifting(1.03, &[50.0], 1.0), None);
    assert_eq!(metric(&within, Metric::Wall), Some(Verdict::Within));
}

#[test]
fn scattered_ratios_are_too_noisy_to_judge() {
    let base = drifting(1.0, &[50.0], 1.0);
    let head: Vec<Run> = base
        .iter()
        .enumerate()
        .map(|(round, run)| {
            // A different change in each pair of rounds: no position effect,
            // which the crossover would cancel, but scatter.
            let scale = [1.4, 0.7, 1.2, 0.8, 1.0][round / 2];
            Run {
                wall_s: run.wall_s * scale,
                ..run.clone()
            }
        })
        .collect();
    let verdict = metric(&comparison(base, head, None), Metric::Wall);
    assert!(
        matches!(verdict, Some(Verdict::Noisy { .. })),
        "{verdict:?}"
    );
    assert!(verdict.is_some_and(|verdict| verdict.inconclusive() && !verdict.fails()));
}

#[test]
fn a_bimodal_peak_regresses_only_when_both_ends_grow() {
    let base = drifting(1.0, &[51.4], 1.0);
    // Most runs of the head land in an upper mode; its least peak holds.
    let bimodal = comparison(base.clone(), drifting(1.0, &[56.0, 56.0, 51.5], 1.0), None);
    assert_eq!(metric(&bimodal, Metric::Rss), Some(Verdict::Within));
    let grown = comparison(base, drifting(1.0, &[56.0], 1.0), None);
    assert!(matches!(
        metric(&grown, Metric::Rss),
        Some(Verdict::Regressed { .. })
    ));
}

#[test]
fn a_rare_low_mode_in_one_of_two_builds_of_one_source_is_no_change() {
    // Two builds of one commit, measured: every run of one peaked at 56.0
    // MiB, and one run of the other reached a lower mode, 51.4 MiB.
    let mut rare = vec![56.0; 10];
    rare[3] = 51.4;
    let without = drifting(1.0, &[56.0], 1.0);
    let with = drifting(1.0, &rare, 1.0);
    for (base, head) in [(without.clone(), with.clone()), (with, without)] {
        let findings = comparison(base, head, None);
        assert_eq!(metric(&findings, Metric::Rss), Some(Verdict::Within));
    }
}

#[test]
fn ends_share_the_smaller_change_when_they_move_the_same_way() {
    assert_eq!(common_shift(4.0, 1.0), 1.0);
    assert_eq!(common_shift(-1.0, -3.0), -1.0);
    assert_eq!(common_shift(-4.6, 0.0), 0.0);
    assert_eq!(common_shift(4.6, -0.1), 0.0);
}

#[test]
fn a_loaded_machine_is_not_judged_but_its_counts_are() {
    let base = record(
        "M4",
        vec![series(0, placed(drifting(1.0, &[50.0], 9.0), false), ONE)],
    );
    let more = Exact { min: 100, max: 100 };
    let head = record(
        "M4",
        vec![series(0, placed(drifting(2.0, &[50.0], 9.0), true), more)],
    );
    let findings = judge_comparison(&base, &head, Some(TOLERANCES), Some(4.0));
    let load = Verdict::Loaded {
        load: 9.0,
        limit: 4.0,
    };
    assert_eq!(metric(&findings, Metric::Wall), Some(load.clone()));
    assert_eq!(metric(&findings, Metric::Rss), Some(load.clone()));
    assert!(load.inconclusive() && !load.fails());
    let connections = metric(&findings, Metric::Connections);
    assert!(connections.is_some_and(|verdict| verdict.fails()));
}

#[test]
fn a_series_the_base_refused_is_reported_and_not_judged() {
    let mut base = record("M4", Vec::new());
    base.refused.push(Refusal {
        key: key(0),
        reason: "exited 255".into(),
    });
    let head = record("M4", vec![series(0, drifting(1.0, &[50.0], 1.0), ONE)]);
    let findings = judge_comparison(&base, &head, Some(TOLERANCES), None);
    assert_eq!(
        verdicts(&findings),
        [(Metric::Series, Verdict::RefusedByBase("exited 255".into()))]
    );
    assert!(!findings[0].verdict.fails() && !findings[0].verdict.inconclusive());
}

#[test]
fn without_a_calibration_only_the_counts_are_judged() {
    let base = record("M4", vec![series(0, drifting(1.0, &[50.0], 1.0), ONE)]);
    let head = record("M4", vec![series(0, drifting(3.0, &[90.0], 1.0), ONE)]);
    let findings = judge_comparison(&base, &head, None, None);
    let uncalibrated = Verdict::NotJudged(Unjudged::Uncalibrated);
    assert_eq!(
        verdicts(&findings),
        [
            (Metric::Connections, Verdict::Within),
            (Metric::Requests, Verdict::Within),
            (Metric::Wall, uncalibrated.clone()),
            (Metric::Cpu, uncalibrated.clone()),
            (Metric::Rss, uncalibrated),
        ]
    );
}

#[test]
fn cpu_time_is_not_judged_on_a_series_with_injected_latency() {
    let base = record(
        "M4",
        vec![series(20, placed(drifting(1.0, &[50.0], 1.0), false), ONE)],
    );
    let head = record(
        "M4",
        vec![series(20, placed(drifting(1.0, &[50.0], 1.0), true), ONE)],
    );
    let metrics: Vec<Metric> = judge_comparison(&base, &head, Some(TOLERANCES), None)
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

#[test]
fn a_baseline_check_judges_medians_and_reports_missing_series() {
    let stored = record(
        "M4",
        vec![series(0, steady(1.0), ONE), series(20, steady(1.0), ONE)],
    );
    let now = record("M4", vec![series(0, steady(1.2), ONE)]);
    let findings = judge_baseline(&stored, &now, Some(TOLERANCES), None, true);
    assert!(matches!(
        metric(&findings, Metric::Wall),
        Some(Verdict::Regressed { .. })
    ));
    let missing = findings
        .iter()
        .find(|finding| finding.verdict == Verdict::Missing);
    assert!(missing.is_some_and(|finding| finding.key == key(20) && finding.verdict.fails()));
    let partial = judge_baseline(&stored, &now, Some(TOLERANCES), None, false);
    assert!(
        partial
            .iter()
            .all(|finding| finding.verdict != Verdict::Missing)
    );
}

#[test]
fn a_baseline_from_another_cpu_judges_only_the_counts() {
    let stored = record("M1", vec![series(0, steady(1.0), ONE)]);
    let now = record("M4", vec![series(0, steady(3.0), ONE)]);
    let findings = judge_baseline(&stored, &now, Some(TOLERANCES), None, true);
    assert_eq!(
        metric(&findings, Metric::Wall),
        Some(Verdict::NotJudged(Unjudged::OtherCpu))
    );
    assert_eq!(
        metric(&findings, Metric::Connections),
        Some(Verdict::Within)
    );
}

#[test]
fn a_stored_median_needs_runs_that_resolve_its_bound() {
    let tight = |p50: f64, mad: f64| Spread {
        p50,
        p95: p50,
        mad,
        min: p50,
        max: p50,
        n: 10,
    };
    let bound = Tolerance {
        relative: 0.2,
        absolute: 0.005,
    };
    assert!(matches!(
        median(tight(0.02, 0.004), tight(0.02, 0.004), bound),
        Verdict::Noisy { .. }
    ));
    assert_eq!(
        median(tight(0.02, 0.001), tight(0.024, 0.001), bound),
        Verdict::Within
    );
    assert!(median(tight(0.02, 0.0), tight(0.026, 0.0), bound).fails());
}

#[test]
fn the_load_limit_adds_its_headroom_per_cpu() {
    let limit = LoadLimit {
        calibrated: 1.6,
        cpus: 12,
        headroom: COMPARISON_HEADROOM,
    };
    assert!((limit.value() - 13.6).abs() < 1e-9);
    let limit = LoadLimit {
        headroom: BASELINE_HEADROOM,
        ..limit
    };
    assert!((limit.value() - 4.6).abs() < 1e-9);
}

/// The native `json-pages-100` series at 0 ms of a comparison of two
/// builds on an idle machine, wall times in ms: whichever binary ran first
/// in a round ran about 6 ms slower.
const FIRST_SLOWER_BASE: [f64; 10] = [34.8, 28.8, 33.7, 28.8, 36.2, 28.7, 35.2, 28.4, 34.4, 29.3];
const FIRST_SLOWER_HEAD: [f64; 10] = [28.8, 34.6, 28.3, 34.4, 28.9, 35.4, 28.7, 34.3, 28.3, 33.3];

fn walls(milliseconds: &[f64], scale: f64) -> Vec<Run> {
    milliseconds
        .iter()
        .map(|ms| run(ms * scale / 1e3, 50.0, 1.0))
        .collect()
}

/// 10% and at least 4.5 ms: the quick comparison's wall tolerance the
/// series above was judged with.
const MEASURED: Tolerance = Tolerance {
    relative: 0.10,
    absolute: 0.0045,
};

#[test]
fn the_crossover_cancels_the_cost_of_running_first_in_a_round() {
    let base = placed(walls(&FIRST_SLOWER_BASE, 1.0), false);
    let head = placed(walls(&FIRST_SLOWER_HEAD, 1.0), true);
    // Each round's ratio alone swings by about 20% either way.
    let estimate = crossover(&base, &head, wall);
    assert!(
        estimate.is_some_and(|estimate| estimate.resolution() < 0.001),
        "{estimate:?}"
    );
    assert_eq!(paired(&base, &head, wall, MEASURED), Verdict::Within);
    let slower = placed(walls(&FIRST_SLOWER_HEAD, 1.25), true);
    assert!(matches!(
        paired(&base, &slower, wall, MEASURED),
        Verdict::Regressed { .. }
    ));
}

#[test]
fn runs_that_do_not_pair_up_are_not_estimated() {
    let base = placed(walls(&FIRST_SLOWER_BASE, 1.0), false);
    let head = placed(walls(&FIRST_SLOWER_HEAD, 1.0), true);
    // An odd number of rounds.
    assert_eq!(crossover(&base[..9], &head[..9], wall), None);
    // Both binaries first in the same round.
    let same = placed(walls(&FIRST_SLOWER_HEAD, 1.0), false);
    assert_eq!(crossover(&base, &same, wall), None);
    // An order that does not alternate from round to round.
    let unalternated: Vec<Run> = base
        .iter()
        .map(|run| Run {
            position: 0,
            ..run.clone()
        })
        .collect();
    let opposite: Vec<Run> = head
        .iter()
        .map(|run| Run {
            position: 1,
            ..run.clone()
        })
        .collect();
    assert_eq!(crossover(&unalternated, &opposite, wall), None);
    assert!(matches!(
        paired(&base[..9], &head[..9], wall, MEASURED),
        Verdict::Noisy { .. }
    ));
}
