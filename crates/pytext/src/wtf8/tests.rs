use super::*;

#[test]
fn text_without_a_surrogate_is_a_string() {
    assert_eq!(
        Wtf8Buf::from_code_points("aé😀".chars().map(u32::from)),
        Ok("aé😀".to_owned())
    );
    assert_eq!(
        Wtf8Buf::from_bytes("aé".as_bytes().to_vec()),
        Ok("aé".to_owned())
    );
}

/// A lone surrogate is held, and its code points come back as they went in;
/// a high surrogate then a low one stay two code points, as in Python.
#[test]
fn surrogates_are_held_as_python_holds_them() -> Result<(), &'static str> {
    let points = [0x61, 0xD800, 0x62, 0xDCFF, 0x10000];
    let Err(text) = Wtf8Buf::from_code_points(points) else {
        return Err("a surrogate makes no String");
    };
    assert_eq!(text.code_points().collect::<Vec<_>>(), points);
    assert_eq!(text.len(), 5);
    let Err(pair) = Wtf8Buf::from_code_points([0xD800, 0xDC00]) else {
        return Err("a pair of surrogates is two code points");
    };
    assert_eq!(pair.len(), 2);
    assert_eq!(
        code_points(pair.as_bytes()).collect::<Vec<_>>(),
        [0xD800, 0xDC00]
    );
    assert_eq!(
        Wtf8Buf::from_bytes(pair.as_bytes().to_vec()),
        Err(pair.clone())
    );
    assert_eq!(format!("{pair:?}"), "Wtf8Buf(\"\\ud800\\udc00\")");
    assert_eq!(pair.to_string_lossy(), "\u{fffd}\u{fffd}");
    Ok(())
}

/// Comparing the bytes compares the code points, so order is Python's:
/// `'\ud800' < '' < '\U00010000'`, and `'𐀀' < '\U00010000'`.
#[test]
fn order_is_python_s() {
    let text = |points: &[u32]| {
        let mut bytes = Vec::new();
        for point in points {
            push_code_point(&mut bytes, *point);
        }
        bytes
    };
    let ordered = [
        text(&[0xD7FF]),
        text(&[0xD800]),
        text(&[0xD800, 0xDC00]),
        text(&[0xDFFF]),
        text(&[0xE000]),
        text(&[0x10000]),
    ];
    assert!(
        ordered.windows(2).all(|pair| pair[0] < pair[1]),
        "{ordered:x?}"
    );
}
