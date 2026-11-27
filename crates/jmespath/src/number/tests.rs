use super::{BigInt, Number, sum};
use std::cmp::Ordering;

fn int(text: &str) -> Number {
    Number::from_int_text(text).unwrap_or(Number::Float(f64::NAN))
}

fn text(number: &Number) -> String {
    match number {
        Number::Int(value) => value.to_string(),
        Number::Big(value) => value.to_string(),
        Number::Float(value) => format!("{value:?}"),
    }
}

#[test]
fn big_integers_read_and_print_exactly() {
    for digits in [
        "0",
        "-1",
        "9223372036854775807",
        "-9223372036854775808",
        "1000000000",
        "-1000000000000000000000000000001",
    ] {
        assert_eq!(text(&int(digits)), digits);
    }
    assert!(matches!(int("9223372036854775807"), Number::Int(_)));
    assert!(matches!(int("9223372036854775808"), Number::Big(_)));
    assert_eq!(BigInt::parse("-"), None);
    assert_eq!(BigInt::parse("1e3"), None);
    assert_eq!(text(&int("-000")), "0");
}

#[test]
fn addition_carries_and_borrows_across_limbs() -> Result<(), crate::Error> {
    let add = |a: &str, b: &str| int(a).add(&int(b)).map(|n| text(&n));
    assert_eq!(add("9223372036854775807", "1")?, "9223372036854775808");
    assert_eq!(add("999999999999999999999", "1")?, "1000000000000000000000");
    assert_eq!(
        add("1000000000000000000000", "-1")?,
        "999999999999999999999"
    );
    assert_eq!(add("-9223372036854775809", "9223372036854775809")?, "0");
    assert_eq!(
        add("5", "-1000000000000000000000")?,
        "-999999999999999999995"
    );
    assert_eq!(
        text(&int("-9223372036854775808").abs()),
        "9223372036854775808"
    );
    Ok(())
}

#[test]
fn ints_and_floats_compare_exactly() {
    let cmp = |a: Number, b: Number| a.compare(&b);
    let p53 = || int("9007199254740993");
    assert_eq!(
        cmp(p53(), Number::Float(9_007_199_254_740_992.0)),
        Some(Ordering::Greater)
    );
    assert_eq!(cmp(Number::Float(-2.5), int("-2")), Some(Ordering::Less));
    assert_eq!(cmp(int("-3"), Number::Float(-2.5)), Some(Ordering::Less));
    assert_eq!(
        cmp(int("100000000000000000000"), Number::Float(1e20)),
        Some(Ordering::Equal)
    );
    assert_eq!(cmp(int("1"), Number::Float(f64::NAN)), None);
    assert_eq!(
        cmp(
            int("-99999999999999999999999"),
            Number::Float(f64::NEG_INFINITY)
        ),
        Some(Ordering::Greater)
    );
}

#[test]
fn rounding_a_float_gives_its_exact_integer() -> Result<(), crate::Error> {
    assert_eq!(
        text(&Number::Float(1e20).round(true)?),
        "100000000000000000000"
    );
    assert_eq!(text(&Number::Float(-2.5).round(false)?), "-3");
    assert_eq!(
        text(&Number::Float(2f64.powi(80)).round(true)?),
        "1208925819614629174706176"
    );
    assert_eq!(
        Number::Float(f64::NAN).round(true).map_err(|e| e.class()),
        Err("ValueError")
    );
    assert_eq!(
        Number::Float(f64::INFINITY)
            .round(false)
            .map_err(|e| e.class()),
        Err("OverflowError")
    );
    Ok(())
}

#[test]
fn sum_follows_cpython() -> Result<(), crate::Error> {
    let total = |items: &[Number]| sum(items).map(|n| text(&n));
    assert_eq!(total(&[])?, "0");
    assert_eq!(
        total(&[Number::Float(0.1), Number::Float(0.2), Number::Float(0.3)])?,
        "0.6"
    );
    let big = int("9223372036854775807");
    assert_eq!(
        total(&[big.clone(), Number::Int(1), Number::Float(1.5)])?,
        "9.223372036854776e18"
    );
    assert_eq!(
        total(&[
            Number::Float(1e16),
            Number::Int(1),
            Number::Float(1.0),
            Number::Float(-1e16)
        ])?,
        "2.0"
    );
    let huge = int(&format!("1{}", "0".repeat(400)));
    assert_eq!(
        sum(&[Number::Float(1.0), huge]).map_err(|e| e.class()),
        Err("OverflowError")
    );
    Ok(())
}
