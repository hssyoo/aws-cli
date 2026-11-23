use super::*;

/// `str.isprintable` of a code point, a lone surrogate never printable.
fn is_printable(code: u32) -> bool {
    char::from_u32(code).is_some_and(pytext::repr::is_printable)
}

#[test]
fn python_repr_preserves_printable_unicode_and_escapes_surrogates() {
    for (source, expected) in [
        (b"it's".as_slice(), b"\"it's\"".as_slice()),
        (b"a\"b'", b"'a\"b\\''"),
        (b"\\", b"'\\\\'"),
        (b"back\\slash\ttab\nline", b"'back\\\\slash\\ttab\\nline'"),
        (b"a\rb", b"'a\\rb'"),
        (b"\x01\x7f", b"'\\x01\\x7f'"),
        ("café😀".as_bytes(), "'café😀'".as_bytes()),
        ("\u{85}\u{a0}".as_bytes(), b"'\\x85\\xa0'"),
        ("x\u{200b}y".as_bytes(), b"'x\\u200by'"),
        ("\u{e0001}".as_bytes(), b"'\\U000e0001'"),
        (b"bad\xff", b"'bad\\udcff'"),
        (b"\xed\xa0\x80", b"'\\udced\\udca0\\udc80'"),
        (b"\xe2\x82", b"'\\udce2\\udc82'"),
    ] {
        assert_eq!(repr(source), expected, "{source:?}");
    }
}

#[test]
fn printable_code_points_match_unicode_boundaries() {
    for code in [
        0x80, 0x85, 0xa0, 0xad, 0x378, 0x200b, 0x2028, 0x3000, 0xdc80, 0xdcff, 0xe000, 0xe0001,
        0x10ffff,
    ] {
        assert!(!is_printable(code));
    }
    for code in [0xa1, 0xe9, 0x20ac, 0xff1a, 0x1f600, 0x212a] {
        assert!(is_printable(code));
    }
}

#[test]
fn lower_preserves_target_non_ascii_policy_and_invalid_bytes() {
    assert_eq!(lower("KKÉ".as_bytes()), "kkÉ".as_bytes());
    assert_eq!(lower(b"A\xff"), b"a\xff");
}
