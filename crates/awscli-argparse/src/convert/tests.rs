#![expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test fixtures and assertions"
)]
use super::*;
use std::io::Write;

const INT_CASES: &[&str] = &[
    "5",
    " 5 ",
    "\t-5\n",
    "+5",
    "007",
    "-0",
    "1_000",
    "1__0",
    "_1",
    "1_",
    "",
    " ",
    "5.0",
    "0x5",
    "1e3",
    "\x1c5",
    "99999999999999999999999",
    "-",
    "+",
    "5 5",
    "\u{b}5\u{c}",
];
const FLOAT_CASES: &[&str] = &[
    "1.5",
    " 1.5 ",
    "-1.5",
    "+.5",
    "5.",
    ".",
    "",
    "1e5",
    "1E-5",
    "1e+5",
    "1e",
    "e5",
    "1.5e5",
    "inf",
    "-Infinity",
    "NaN",
    "+nan",
    "infinit",
    "in_f",
    "1_0.5",
    "1._5",
    "1_.5",
    "1.5_0",
    "1e1_0",
    "1e_10",
    "0x1p3",
    "1e999",
    "-1e999",
    "1_000_000.000_1",
    "1.2.3",
    "\x1c1",
    "5 5",
];

#[test]
fn ascii_text_converts_as_int_and_float_read_it() {
    for (text, expected) in [
        ("5", Some("5")),
        (" 007 ", Some("7")),
        ("-0", Some("0")),
        ("1_000", Some("1000")),
        ("99999999999999999999999", Some("99999999999999999999999")),
        ("1__0", None),
        ("\x1c5", None),
        ("5.0", None),
    ] {
        let converted = convert(ValueType::Int, OsStr::new(text));
        let wanted = expected.map_or(Converted::Invalid, |value| Converted::Value(value.into()));
        assert_eq!(converted, wanted, "int({text:?})");
    }
    let over = "1".repeat(PYTHON_INT_MAX_STR_DIGITS + 1);
    assert_eq!(
        convert(ValueType::Int, OsStr::new(&over)),
        Converted::Invalid
    );
    for (text, expected) in [
        ("1.5", "1.5"),
        ("-inf", "-inf"),
        ("1e999", "inf"),
        (".5", "0.5"),
    ] {
        let converted = convert(ValueType::Float, OsStr::new(text));
        assert_eq!(
            converted,
            Converted::Value(expected.into()),
            "float({text:?})"
        );
    }
    assert_eq!(
        convert(ValueType::Float, OsStr::new("1._5")),
        Converted::Invalid
    );
}

#[test]
fn non_ascii_text_is_python_s_to_judge() {
    for text in ["\u{661}", "5\u{a0}", "\u{ff15}.5"] {
        for value_type in [ValueType::Int, ValueType::Float] {
            assert_eq!(convert(value_type, OsStr::new(text)), Converted::Undecided);
        }
    }
}

/// Python's own `int()` and `float()` on every case: the toolchain's
/// CPython prints each result's value (or `invalid`), and the native
/// conversion must give the same value.
#[test]
fn conversions_match_the_toolchain_s_python() {
    let Some(prefix) = std::env::var_os("AWS_NATIVE_CPYTHON_PREFIX") else {
        panic!("AWS_NATIVE_CPYTHON_PREFIX must name the toolchain CPython");
    };
    let script = "import json, sys\n\
        for line in sys.stdin:\n\
        \x20   kind, text = json.loads(line)\n\
        \x20   try:\n\
        \x20       value = int(text) if kind == 'int' else float(text)\n\
        \x20       print(json.dumps(str(value) if kind == 'int' else repr(value)))\n\
        \x20   except ValueError:\n\
        \x20       print(json.dumps(None))\n";
    let cases: Vec<(ValueType, &str)> = INT_CASES
        .iter()
        .map(|text| (ValueType::Int, *text))
        .chain(FLOAT_CASES.iter().map(|text| (ValueType::Float, *text)))
        .collect();
    let input: String = cases
        .iter()
        .map(|(kind, text)| format!("[\"{}\", {}]\n", kind.name(), json_string(text)))
        .collect();
    let mut child = std::process::Command::new(std::path::Path::new(&prefix).join("bin/python3"))
        .args(["-I", "-c", script])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let lines: Vec<&str> = std::str::from_utf8(&output.stdout)
        .unwrap()
        .lines()
        .collect();
    assert_eq!(lines.len(), cases.len());
    for ((kind, text), line) in cases.iter().zip(lines) {
        let python = line.trim_matches('"');
        let native = convert(*kind, OsStr::new(text));
        match (python, native) {
            ("null", Converted::Invalid) => {}
            (expected, Converted::Value(actual)) if *kind == ValueType::Int => {
                assert_eq!(actual, expected, "int({text:?})");
            }
            (expected, Converted::Value(actual)) => {
                let (expected, actual): (f64, f64) =
                    (expected.parse().unwrap(), actual.parse().unwrap());
                let same = expected == actual || (expected.is_nan() && actual.is_nan());
                assert!(same, "float({text:?}): {expected} != {actual}");
            }
            (expected, actual) => {
                panic!("{kind:?}({text:?}): Python {expected}, native {actual:?}")
            }
        }
    }
}

fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
