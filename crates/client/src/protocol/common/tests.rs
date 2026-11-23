use super::*;
use crate::http::Headers;

fn response(status: u16, body: &[u8]) -> HttpResponse {
    HttpResponse {
        status,
        headers: Headers::from_pairs([
            ("X-Amzn-RequestId", "r1"),
            ("Set-Cookie", "a"),
            ("set-cookie", "b"),
        ]),
        body: body.to_vec(),
    }
}

/// `_is_generic_error_response` and `_do_generic_error_parse`: a 5xx with
/// no body or an HTML one is the status and its reason phrase.
#[test]
fn a_bodiless_or_html_5xx_is_the_generic_error() -> Result<(), &'static str> {
    assert!(is_generic_error(&response(503, b"  ")));
    assert!(is_generic_error(&response(
        500,
        b"\n<html><body>x</body></html>"
    )));
    assert!(!is_generic_error(&response(500, b"<Error/>")));
    assert!(!is_generic_error(&response(404, b"")));
    let parsed = generic_error(503);
    let Some(Value::Map(error)) = parsed.get("Error") else {
        return Err("no Error");
    };
    assert_eq!(error.get("Code"), Some(&Value::String("503".into())));
    assert_eq!(
        error.get("Message"),
        Some(&Value::String("Service Unavailable".into()))
    );
    assert_eq!(reason_phrase(599), "");
    Ok(())
}

/// `lowercase_dict` of headers as urllib3 joins a repeated one.
#[test]
fn response_headers_lowercase_and_join() {
    let headers = lowercase_headers(&response(200, b"").headers);
    assert_eq!(
        headers.get("x-amzn-requestid"),
        Some(&Value::String("r1".into()))
    );
    assert_eq!(
        headers.get("set-cookie"),
        Some(&Value::String("a, b".into()))
    );
}

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
    assert_eq!(decode_blob("AP8="), Ok(Value::Blob(vec![0, 255])));
    assert!(decode_blob("A").is_err());
}
