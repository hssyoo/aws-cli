use super::sort;

/// `sorted()` of floats with a `NaN`, in CPython 3.14: an
/// inconsistent `<` sorts as its run detection and insertion leave it.
#[test]
fn short_lists_move_as_cpython_moves_them() {
    let sorted = |items: &[f64]| {
        let mut items = items.to_vec();
        sort(&mut items, |a, b| a < b);
        format!("{items:?}")
    };
    assert_eq!(sorted(&[3.0, f64::NAN, 1.0, 2.0]), "[3.0, NaN, 1.0, 2.0]");
    assert_eq!(sorted(&[1.0, f64::NAN, 3.0, 2.0]), "[1.0, NaN, 2.0, 3.0]");
    assert_eq!(
        sorted(&[5.0, 4.0, 4.0, 3.0, 1.0, 2.0]),
        "[1.0, 2.0, 3.0, 4.0, 4.0, 5.0]"
    );
}

#[test]
fn sorting_is_stable_at_any_length() {
    for length in [10, 64, 200] {
        let mut items: Vec<(u32, usize)> = (0..length).map(|i| ((i * 7 % 5) as u32, i)).collect();
        sort(&mut items, |a, b| a.0 < b.0);
        assert!(
            items
                .windows(2)
                .all(|w| w[0].0 < w[1].0 || (w[0].0 == w[1].0 && w[0].1 < w[1].1))
        );
    }
}
