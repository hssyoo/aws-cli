use super::*;
use crate::testing::{self, Result};
use crate::value::{Map, Value};
use timestamp::Timestamp;

#[test]
fn printable_matches_python_str_isprintable_for_every_scalar_value() -> Result<()> {
    let script = r#"
import sys
sys.stdout.write(''.join('1' if chr(c).isprintable() else '0' for c in range(0x110000)))
"#;
    let expected = testing::python(script, b"")?;
    let mut mismatches = Vec::new();
    for (code, flag) in expected.iter().enumerate() {
        let Some(character) = u32::try_from(code).ok().and_then(char::from_u32) else {
            continue;
        };
        if is_printable(character) != (*flag == b'1') {
            mismatches.push(code);
        }
    }
    assert!(mismatches.is_empty(), "{} differ, first {:x?}", mismatches.len(), &mismatches[..mismatches.len().min(8)]);
    Ok(())
}

/// `repr()` and `str()` of what `json.loads` makes of a corpus, from CPython.
#[test]
fn repr_and_str_agree_with_python_for_json_values() -> Result<()> {
    let corpus = r#"["plain", "it's", "say \"hi\"", "both ' \"", "\u0000\u007f\u0080 ­ 😀󠀁￿\\",
        "tab\tnl\ncr\r", "é中文", 1, -12345678901234567890, 2.5, 1e16, -0.0, 1e-05, 1e400, true, false, null,
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
fn bytes_and_datetimes_print_as_python_does() -> Result<()> {
    let blob = Value::Blob(b"a'b\"c\\\x00\x7f\xff\t\n\r".to_vec());
    assert_eq!(py_repr(&blob), r#"b'a\'b"c\\\x00\x7f\xff\t\n\r'"#);
    assert_eq!(py_str(&blob), py_repr(&blob));
    assert_eq!(py_repr(&Value::Blob(b"it's".to_vec())), r#"b"it's""#);
    assert_eq!(py_repr(&Value::List(vec![Value::Blob(b"x".to_vec())])), "[b'x']");
    let at = |seconds, micros| Timestamp::from_unix(seconds, micros).ok_or("instant");
    let micro = Value::Timestamp(at(1_577_934_245, 6)?);
    assert_eq!(py_str(&micro), "2020-01-02 03:04:05.000006+00:00");
    assert_eq!(
        py_repr(&micro),
        "datetime.datetime(2020, 1, 2, 3, 4, 5, 6, tzinfo=datetime.timezone.utc)"
    );
    let minute = Value::Timestamp(at(1_577_934_240, 0)?);
    assert_eq!(py_str(&minute), "2020-01-02 03:04:00+00:00");
    assert_eq!(py_repr(&minute), "datetime.datetime(2020, 1, 2, 3, 4, tzinfo=datetime.timezone.utc)");
    let second = Value::Timestamp(at(1_577_934_245, 0)?);
    assert_eq!(
        py_repr(&second),
        "datetime.datetime(2020, 1, 2, 3, 4, 5, tzinfo=datetime.timezone.utc)"
    );
    assert_eq!(py_str(&Value::Timestamp(Timestamp::MIN)), "0001-01-01 00:00:00+00:00");
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
        (Value::BigInteger("99999999999999999999".into()), "<class 'int'>"),
        (Value::Float(1.0), "<class 'float'>"),
        (Value::String(String::new()), "<class 'str'>"),
        (Value::Blob(Vec::new()), "<class 'bytes'>"),
        (Value::Timestamp(Timestamp::MIN), "<class 'datetime.datetime'>"),
        (Value::List(Vec::new()), "<class 'list'>"),
        (Value::Map(map), "<class 'dict'>"),
    ] {
        assert_eq!(py_type(&value), name);
    }
}
