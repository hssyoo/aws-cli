//! Python's `str` operations on a `str` holding a lone surrogate, which
//! count, slice, join and compare code points.
use super::*;

fn text(points: &[u32]) -> Value {
    Value::from_text(Wtf8Buf::from_code_points(points.iter().copied()))
}

#[test]
fn a_lone_surrogate_s_str_counts_and_slices_by_code_point() -> Result<(), String> {
    let lone = text(&[0x61, 0xD800, 0x1F600, 0x62]);
    assert_eq!(len(&lone)?, 4);
    assert_eq!(
        slice_from(&lone, &Value::Integer(1))?,
        text(&[0xD800, 0x1F600, 0x62])
    );
    // A slice without the surrogate is a plain `str` again.
    assert_eq!(
        slice_from(&lone, &Value::Integer(2))?,
        Value::String("\u{1F600}b".into())
    );
    assert_eq!(slice_to(&lone, -2)?, text(&[0x61, 0xD800]));
    assert_eq!(slice_to(&lone, 1)?, Value::String("a".into()));
    Ok(())
}

#[test]
fn a_lone_surrogate_s_str_joins_and_compares_by_code_point() -> Result<(), String> {
    let lone = text(&[0x61, 0xD800, 0x1F600, 0x62]);
    // `'\ud800' + '\udc00'` stays two code points.
    let joined = add(&text(&[0xD800]), &text(&[0xDC00]))?;
    assert_eq!(len(&joined)?, 2);
    assert!(!eq(&joined, &Value::String("\u{10000}".into())));
    assert_eq!(
        add(&Value::String("x".into()), &lone)?,
        text(&[0x78, 0x61, 0xD800, 0x1F600, 0x62])
    );
    assert!(eq(&lone, &text(&[0x61, 0xD800, 0x1F600, 0x62])));
    assert_eq!(
        add(&lone, &Value::Integer(1)),
        Err("can only concatenate str (not \"int\") to str".into())
    );
    Ok(())
}

#[test]
fn int_of_a_lone_surrogate_s_str_is_a_value_error() {
    assert_eq!(
        int(&text(&[0x31, 0xDC80])),
        Err(PaginationError::Value(
            "invalid literal for int() with base 10: '1\\udc80'".into()
        ))
    );
}
