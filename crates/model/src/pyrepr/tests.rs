use super::*;
use crate::testing::{self, Result};
use crate::value::{Map, Value};
use pytext::repr::str_repr;
use timestamp::Timestamp;

/// `repr()` and `str()` of what `json.loads` makes of a corpus, from CPython.
#[test]
fn repr_and_str_agree_with_python_for_json_values() -> Result<()> {
    let corpus = r#"["plain", "it's", "say \"hi\"", "both ' \"",
        "\u0000\u007f\u0080\u00a0\u00ad\u2028\ud83d\ude00\udb40\udc01\uffff\\",
        "tab\tnl\ncr\r", "é中文", 1, -12345678901234567890, 2.5, 1e16, -0.0, 1e-05, 1e400,
        true, false, null,
        [], {}, ["a", [1, {"k": "v"}]], {"a": [1, 2.5, null, true], "it's": "x"}]"#;
    let script = r#"
import json, sys
items = json.loads(sys.stdin.read())
sys.stdout.write(json.dumps([[repr(item), str(item)] for item in items]))
"#;
    let expected: Vec<(String, String)> =
        serde_json::from_slice(&testing::python(script, corpus.as_bytes())?)?;
    let Value::List(items) = Value::from_json(corpus.as_bytes())? else {
        return Err("corpus is not a list".into());
    };
    assert_eq!(items.len(), expected.len());
    for (item, (repr, text)) in items.iter().zip(&expected) {
        assert_eq!(&py_repr(item), repr);
        assert_eq!(&py_str(item), text);
    }
    Ok(())
}

#[test]
fn bytes_print_as_python_does() {
    let blob = Value::Blob(b"a'b\"c\\\x00\x7f\xff\t\n\r".to_vec());
    let cases = [
        (py_repr(&blob), r#"b'a\'b"c\\\x00\x7f\xff\t\n\r'"#),
        (py_str(&blob), r#"b'a\'b"c\\\x00\x7f\xff\t\n\r'"#),
        (py_repr(&Value::Blob(b"it's".to_vec())), r#"b"it's""#),
        (
            py_repr(&Value::List(vec![Value::Blob(b"x".to_vec())])),
            "[b'x']",
        ),
    ];
    for (actual, expected) in cases {
        assert_eq!(actual, expected);
    }
}

#[test]
fn datetimes_print_as_python_does() -> Result<()> {
    let at = |seconds: i64, micros: i64| {
        Timestamp::from_unix_micros(seconds * 1_000_000 + micros).ok_or("instant")
    };
    let micro = Value::Timestamp(at(1_577_934_245, 6)?);
    let minute = Value::Timestamp(at(1_577_934_240, 0)?);
    let second = Value::Timestamp(at(1_577_934_245, 0)?);
    let utc = "tzinfo=datetime.timezone.utc";
    let cases = [
        (
            py_str(&micro),
            "2020-01-02 03:04:05.000006+00:00".to_owned(),
        ),
        (
            py_repr(&micro),
            format!("datetime.datetime(2020, 1, 2, 3, 4, 5, 6, {utc})"),
        ),
        (py_str(&minute), "2020-01-02 03:04:00+00:00".to_owned()),
        (
            py_repr(&minute),
            format!("datetime.datetime(2020, 1, 2, 3, 4, {utc})"),
        ),
        (
            py_repr(&second),
            format!("datetime.datetime(2020, 1, 2, 3, 4, 5, {utc})"),
        ),
        (
            py_str(&Value::Timestamp(Timestamp::MIN)),
            "0001-01-01 00:00:00+00:00".to_owned(),
        ),
    ];
    for (actual, expected) in cases {
        assert_eq!(actual, expected);
    }
    Ok(())
}

#[test]
fn type_names_are_python_class_strings() {
    let mut map = Map::new();
    map.insert("k".into(), Value::Null);
    for (value, name) in [
        (Value::Null, "<class 'NoneType'>"),
        (Value::Bool(true), "<class 'bool'>"),
        (Value::Integer(1), "<class 'int'>"),
        (
            Value::BigInteger("99999999999999999999".into()),
            "<class 'int'>",
        ),
        (Value::Float(1.0), "<class 'float'>"),
        (Value::String(String::new()), "<class 'str'>"),
        (Value::Blob(Vec::new()), "<class 'bytes'>"),
        (
            Value::Timestamp(Timestamp::MIN),
            "<class 'datetime.datetime'>",
        ),
        (Value::List(Vec::new()), "<class 'list'>"),
        (Value::Map(map), "<class 'dict'>"),
    ] {
        assert_eq!(py_type(&value), name);
    }
    let at = Value::Timestamp(Timestamp::MIN);
    assert_eq!(
        (py_type_name(&at), py_short_type_name(&at)),
        ("datetime.datetime", "datetime")
    );
    let list = Value::List(Vec::new());
    assert_eq!(
        (py_type_name(&list), py_short_type_name(&list)),
        ("list", "list")
    );
}

/// `float(text)` in CPython: whitespace, signs, underscores between digits,
/// the names it knows, and what it refuses.
#[test]
fn float_from_text_reads_what_python_float_reads() {
    let cases = [
        (" 1_000.5 ", Some(1000.5)),
        ("-12e3", Some(-12_000.0)),
        ("+.5", Some(0.5)),
        ("5.", Some(5.0)),
        ("\t7\u{b}", Some(7.0)),
        ("\u{3000}7\u{a0}", Some(7.0)),
        // CPython strips C's six, not str.strip()'s separators.
        ("\u{1c}7", None),
        ("7\u{1f}", None),
        ("Infinity", Some(f64::INFINITY)),
        ("-inf", Some(f64::NEG_INFINITY)),
        ("1__0", None),
        ("_1", None),
        ("1_", None),
        ("1e", None),
        (".", None),
        ("", None),
        ("0x10", None),
        ("1 2", None),
    ];
    for (text, expected) in cases {
        assert_eq!(float_from_text(text), expected, "{text:?}");
    }
    assert!(float_from_text("NaN").is_some_and(f64::is_nan));
}

/// `int(text)` in CPython 3.14, for the ASCII digits read here, with its
/// ValueError's text.
#[test]
fn int_from_text_reads_what_python_int_reads() {
    let cases = [
        (" 12 ", Value::Integer(12)),
        ("-0", Value::Integer(0)),
        ("+5", Value::Integer(5)),
        ("1_000", Value::Integer(1000)),
        ("007", Value::Integer(7)),
        (
            "-99999999999999999999",
            Value::BigInteger("-99999999999999999999".into()),
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(int_from_text(text), Ok(expected), "{text:?}");
    }
    for text in ["1__0", "0x10", "1.5", "", "\u{1c}7", "7\u{1f}"] {
        let message = format!("invalid literal for int() with base 10: {}", str_repr(text));
        assert_eq!(int_from_text(text), Err(message), "{text:?}");
    }
    assert!(int_from_text(&"1".repeat(4300)).is_ok());
    assert_eq!(
        int_from_text(&format!(" {} ", "0".repeat(4301))),
        Err(
            "Exceeds the limit (4300 digits) for integer string conversion: value has 4301 \
             digits; use sys.set_int_max_str_digits() to increase the limit"
                .into()
        )
    );
}
