//! `TokenEncoder` and `TokenDecoder` (tests/unit/botocore/test_paginate.py
//! `TestTokenDecoder`, tests/functional/botocore/test_paginate.py
//! `test_token_encoding`), each encoding the vendored botocore's.
use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn json(text: &str) -> Result<Map, Box<dyn std::error::Error>> {
    match Value::from_json(text.as_bytes())? {
        Value::Map(map) => Ok(map),
        other => Err(format!("not an object: {other:?}").into()),
    }
}

fn blob(bytes: &[u8]) -> Value {
    Value::Blob(bytes.to_vec())
}

fn map(pairs: Vec<(&str, Value)>) -> Map {
    pairs
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

/// `{'foo': 'bar'}`, then each of `test_token_encoding`'s tokens with bytes
/// in them, at the top, nested, listed, and several at once.
fn tokens() -> Vec<(Map, &'static str)> {
    let foo = |value| map(vec![("foo", value)]);
    vec![
        (foo(Value::String("bar".into())), "eyJmb28iOiAiYmFyIn0="),
        (
            foo(blob(b"bar")),
            "eyJmb28iOiAiWW1GeSIsICJib3RvX2VuY29kZWRfa2V5cyI6IFtbImZvbyJdXX0=",
        ),
        (
            foo(Value::Map(map(vec![("bar", blob(b"baz"))]))),
            "eyJmb28iOiB7ImJhciI6ICJZbUY2In0sICJib3RvX2VuY29kZWRfa2V5cyI6IFtbImZvbyIsICJiYXIiXV19",
        ),
        (
            foo(Value::List(vec![Value::String("bar".into()), blob(b"baz")])),
            "eyJmb28iOiBbImJhciIsICJZbUY2Il0sICJib3RvX2VuY29kZWRfa2V5cyI6IFtbImZvbyIsIDFdXX0=",
        ),
        (
            foo(blob(b"\xff")),
            "eyJmb28iOiAiL3c9PSIsICJib3RvX2VuY29kZWRfa2V5cyI6IFtbImZvbyJdXX0=",
        ),
        (
            foo(Value::Map(map(vec![
                ("bar", blob(b"baz")),
                ("bin", Value::List(vec![blob(b"bam")])),
            ]))),
            "eyJmb28iOiB7ImJhciI6ICJZbUY2IiwgImJpbiI6IFsiWW1GdCJdfSwgImJvdG9fZW5jb2RlZF9rZXlzIjogW1\
                si\
             Zm9vIiwgImJhciJdLCBbImZvbyIsICJiaW4iLCAwXV19",
        ),
    ]
}

#[test]
fn a_token_encodes_as_botocore_s_and_decodes_back() -> Result<(), String> {
    for (token, expected) in tokens() {
        assert_eq!(encode(&token)?, expected, "{token:?}");
        assert_eq!(decode(expected), Ok(token));
    }
    Ok(())
}

/// `json.dumps`' defaults: `', '` and `': '`, non-ASCII escaped, Python's
/// float and big `int` text.
#[test]
fn a_token_is_json_dumps_with_its_defaults() -> TestResult {
    let token = json(
        r#"{"Marker": "m1", "n": 1.5, "big": 1180591620717411303424, "u": "é", "l": [null, true]}"#,
    )?;
    assert_eq!(
        encode(&token)?,
        "eyJNYXJrZXIiOiAibTEiLCAibiI6IDEuNSwgImJpZyI6IDExODA1OTE2MjA3MTc0MTEzMDM0MjQsICJ1IjogIlx1MD\
            BlOSIs\
         ICJsIjogW251bGwsIHRydWVdfQ=="
    );
    Ok(())
}

/// `TestTokenDecoder`'s tokens, bytes listed and several at once.
#[test]
fn the_decoder_reads_botocore_s_tokens() {
    let listed = "eyJib3RvX2VuY29kZWRfa2V5cyI6IFtbImZvbyIsICJiYXIiLCAxXV0sICJmb28iOiB7ImJhciI6IFsiY\
        mF6IiwgIllt\
                  bHUiXX19";
    let foo = |value| Ok(map(vec![("foo", Value::Map(value))]));
    assert_eq!(
        decode(listed),
        foo(map(vec![(
            "bar",
            Value::List(vec![Value::String("baz".into()), blob(b"bin")])
        )]))
    );
    let several = "eyJib3RvX2VuY29kZWRfa2V5cyI6IFtbImZvbyIsICJiaW4iXSwgWyJmb28iLCAiYmFyIl1dLCAiZm9v\
        IjogeyJiaW4i\
                   OiAiWW1GdCIsICJiYXIiOiAiWW1GNiJ9fQ==";
    assert_eq!(
        decode(several),
        foo(map(vec![("bin", blob(b"bam")), ("bar", blob(b"baz"))]))
    );
}

fn b64(json: &str) -> String {
    model::base64::encode(json.as_bytes())
}

/// What the decoder raises is a `ValueError` or `TypeError`, which sends
/// the token to the old form's parser, or any other exception, which
/// `_parse_starting_token` lets through.
#[test]
fn a_token_that_is_not_one_fails_as_python_s_decoder_does() {
    let raised = |text: &str| Err(DecodeError::Raised(text.to_owned()));
    let cases = [
        ("not base64!", Err(DecodeError::Deprecated)),
        (&*b64("{"), Err(DecodeError::Deprecated)),
        (
            &*model::base64::encode(b"\xff"),
            Err(DecodeError::Deprecated),
        ),
        (&*b64(r#"["x"]"#), Err(DecodeError::Deprecated)),
        (
            &*b64(r#""abc""#),
            raised("'str' object has no attribute 'pop'"),
        ),
        (&*b64("5"), raised("'int' object has no attribute 'pop'")),
        (
            &*b64("null"),
            raised("'NoneType' object has no attribute 'pop'"),
        ),
        (
            &*b64(r#"{"a": "b", "boto_encoded_keys": [["Nope"]]}"#),
            raised("'Nope'"),
        ),
        (
            &*b64(r#"{"a": "b", "boto_encoded_keys": [["a", 5]]}"#),
            raised("string index out of range"),
        ),
        (
            &*b64(r#"{"a": ["b"], "boto_encoded_keys": [["a", 5]]}"#),
            raised("list index out of range"),
        ),
        (
            &*b64(r#"{"a": 5, "boto_encoded_keys": [["a"]]}"#),
            raised("'int' object has no attribute 'encode'"),
        ),
        (
            &*b64(r#"{"a": "b", "boto_encoded_keys": [[]]}"#),
            raised("'dict' object has no attribute 'encode'"),
        ),
        (
            &*b64(r#"{"a": "b", "boto_encoded_keys": 5}"#),
            Err(DecodeError::Deprecated),
        ),
        (
            &*b64(r#"{"a": "xy", "boto_encoded_keys": [["a", 0]]}"#),
            Err(DecodeError::Deprecated),
        ),
        // `'='[0]` decodes to `b''`, which a str cannot take.
        (
            &*b64(r#"{"a": "=", "boto_encoded_keys": [["a", 0]]}"#),
            Err(DecodeError::Deprecated),
        ),
        // A dict of paths is its keys, each a str of steps.
        (
            &*b64(r#"{"a": {"b": "Yg=="}, "boto_encoded_keys": {"a": 1}}"#),
            raised("'dict' object has no attribute 'encode'"),
        ),
        // `path[:-1]` of a dict path: slices hash on 3.12 and later.
        (
            &*b64(r#"{"a": {"b": "Yg=="}, "boto_encoded_keys": [{"a": 1, "b": 2}]}"#),
            raised("slice(None, -1, None)"),
        ),
    ];
    for (token, expected) in cases {
        assert_eq!(decode(token), expected, "{token}");
    }
}

/// base64 skips what is not of its alphabet, and a str of steps is its
/// characters.
#[test]
fn the_decoder_takes_what_python_s_takes() {
    let bytes = |value: &[u8]| Ok(map(vec![("a", blob(value))]));
    assert_eq!(
        decode(&b64(r#"{"a": "!", "boto_encoded_keys": [["a"]]}"#)),
        bytes(b"")
    );
    assert_eq!(
        decode(&b64(r#"{"a": "Yg==", "boto_encoded_keys": "a"}"#)),
        bytes(b"b")
    );
}

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

/// `_parse_starting_token_deprecated` (`TestDeprecatedStartingToken`):
/// `___` between the tokens, the offset after them, `None` for a token not
/// sent, those missing `None` too.
#[test]
fn an_old_token_is_split_on_triple_underscores() -> TestResult {
    let parse = |raw, input: &[&str]| parse_starting_token(raw, &names(input));
    let string = |text: &str| Value::String(text.into());
    let one = parse("m1___0", &["Marker"])?;
    assert_eq!(one.token, map(vec![("Marker", string("m1"))]));
    assert_eq!(one.index, Value::Integer(0));
    let two = parse("m0___None___ 1 ", &["A", "B"])?;
    assert_eq!(
        two.token,
        map(vec![("A", string("m0")), ("B", Value::Null)])
    );
    assert_eq!(two.index, Value::Integer(1));
    let short = parse("m0", &["A", "B"])?;
    assert_eq!(
        short.token,
        map(vec![("A", string("m0")), ("B", Value::Null)])
    );
    // `int('bar')` fails: the whole token is the service's.
    let opaque = parse("foo___bar", &["nextToken"])?;
    assert_eq!(opaque.token, map(vec![("nextToken", string("foo___bar"))]));
    assert_eq!(
        parse("m1___m4___0", &["Marker"]),
        Err(CallError::Pagination(PaginationError::Value(
            "Bad starting token: m1___m4___0".into()
        )))
    );
    assert_eq!(
        parse("does___not___work", &["Marker"]),
        Err(CallError::Pagination(PaginationError::Value(
            "Bad starting token: does___not___work".into()
        )))
    );
    Ok(())
}

/// A token of the new form gives up `boto_truncate_amount`, whatever it is.
#[test]
fn a_new_token_s_offset_is_taken_out() -> TestResult {
    let parsed = parse_starting_token(
        &b64(r#"{"Marker": null, "boto_truncate_amount": "x"}"#),
        &names(&["Marker"]),
    )?;
    assert_eq!(parsed.token, map(vec![("Marker", Value::Null)]));
    assert_eq!(parsed.index, Value::String("x".into()));
    Ok(())
}

/// The `resume_token` setter refuses keys beyond the input tokens.
#[test]
fn a_resume_token_has_only_the_input_tokens() -> TestResult {
    let input = names(&["Marker"]);
    let truncated = json(r#"{"Marker": "m1", "boto_truncate_amount": 1}"#)?;
    assert_eq!(
        resume_token(&truncated, &input)?,
        "eyJNYXJrZXIiOiAibTEiLCAiYm90b190cnVuY2F0ZV9hbW91bnQiOiAxfQ=="
    );
    let extra = json(r#"{"Marker": "m1", "Filter": "x", "boto_truncate_amount": 2}"#)?;
    assert_eq!(
        resume_token(&extra, &input),
        Err(PaginationError::Value(
            "Bad starting token: {'Marker': 'm1', 'Filter': 'x', 'boto_truncate_amount': 2}".into()
        ))
    );
    Ok(())
}

/// `int()` of the old form's offset reads any Unicode decimal digit, as
/// `_PyUnicode_TransformDecimalAndSpaceToASCII` gives it (Python 3.12 for
/// those it knows, and CPython 3.14.8's table for Unicode 16.0's).
#[test]
fn an_old_token_s_offset_is_int_of_any_decimal_digits() -> TestResult {
    let int = |text: &str| py::int_of_str(text).ok();
    let cases = [
        ("\u{661}", Some(1)),
        ("\u{661}\u{662}", Some(12)),
        ("\u{3000}\u{661}\u{3000}", Some(1)),
        ("1\u{661}", Some(11)),
        ("\u{1D7CE}", Some(0)),
        ("\u{FF11}_\u{FF12}", Some(12)),
        ("-\u{663}", Some(-3)),
        ("\u{661}\u{85}", Some(1)),
        ("\u{1E5F1}", Some(0)),
        ("\u{1E5FA}", Some(9)),
        ("\u{1E5FB}", None),
        ("x", None),
        ("\u{661}x", None),
        ("\u{B2}", None),
        ("\u{1C}1", None),
    ];
    for (text, expected) in cases {
        assert_eq!(int(text), expected.map(Value::Integer), "{text:?}");
    }
    let parsed = parse_starting_token("m1___\u{661}", &names(&["Marker"]))?;
    assert_eq!(
        parsed.token,
        map(vec![("Marker", Value::String("m1".into()))])
    );
    assert_eq!(parsed.index, Value::Integer(1));
    Ok(())
}

/// The reviewer's tokens (kms `ListAliases` on the Python route): `NaN`
/// and `Infinity` offsets are floats; an `int` past 4300 digits fails
/// `json.loads`, so the token is of the old form; a lone surrogate is the
/// Python route's.
#[test]
fn a_new_token_is_json_loads_python_s() -> TestResult {
    let input = names(&["Marker"]);
    for literal in ["NaN", "Infinity"] {
        let token = b64(&format!(
            r#"{{"Marker": "m1", "boto_truncate_amount": {literal}}}"#
        ));
        let parsed = parse_starting_token(&token, &input)?;
        assert_eq!(
            parsed.token,
            map(vec![("Marker", Value::String("m1".into()))])
        );
        assert!(matches!(parsed.index, Value::Float(_)), "{literal}");
    }
    let long = "1".repeat(5000);
    for json in [
        format!(r#"{{"Marker": "m1", "boto_truncate_amount": {long}}}"#),
        format!(r#"{{"Marker": {long}}}"#),
    ] {
        let token = b64(&json);
        let parsed = parse_starting_token(&token, &input)?;
        assert_eq!(parsed.token, map(vec![("Marker", Value::String(token))]));
    }
    assert_eq!(
        parse_starting_token(&b64(r#"{"Marker": "\ud800"}"#), &input),
        Err(CallError::Unsupported(Unsupported::StartingToken(
            "a lone surrogate"
        )))
    );
    Ok(())
}
