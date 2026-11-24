//! The API's own cases. `repr::tests` checks `str.isprintable` against
//! the toolchain's CPython for every scalar value, and `base64::tests`
//! checks `b64encode` and `b64decode`; the readers built over `number`
//! keep their CPython cases where they strip (`model::pyrepr`,
//! `awscli_argparse::convert`).
use super::number::{self, C_SPACE};
use super::repr;

#[test]
fn floats_read_as_float_reads_them() {
    assert_eq!(number::float("-1_0.5e-1"), Some(-1.05));
    assert_eq!(number::float(".5"), Some(0.5));
    assert!(number::float("+nAn").is_some_and(f64::is_nan));
    assert_eq!(number::float("-Infinity"), Some(f64::NEG_INFINITY));
    for invalid in ["", ".", "1_", "_1", "1__0", "1e", "e1", "0x10", " 1"] {
        assert_eq!(number::float(invalid), None, "{invalid:?}");
    }
}

#[test]
fn ints_read_as_int_reads_them() {
    assert_eq!(number::int("-00_12").as_deref(), Some("-12"));
    assert_eq!(number::int("-0").as_deref(), Some("0"));
    assert_eq!(number::int("1.0"), None);
    assert_eq!(number::digit_count("-00_12"), 4);
    assert_eq!(" \t1\r".trim_matches(C_SPACE), "1");
    assert_eq!("\u{1c}1".trim_matches(C_SPACE), "\u{1c}1");
}

#[test]
fn reprs_quote_and_escape_as_python_does() {
    assert_eq!(repr::str_repr("it's"), "\"it's\"");
    assert_eq!(
        repr::str_repr("a\u{a0}\u{e9}\t\u{1f600}\u{10ffff}"),
        "'a\\xa0\u{e9}\\t\u{1f600}\\U0010ffff'"
    );
    assert_eq!(repr::bytes_repr(b"'\x00\xff"), "b\"'\\x00\\xff\"");
    assert_eq!(repr::float_repr(1e16), "1e+16");
    assert_eq!(repr::float_repr(0.0001), "0.0001");
    assert_eq!(repr::float_repr(-0.0), "-0.0");
}

/// What `float()` and `int()` read first: other Unicode whitespace as a
/// space and other decimal digits as their ASCII digit, ASCII as it is, and
/// anything else ends the literal invalid.
#[test]
fn literals_are_mapped_to_ascii_as_cpython_maps_them() {
    assert_eq!(number::ascii_literal("12"), "12");
    assert_eq!(number::ascii_literal("\u{661}\u{662}"), "12");
    assert_eq!(number::ascii_literal("\u{2000}5\u{85}"), " 5 ");
    assert_eq!(number::ascii_literal("\u{1c}5"), "\u{1c}5");
    assert_eq!(number::ascii_literal("5\u{180e}9"), "5?");
    assert_eq!(number::ascii_literal("\u{7f}"), "?");
}
