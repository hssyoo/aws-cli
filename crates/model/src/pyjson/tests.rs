use super::*;
use crate::testing::{self, Result};
use crate::value::{Map, Value};

fn dump(text: &str, options: &Options) -> Result<String> {
    Ok(dumps(&Value::from_json(text.as_bytes())?, options, None)?)
}

#[test]
fn round_trips_match_python_for_the_cases_that_matter() -> Result<()> {
    let options = Options::default();
    assert_eq!(dump("1.50", &options)?, "1.5");
    assert_eq!(
        dump("12345678901234567890", &options)?,
        "12345678901234567890"
    );
    assert_eq!(
        dump(r#""é😀\u0001\u007f\"\\/""#, &options)?,
        r#""\u00e9\ud83d\ude00\u0001\u007f\"\\/""#
    );
    let raw = Options {
        ensure_ascii: false,
        ..Options::default()
    };
    assert_eq!(
        dump(r#""é😀\u0001\u007f\b\f\n\r\t""#, &raw)?,
        "\"é😀\\u0001\u{7f}\\b\\f\\n\\r\\t\""
    );
    assert_eq!(
        dump(r#"{"a":[1,{"b":null}],"c":{}}"#, &options)?,
        r#"{"a": [1, {"b": null}], "c": {}}"#
    );
    assert_eq!(
        dump(r#"{"a":[1,{"b":null}],"c":{}}"#, &Options::compact())?,
        r#"{"a":[1,{"b":null}],"c":{}}"#
    );
    let indented = "{\n    \"a\": [\n        1,\n        []\n    ],\n    \"b\": {}\n}";
    assert_eq!(
        dump(r#"{"a":[1,[]],"b":{}}"#, &Options::indented(4))?,
        indented
    );
    assert_eq!(dump("[1]", &Options::indented(0))?, "[\n1\n]");
    Ok(())
}

#[test]
fn floats_print_as_python_repr() -> Result<()> {
    let options = Options::default();
    for (value, text) in [
        (1e16, "1e+16"),
        (1e15, "1000000000000000.0"),
        (0.0001, "0.0001"),
        (0.00001, "1e-05"),
        (-0.0, "-0.0"),
        (5e-324, "5e-324"),
        (f64::MAX, "1.7976931348623157e+308"),
        (123_456_789.123_456_79, "123456789.12345679"),
        (f64::INFINITY, "Infinity"),
        (f64::NEG_INFINITY, "-Infinity"),
        (f64::NAN, "NaN"),
        (2.5, "2.5"),
    ] {
        assert_eq!(dumps(&Value::Float(value), &options, None)?, text);
    }
    Ok(())
}

#[test]
fn unencodable_values_fail_as_json_dumps_does_unless_default_converts_them() -> Result<()> {
    let options = Options::default();
    let error = dumps(&Value::Blob(vec![1]), &options, None)
        .err()
        .ok_or("blob dumped")?;
    assert_eq!(
        error.to_string(),
        "Object of type bytes is not JSON serializable"
    );
    let instant = timestamp::Timestamp::MIN;
    let error = dumps(&Value::Timestamp(instant), &options, None)
        .err()
        .ok_or("datetime dumped")?;
    assert_eq!(
        error.to_string(),
        "Object of type datetime is not JSON serializable"
    );
    let mut map = Map::new();
    map.insert("t".into(), Value::Timestamp(instant));
    let convert = |value: &Value| match value {
        Value::Timestamp(at) => Ok(Value::String(at.iso8601())),
        _ => Err(DumpError::unserializable(value)),
    };
    let text = dumps(&Value::Map(map), &options, Some(&convert))?;
    assert_eq!(text, r#"{"t": "0001-01-01T00:00:00Z"}"#);
    Ok(())
}

/// A corpus `json.loads` reads and `json.dumps` writes under each option set;
/// CPython produces the expected bytes at test time.
#[test]
fn json_dumps_agrees_with_python_byte_for_byte() -> Result<()> {
    let mut corpus = String::from(
        r#"[{"z":1,"a":[1.50,-0.0,1e22,1e-7,0.1,3.141592653589793,1e400,-1e400],
        "big":123456789012345678901234567890,"neg":-12345678901234567890,
        "s":"plain é ü 中文 😀 \u0000\u001f\u007f\u0080\u2028\ufeff \"q\" \\ / \t",
        "nested":{"empty":{},"list":[],"deep":[[[]],[{}]]},"t":true,"f":false,"n":null}"#,
    );
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    for _ in 0..400 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let value = f64::from_bits(state);
        if value.is_finite() {
            corpus.push_str(&format!(",{value:e}"));
        }
    }
    corpus.push(']');
    let script = r#"
import json, sys
value = json.loads(sys.stdin.read())
out = []
for indent in (None, 0, 2, 4):
    for separators in (None, (',', ':'), (', ', ': ')):
        for ensure_ascii in (True, False):
            out.append(json.dumps(value, indent=indent, separators=separators,
                                  ensure_ascii=ensure_ascii))
sys.stdout.write(json.dumps(out))
"#;
    let expected = testing::python(script, corpus.as_bytes())?;
    let expected: Vec<String> = serde_json::from_slice(&expected)?;
    let value = Value::from_json(corpus.as_bytes())?;
    let mut actual = Vec::new();
    for indent in [None, Some(0), Some(2), Some(4)] {
        for separators in [None, Some((",", ":")), Some((", ", ": "))] {
            for ensure_ascii in [true, false] {
                let options = Options {
                    indent: indent.map(|width| " ".repeat(width)),
                    separators: separators.map(|(item, key)| (item.to_owned(), key.to_owned())),
                    ensure_ascii,
                };
                actual.push(dumps(&value, &options, None)?);
            }
        }
    }
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(actual, expected, "option set {index}");
    }
    Ok(())
}

/// Each chunk as hex of its bytes, a lone surrogate as UTF-8's three-byte
/// form (`surrogatepass`).
fn chunks(value: &Value, options: &Options, fallback: Option<Fallback<'_>>) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut sink = |chunk: &[u8]| -> std::result::Result<(), ()> {
        out.push(chunk.iter().map(|byte| format!("{byte:02x}")).collect());
        Ok(())
    };
    dump_chunks(value, options, fallback, &mut sink).map_err(|error| format!("{error:?}"))?;
    Ok(out)
}

const CHUNK_ORACLE: &str = r#"
import json, sys
def default(o):
    if isinstance(o, bytes):
        return o.hex()
    raise TypeError(f'Object of type {type(o).__name__} is not JSON serializable')
def blobs(o):
    if isinstance(o, dict):
        if list(o) == ['$blob']:
            return bytes.fromhex(o['$blob'])
        return {k: blobs(v) for k, v in o.items()}
    if isinstance(o, list):
        return [blobs(v) for v in o]
    return o
value = blobs(json.loads(sys.stdin.read()))
out = []
for indent in (None, 0, 4):
    for separators in (None, (',', ':'), ('', ': ')):
        for ensure_ascii in (True, False):
            encoder = json.JSONEncoder(indent=indent, separators=separators,
                                       ensure_ascii=ensure_ascii, default=default)
            out.append([c.encode('utf-8', 'surrogatepass').hex()
                        for c in encoder.iterencode(value)])
sys.stdout.write(json.dumps(out))
"#;

/// `{"$blob": hex}` as the bytes, which the fallback writes as hex again.
fn with_blobs(value: Value) -> Value {
    match value {
        Value::Map(map) if map.len() == 1 && map.contains_key("$blob") => {
            let hex = map.get("$blob").and_then(Value::as_str).unwrap_or("");
            Value::Blob(
                (0..hex.len())
                    .step_by(2)
                    .filter_map(|at| u8::from_str_radix(&hex[at..at + 2], 16).ok())
                    .collect(),
            )
        }
        Value::Map(map) => Value::Map(map.into_iter().map(|(k, v)| (k, with_blobs(v))).collect()),
        Value::List(items) => Value::List(items.into_iter().map(with_blobs).collect()),
        other => other,
    }
}

fn hex_fallback(value: &Value) -> std::result::Result<Value, DumpError> {
    match value {
        Value::Blob(bytes) => Ok(Value::String(
            bytes.iter().map(|byte| format!("{byte:02x}")).collect(),
        )),
        other => Err(DumpError::unserializable(other)),
    }
}

/// `json.dump` writes each chunk `iterencode` yields; ours are the same
/// chunks, under each option set, a lone surrogate and a converted value
/// included.
#[test]
fn dump_chunks_are_iterencode_s() -> Result<()> {
    let corpus = r#"{"a": 1, "s": "x\ud800y\udc80", "\udcff": [1, "t", [], {}, [2.5, null],
        {"k": true}, {"$blob": "0102"}, false], "e": {}, "l": [], "n": {"m": [{"$blob": ""}]},
        "b": {"$blob": "ff"}, "z": "é😀"}"#;
    let expected = testing::python(CHUNK_ORACLE, corpus.as_bytes())?;
    let expected: Vec<Vec<String>> = serde_json::from_slice(&expected)?;
    let value = with_blobs(loads(corpus).map_err(|error| format!("{error:?}"))?);
    let mut actual = Vec::new();
    for indent in [None, Some(0), Some(4)] {
        for separators in [None, Some((",", ":")), Some(("", ": "))] {
            for ensure_ascii in [true, false] {
                let options = Options {
                    indent: indent.map(|width| " ".repeat(width)),
                    separators: separators.map(|(item, key)| (item.to_owned(), key.to_owned())),
                    ensure_ascii,
                };
                actual.push(chunks(&value, &options, Some(&hex_fallback))?);
            }
        }
    }
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(actual, expected, "option set {index}");
    }
    Ok(())
}

/// A sink's error stops the dump, the chunks before it written; a value
/// nothing converts fails after the chunks before it.
#[test]
fn dump_chunks_stops_at_the_first_error() {
    let mut map = Map::new();
    map.insert("Text".into(), Value::String("x".into()));
    map.insert("At".into(), Value::Blob(vec![1]));
    let value = Value::Map(map);
    let options = Options::indented(4);
    let mut written = Vec::new();
    let mut sink = |chunk: &[u8]| -> std::result::Result<(), &'static str> {
        if written.len() == 3 {
            return Err("full");
        }
        written.push(String::from_utf8_lossy(chunk).into_owned());
        Ok(())
    };
    assert_eq!(
        dump_chunks(&value, &options, None, &mut sink),
        Err(ChunkError::Write("full"))
    );
    assert_eq!(written, ["{", "\n    ", "\"Text\""]);
    let mut written = Vec::new();
    let mut sink = |chunk: &[u8]| -> std::result::Result<(), ()> {
        written.push(String::from_utf8_lossy(chunk).into_owned());
        Ok(())
    };
    assert_eq!(
        dump_chunks(&value, &options, None, &mut sink),
        Err(ChunkError::Dump(DumpError::unserializable(&Value::Blob(
            vec![1]
        ))))
    );
    assert_eq!(
        written,
        [
            "{", "\n    ", "\"Text\"", ": ", "\"x\"", ",\n    ", "\"At\"", ": "
        ]
    );
}
