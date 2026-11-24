use super::*;
use crate::testing::{self, Result};

#[test]
fn whitespace_matches_python_str_isspace_for_every_scalar_value() -> Result<()> {
    let script = r#"
import sys
sys.stdout.write(''.join('1' if chr(c).isspace() else '0' for c in range(0x110000)))
"#;
    let expected = testing::python(script, b"")?;
    let mismatches: Vec<usize> = expected
        .iter()
        .enumerate()
        .filter(|(code, flag)| {
            u32::try_from(*code)
                .ok()
                .and_then(char::from_u32)
                .is_some_and(|character| is_space(character) != (**flag == b'1'))
        })
        .map(|(code, _)| code)
        .collect();
    assert!(mismatches.is_empty(), "{mismatches:x?}");
    assert_eq!(strip("\u{1c}\u{2000} a b\u{85}\u{1f}"), "a b");
    Ok(())
}

/// Each case's text is CPython's own `str(UnicodeDecodeError)`.
#[test]
fn a_utf_8_decode_error_reads_as_cpython_s() -> Result<()> {
    let cases: [&[u8]; 5] = [
        b"ok \xff",
        b"\xc3",
        b"a\xe2\x82",
        b"\xe2\x28\xa1",
        b"\xf0\x9f\x98",
    ];
    let script = r#"
import sys
for line in sys.stdin.buffer.read().split(b'\n'):
    try:
        bytes.fromhex(line.decode()).decode('utf-8')
        print('ok')
    except UnicodeDecodeError as error:
        print(error)
"#;
    let input: Vec<String> = cases
        .iter()
        .map(|bytes| bytes.iter().map(|byte| format!("{byte:02x}")).collect())
        .collect();
    let expected = String::from_utf8(testing::python(script, input.join("\n").as_bytes())?)?;
    let actual: Vec<String> = cases
        .iter()
        .map(|bytes| decode_utf8(bytes).map_or_else(|text| text, |_| "ok".to_owned()))
        .collect();
    assert_eq!(expected.lines().collect::<Vec<_>>(), actual);
    assert_eq!(decode_utf8("é".as_bytes()), Ok("é"));
    Ok(())
}
