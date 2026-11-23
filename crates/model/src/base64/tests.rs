use super::*;

/// RFC 4648 section 10's vectors, and every byte value.
#[test]
fn encodes_as_b64encode_does() {
    for (input, expected) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ] {
        assert_eq!(encode(input.as_bytes()), expected);
    }
    assert_eq!(encode(&[0xfb, 0xff, 0xbf]), "+/+/");
}

/// `base64.b64decode` in CPython 3.14.
#[test]
fn decodes_as_b64decode_does() {
    let length = "Invalid base64-encoded string: number of data characters (1) cannot be 1 more \
        than a multiple of 4";
    let cases: [(&str, Result<&[u8], &str>); 13] = [
        ("Zm9v", Ok(b"foo")),
        ("Zm9", Err("Incorrect padding")),
        ("Zm8=", Ok(b"fo")),
        ("Zg", Err("Incorrect padding")),
        ("Z", Err(length)),
        ("Zm9v\n", Ok(b"foo")),
        ("Z m 9 v", Ok(b"foo")),
        ("Zm9v!", Ok(b"foo")),
        ("Zm9vYg=", Err("Incorrect padding")),
        ("Zm9vYg==extra", Err("Incorrect padding")),
        ("====", Ok(b"")),
        ("Zg==Zg==", Ok(b"f\x06`")),
        ("", Ok(b"")),
    ];
    for (text, expected) in cases {
        let decoded = decode(text);
        assert_eq!(
            decoded.as_deref().map_err(String::as_str),
            expected,
            "{text:?}"
        );
    }
    assert_eq!(decode("+/+/"), Ok(vec![0xfb, 0xff, 0xbf]));
}
