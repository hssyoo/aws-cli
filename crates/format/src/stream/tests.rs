use super::*;

/// Each write is encoded on its own: the one that fails writes nothing and
/// names its position within itself; the writes before it stay.
#[test]
fn a_write_that_cannot_be_encoded_leaves_the_writes_before_it() {
    let mut stdout = Encoded::new(Errors::SurrogateEscape);
    assert_eq!(stdout.write(b"ok "), Ok(()));
    // U+DC80 is its byte on stdout; U+D800 is not writable.
    assert_eq!(stdout.write(b"x\xed\xb2\x80"), Ok(()));
    let failed = stdout.write(b"bad\xed\xa0\x80");
    assert_eq!(
        failed.map_err(|error| error.message),
        Err(
            "'utf-8' codec can't encode character '\\ud800' in position 3: surrogates not allowed"
                .to_owned()
        )
    );
    assert!(stdout.wrote());
    assert_eq!(stdout.into_bytes(), b"ok x\x80");
}

/// The pager's pipe is strict, so even U+DC80 fails there; a failed first
/// write still counts as the write that starts the pager.
#[test]
fn a_strict_stream_fails_on_every_lone_surrogate() {
    let mut pipe = Encoded::new(Errors::Strict);
    assert!(!pipe.wrote());
    assert!(pipe.write(b"\xed\xb2\x80").is_err());
    assert!(pipe.wrote());
    assert!(pipe.into_bytes().is_empty());
}
