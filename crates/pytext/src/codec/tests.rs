use super::*;
use crate::testing::{self, Result};

/// Code-point sequences, each encoded with the three handlers.
const CASES: &[&[u32]] = &[
    &[0x61, 0xE9, 0x1F600],
    &[0x61, 0xD800, 0x62],
    &[0xDC80],
    &[0xDCFF, 0x61, 0xDC80],
    &[0x61, 0xDC80, 0xD800, 0x62],
    &[0xD800, 0xDC00],
    &[0x61, 0xDC7F],
    &[0xDD00, 0xDFFF, 0x7A],
    &[0xDC80, 0xD800, 0xD801, 0x61],
];

const ORACLE: &str = r#"
import json, sys
for points in json.loads(sys.stdin.read()):
    text = ''.join(map(chr, points))
    row = []
    for errors in ('strict', 'surrogateescape', 'backslashreplace'):
        try:
            row.append('ok ' + text.encode('utf-8', errors).hex())
        except UnicodeEncodeError as error:
            row.append('error ' + str(error))
    print(' | '.join(row))
"#;

fn text(points: &[u32]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for point in points {
        crate::wtf8::push_code_point(&mut bytes, *point);
    }
    bytes
}

fn ours(points: &[u32]) -> String {
    let bytes = text(points);
    [
        Errors::Strict,
        Errors::SurrogateEscape,
        Errors::BackslashReplace,
    ]
    .into_iter()
    .map(|errors| match encode_utf8(&bytes, errors) {
        Ok(encoded) => format!(
            "ok {}",
            encoded
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ),
        Err(error) => format!("error {error}"),
    })
    .collect::<Vec<_>>()
    .join(" | ")
}

#[test]
fn each_handler_encodes_and_fails_as_cpython_s() -> Result<()> {
    let input = format!(
        "[{}]",
        CASES
            .iter()
            .map(|points| format!(
                "[{}]",
                points
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ))
            .collect::<Vec<_>>()
            .join(",")
    );
    let expected = String::from_utf8(testing::python(ORACLE, input.as_bytes())?)?;
    let actual: Vec<String> = CASES.iter().map(|points| ours(points)).collect();
    assert_eq!(expected.lines().collect::<Vec<_>>(), actual);
    assert_eq!(backslashreplace(&text(&[0x61, 0xD800])), "a\\ud800");
    assert!(matches!(
        encode_utf8(b"plain", Errors::Strict),
        Ok(Cow::Borrowed(_))
    ));
    Ok(())
}
