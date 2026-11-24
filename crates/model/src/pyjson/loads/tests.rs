//! `json.loads` against the toolchain's CPython: each text's value, or the
//! text of the error it raises, compared with Python's own.
use super::*;
use crate::pyjson::{Options, dumps};
use pytext::testing;

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Texts that load, fail, or meet a limit, each at a different point of the
/// scanner.
const CORPUS: &[&str] = &[
    // Values.
    "[NaN, Infinity, -Infinity, 1e400, -1e400]",
    "-0",
    "-0.0",
    "1e5",
    "1E+2",
    "1e-400",
    "12345678901234567890123",
    r#""\/\b\f\n\r\t\"\\""#,
    r#""ééé""#,
    "\"a\u{7f}b\"",
    r#""\ud83d\ude00""#,
    r#""\ud800\ude00x""#,
    " {\"a\" :\t[ 1 ,\n2 ] }\r",
    r#"{"a": 1, "a": 2, "b": 3}"#,
    "[true, false, null, {}, []]",
    "[\"\\u00e9\", \"\u{1f600}\"]",
    // Lone surrogates: a limit here, kept by Python.
    r#""\ud800""#,
    r#""\udc00""#,
    r#""\ud800A""#,
    r#""\ud800\udbff""#,
    r#"{"Marker": "m\udfff"}"#,
    // A failure after a lone surrogate is Python's error.
    r#""\ud800" x"#,
    r#""\ud800\u12""#,
    // Each error, at its position, across lines and wide characters.
    "",
    " ",
    "nul",
    "nulls",
    "-",
    "-Inf",
    "01",
    "1.",
    "1e",
    ".5",
    "1e+",
    "[1,]",
    "[1, ]",
    "{\"a\":1,}",
    "{\"a\" 1}",
    "{1: 2}",
    "{\"a\": }",
    "[1 2]",
    "[1,",
    "[",
    "{",
    "{\"a\"",
    "{\"a\":",
    "{\"a\":1",
    "{\"a\":1,",
    "\"\u{1}\"",
    "\"\t\"",
    r#""\x""#,
    r#""\u12""#,
    r#""\U00e9""#,
    r#""abc"#,
    "\"abc\\",
    "\u{feff}{}",
    "\u{a0}{}",
    "[1]\n\n  x",
    "{\"é\": [1,\n \"ü\" 2]}",
    "{\"a\": \"b\"} {}",
];

/// What CPython gives for each input on stdin (one JSON-encoded list of
/// texts): `json.dumps` of the value, `LIMIT` for a value holding a lone
/// surrogate, or the error's class and text.
const ORACLE: &str = r#"
import json, sys
def lone(value):
    if isinstance(value, str):
        return any(0xD800 <= ord(c) <= 0xDFFF for c in value)
    if isinstance(value, list):
        return any(lone(item) for item in value)
    if isinstance(value, dict):
        return any(lone(k) or lone(v) for k, v in value.items())
    return False
for text in json.loads(sys.stdin.read()):
    try:
        value = json.loads(text)
        print('LIMIT a string with a lone surrogate' if lone(value) else 'OK ' + json.dumps(value))
    except ValueError as error:
        print('ValueError ' + str(error))
"#;

fn ours(text: &str) -> String {
    match loads(text) {
        Ok(value) => format!(
            "OK {}",
            dumps(&value, &Options::default(), None).unwrap_or_default()
        ),
        Err(LoadError::Limit(limit)) => format!("LIMIT {limit}"),
        Err(LoadError::Invalid(message)) => format!("ValueError {message}"),
    }
}

#[test]
fn every_text_loads_or_fails_as_cpython_s_json_loads() -> Result {
    let input = dumps(
        &Value::List(
            CORPUS
                .iter()
                .map(|text| Value::String((*text).into()))
                .collect(),
        ),
        &Options::default(),
        None,
    )?;
    let expected = String::from_utf8(testing::python(ORACLE, input.as_bytes())?)?;
    let expected: Vec<&str> = expected.lines().collect();
    assert_eq!(expected.len(), CORPUS.len());
    let mismatches: Vec<String> = CORPUS
        .iter()
        .zip(expected)
        .filter_map(|(text, python)| {
            let native = ours(text);
            (native != python).then(|| format!("{text:?}\n  python: {python}\n  native: {native}"))
        })
        .collect();
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    Ok(())
}

/// `int()`'s limit: 4300 digits, the sign not counted, a `ValueError`
/// that is not a `JSONDecodeError`; a float has none.
#[test]
fn an_int_of_more_than_4300_digits_is_a_value_error() {
    let digits = |count| "1".repeat(count);
    assert!(matches!(loads(&digits(4300)), Ok(Value::BigInteger(_))));
    assert!(matches!(
        loads(&format!("-{}", digits(4300))),
        Ok(Value::BigInteger(_))
    ));
    assert!(matches!(
        loads(&digits(4301)),
        Err(LoadError::Invalid(message)) if message.starts_with("Exceeds the limit (4300 digits)")
    ));
    assert_eq!(
        loads(&format!("{}.0", digits(5000))),
        Ok(Value::Float(f64::INFINITY))
    );
}

/// Nesting up to [`MAX_DEPTH`] loads; one more is the limit, met without
/// recursing past it, however deep the text goes.
#[test]
fn nesting_past_the_limit_is_a_limit() {
    let nested = |depth| format!("{}{}", "[".repeat(depth), "]".repeat(depth));
    assert!(matches!(loads(&nested(MAX_DEPTH)), Ok(Value::List(_))));
    assert_eq!(
        loads(&nested(MAX_DEPTH + 1)),
        Err(LoadError::Limit(Limit::Depth))
    );
    assert_eq!(loads(&nested(100_000)), Err(LoadError::Limit(Limit::Depth)));
    let document = format!("{{\"Doc\": {}}}", nested(MAX_DEPTH - 1));
    assert!(loads(&document).is_ok());
    // The limit's name states the constant.
    assert!(Limit::Depth.as_str().contains(&format!(" {MAX_DEPTH} ")));
}
