use super::*;

fn close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 1e-9
}

#[test]
fn the_median_of_an_odd_series_is_its_middle_value() {
    assert_eq!(percentile(&[5.0, 1.0, 3.0], 0.5), Some(3.0));
}

#[test]
fn the_median_of_an_even_series_is_the_mean_of_its_middle_pair() {
    assert_eq!(percentile(&[4.0, 1.0, 3.0, 2.0], 0.5), Some(2.5));
}

#[test]
fn percentiles_interpolate_between_the_nearest_ranks() {
    let values: Vec<f64> = (1..=10).map(f64::from).collect();
    // numpy.percentile([1..10], 95) is 9.55.
    assert!(close(percentile(&values, 0.95).unwrap_or_default(), 9.55));
    assert_eq!(percentile(&values, 0.0), Some(1.0));
    assert_eq!(percentile(&values, 1.0), Some(10.0));
}

#[test]
fn a_single_value_is_every_percentile_and_no_values_have_none() {
    assert_eq!(percentile(&[7.0], 0.95), Some(7.0));
    assert_eq!(percentile(&[], 0.5), None);
    assert_eq!(Spread::of(&[]), None);
}

#[test]
fn the_spread_ignores_an_outlier_in_its_median_and_deviation() {
    let spread = Spread::of(&[10.0, 11.0, 9.0, 10.0, 100.0]);
    let fields = spread.map(|spread| (spread.p50, spread.mad, spread.n, spread.min, spread.max));
    assert_eq!(fields, Some((10.0, 1.0, 5, 9.0, 100.0)));
    // The rank is 3.8: 11 + 0.8 * (100 - 11).
    assert!(spread.is_some_and(|spread| close(spread.p95, 82.2)));
}

#[test]
fn the_median_error_shrinks_with_the_square_root_of_the_runs() {
    let four = Spread {
        p50: 0.0,
        p95: 0.0,
        mad: 1.0,
        min: 0.0,
        max: 0.0,
        n: 4,
    };
    let sixteen = Spread { n: 16, ..four };
    assert!(close(four.median_error(), 1.4826 * 1.2533 / 2.0));
    assert!(close(four.median_error() / sixteen.median_error(), 2.0));
}
