//! Python's `str` operations on text: what is whitespace
//! (`str.isspace`), `str.strip()`, and `bytes.decode('utf-8')` with the
//! error CPython raises.

/// `str.isspace()` of one character: Unicode's `White_Space`, and the
/// separators U+001C to U+001F, which CPython counts as whitespace
/// (bidirectional class B or S) and `White_Space` does not.
pub fn is_space(character: char) -> bool {
    character.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&character)
}

/// `str.strip()`: [`is_space`] characters off both ends.
pub fn strip(text: &str) -> &str {
    text.trim_matches(is_space)
}

/// `bytes.decode('utf-8')`; `Err` holds the `UnicodeDecodeError`'s text,
/// as CPython words it.
pub fn decode_utf8(bytes: &[u8]) -> Result<&str, String> {
    std::str::from_utf8(bytes).map_err(|error| {
        let start = error.valid_up_to();
        let first = bytes.get(start).copied().unwrap_or(0);
        let (end, reason) = match error.error_len() {
            None => (bytes.len(), "unexpected end of data"),
            Some(_) if !matches!(first, 0xc2..=0xf4) => (start + 1, "invalid start byte"),
            Some(len) => (start + len, "invalid continuation byte"),
        };
        let at = if end - start == 1 {
            format!("byte 0x{first:02x} in position {start}")
        } else {
            format!("bytes in position {start}-{}", end - 1)
        };
        format!("'utf-8' codec can't decode {at}: {reason}")
    })
}

#[cfg(test)]
mod tests;
