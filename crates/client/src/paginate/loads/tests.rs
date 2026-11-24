//! `json.loads` of each text, as CPython's scanner reads it (checked with
//! Python 3.12's `json`, whose scanner 3.14's matches here).
use super::*;

fn ok(text: &str) -> Value {
    loads(text).unwrap_or(Value::String(format!("failed: {text}")))
}

fn string(text: &str) -> Value {
    Value::String(text.into())
}

/// `parse_constant`'s three, which `json.dumps` writes too.
#[test]
fn nan_and_the_infinities_are_floats() {
    let items = match ok("[NaN, Infinity, -Infinity]") {
        Value::List(items) => items,
        _ => Vec::new(),
    };
    assert_eq!(items.len(), 3);
    assert!(matches!(items[0], Value::Float(number) if number.is_nan()));
    assert_eq!(
        items[1..],
        [Value::Float(f64::INFINITY), Value::Float(f64::NEG_INFINITY)]
    );
    assert_eq!(loads("-Inf"), Err(LoadError::Invalid));
}

/// A pair joins; a lone surrogate, which no `String` holds, is the Python
/// route's, but only once the whole text loads.
#[test]
fn a_lone_surrogate_is_unsupported_once_the_text_loads() {
    let lone = Err(LoadError::Unsupported(LONE_SURROGATE));
    assert_eq!(loads(r#""\ud83d\ude00""#), Ok(string("\u{1F600}")));
    assert_eq!(loads(r#""\ud800\ude00x""#), Ok(string("\u{10200}x")));
    for text in [
        r#""\ud800""#,
        r#""\udc00""#,
        r#""\ud800A""#,
        r#""\ud800\udbff""#,
        r#"{"Marker": "m\udfff"}"#,
    ] {
        assert_eq!(loads(text), lone, "{text}");
    }
    assert_eq!(loads(r#""\ud800" x"#), Err(LoadError::Invalid));
    assert_eq!(loads(r#""\ud800\u12""#), Err(LoadError::Invalid));
}

/// `int()`'s limit: 4300 digits, the sign not counted; a float has none.
#[test]
fn an_int_of_more_than_4300_digits_is_a_value_error() {
    let digits = |count| "1".repeat(count);
    assert!(matches!(ok(&digits(4300)), Value::BigInteger(_)));
    assert!(matches!(
        ok(&format!("-{}", digits(4300))),
        Value::BigInteger(_)
    ));
    assert_eq!(loads(&digits(4301)), Err(LoadError::Invalid));
    assert_eq!(
        loads(&format!("-{}", digits(4301))),
        Err(LoadError::Invalid)
    );
    assert_eq!(
        ok(&format!("{}.0", digits(5000))),
        Value::Float(f64::INFINITY)
    );
}

/// `_match_number_unicode`: an `int` without fraction or exponent, a
/// fraction or exponent backed out of when no digit follows.
#[test]
fn numbers_read_as_the_scanner_reads_them() {
    assert_eq!(ok("-0"), Value::Integer(0));
    assert_eq!(ok("1e5"), Value::Float(100_000.0));
    assert_eq!(ok("1E+2"), Value::Float(100.0));
    assert_eq!(ok("1e-400"), Value::Float(0.0));
    assert!(matches!(ok("-0.0"), Value::Float(zero) if zero == 0.0 && zero.is_sign_negative()));
    for text in ["01", "-", "1.", "1e", ".5", "1e+"] {
        assert_eq!(loads(text), Err(LoadError::Invalid), "{text}");
    }
}

/// `scanstring_unicode`, strict: the escapes it knows, no raw control
/// character, DEL allowed.
#[test]
fn strings_read_as_the_scanner_reads_them() {
    assert_eq!(ok(r#""\/\b\f\n\r\t\"\\""#), string("/\u{8}\u{c}\n\r\t\"\\"));
    assert_eq!(ok(r#""ééé""#), string("ééé"));
    assert_eq!(ok("\"a\u{7f}b\""), string("a\u{7f}b"));
    for text in [
        "\"\u{1}\"",
        "\"\t\"",
        r#""\x""#,
        r#""\u12""#,
        r#""\U00e9""#,
        r#""abc"#,
    ] {
        assert_eq!(loads(text), Err(LoadError::Invalid), "{text}");
    }
}

/// Objects and arrays: `[ \t\n\r]` between tokens, no trailing comma, a
/// repeated key's later value in its first place, nothing after the
/// document.
#[test]
fn documents_read_as_the_decoder_reads_them() -> Result<(), model::value::JsonError> {
    assert_eq!(
        ok(" {\"a\" :\t[ 1 ,\n2 ] }\r"),
        Value::from_json(br#"{"a": [1, 2]}"#)?
    );
    assert_eq!(
        ok(r#"{"a": 1, "a": 2, "b": 3}"#),
        Value::from_json(br#"{"a": 2, "b": 3}"#)?
    );
    assert_eq!(
        ok("[true, false, null, {}, []]"),
        Value::from_json(b"[true, false, null, {}, []]")?
    );
    for text in [
        r#"{"a":1,}"#,
        "[1,]",
        r#"{"a" 1}"#,
        "{1: 2}",
        "[1 2]",
        "nul",
        "nulls",
        "\u{feff}{}",
        "\u{a0}{}",
        "",
    ] {
        assert_eq!(loads(text), Err(LoadError::Invalid), "{text}");
    }
    Ok(())
}

/// Nesting beyond what this reader follows is the Python route's, whose
/// own limit depends on its stack.
#[test]
fn nesting_beyond_200_is_unsupported() {
    let nested = |depth| format!("{}{}", "[".repeat(depth), "]".repeat(depth));
    assert!(matches!(ok(&nested(200)), Value::List(_)));
    assert_eq!(
        loads(&nested(201)),
        Err(LoadError::Unsupported("JSON nested too deep"))
    );
}
