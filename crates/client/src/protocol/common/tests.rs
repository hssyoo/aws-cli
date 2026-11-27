use super::*;

/// `_convert_timestamp_to_str`: every input form to every output format.
#[test]
fn timestamps_serialize_in_each_format() {
    let at = Value::String("2015-08-30T12:36:00.5Z".into());
    let whole = Value::Integer(1_440_938_160);
    let cases = [
        (
            &at,
            "iso8601",
            Value::String("2015-08-30T12:36:00.500000Z".into()),
        ),
        (
            &at,
            "ISO8601",
            Value::String("2015-08-30T12:36:00.500000Z".into()),
        ),
        (&at, "unixTimestamp", Value::Float(1_440_938_160.5)),
        (&whole, "unixtimestamp", Value::Integer(1_440_938_160)),
        (
            &whole,
            "rfc822",
            Value::String("Sun, 30 Aug 2015 12:36:00 GMT".into()),
        ),
        (
            &Value::String("2015-08-30 12:36:00".into()),
            "iso8601",
            Value::String("2015-08-30T12:36:00Z".into()),
        ),
        (
            &Value::String("1440938160".into()),
            "iso8601",
            Value::String("2015-08-30T12:36:00Z".into()),
        ),
    ];
    for (value, format, expected) in cases {
        assert_eq!(
            timestamp_out(value, format),
            Ok(expected),
            "{value:?} {format}"
        );
    }
    assert!(timestamp_out(&at, "epoch").is_err());
    assert!(timestamp_out(&Value::String("soon".into()), "iso8601").is_err());
}

/// The special floats as text; blobs from text or bytes.
#[test]
fn floats_and_blobs_go_on_the_wire_as_botocore_writes_them() {
    assert_eq!(
        special_float(&Value::Float(f64::NAN)),
        Value::String("NaN".into())
    );
    assert_eq!(
        special_float(&Value::Float(f64::NEG_INFINITY)),
        Value::String("-Infinity".into())
    );
    assert_eq!(special_float(&Value::Float(1.5)), Value::Float(1.5));
    assert_eq!(base64_in(&Value::String("é".into())), Ok("w6k=".into()));
    assert_eq!(base64_in(&Value::Blob(vec![0, 255])), Ok("AP8=".into()));
    assert!(base64_in(&Value::Integer(1)).is_err());
    let text = |text: &str| Value::String(text.into());
    assert_eq!(decode_blob(&text("AP8=")), Ok(Value::Blob(vec![0, 255])));
    assert!(decode_blob(&text("A")).is_err());
}

/// `base64.b64decode` of a `str` that is not ASCII, as CPython words it,
/// a lone surrogate's included.
#[test]
fn a_blob_s_text_must_be_ascii() {
    let text = |text: &str| Value::String(text.into());
    let ascii = ParseError::new("string argument should contain only ASCII characters");
    assert_eq!(decode_blob(&text("A\u{e9}==")), Err(ascii.clone()));
    let lone = Value::from_text(model::value::Wtf8Buf::from_code_points([0x41, 0xD800]));
    assert_eq!(decode_blob(&lone), Err(ascii));
}
