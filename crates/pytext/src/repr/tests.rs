use super::*;
use crate::testing::{self, Result};

#[test]
fn printable_matches_python_str_isprintable_for_every_scalar_value() -> Result<()> {
    let script = r#"
import sys
sys.stdout.write(''.join('1' if chr(c).isprintable() else '0' for c in range(0x110000)))
"#;
    let expected = testing::python(script, b"")?;
    let mut mismatches = Vec::new();
    for (code, flag) in expected.iter().enumerate() {
        let Some(character) = u32::try_from(code).ok().and_then(char::from_u32) else {
            continue;
        };
        if is_printable(character) != (*flag == b'1') {
            mismatches.push(code);
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} differ, first {:x?}",
        mismatches.len(),
        &mismatches[..mismatches.len().min(8)]
    );
    Ok(())
}
