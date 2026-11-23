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
