//! A Rust source mask: the same bytes, with the contents of string and
//! character literals and of comments replaced by [`FILL`] (newlines kept),
//! so code-only scans line up with the original byte for byte and never
//! see text in a literal or a comment. The delimiters (`"`, `r#"`, `//`,
//! `/*`, `*/`) are kept.

/// What a literal's or a comment's bytes become: not whitespace, so code
/// whitespace stays apart from the text of literals.
pub const FILL: u8 = 0x01;

pub fn mask(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = bytes.to_vec();
    let mut index = 0;
    while index < bytes.len() {
        if let Some((hashes, quote)) = raw_start(bytes, index) {
            index = string(bytes, &mut out, quote + 1, Some(hashes));
            continue;
        }
        index = match bytes[index] {
            b'/' if bytes.get(index + 1) == Some(&b'/') => line_comment(bytes, &mut out, index),
            b'/' if bytes.get(index + 1) == Some(&b'*') => block_comment(bytes, &mut out, index),
            b'"' => string(bytes, &mut out, index + 1, None),
            b'\'' => character(bytes, &mut out, index),
            _ => index + 1,
        };
    }
    String::from_utf8(out)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

fn blank(out: &mut [u8], from: usize, to: usize) {
    for byte in out.iter_mut().take(to).skip(from) {
        if *byte != b'\n' {
            *byte = FILL;
        }
    }
}

fn line_comment(bytes: &[u8], out: &mut [u8], index: usize) -> usize {
    let end = bytes[index..]
        .iter()
        .position(|&byte| byte == b'\n')
        .map_or(bytes.len(), |offset| index + offset);
    blank(out, index + 2, end);
    end
}

fn block_comment(bytes: &[u8], out: &mut [u8], index: usize) -> usize {
    let mut depth = 0;
    let mut at = index;
    while at < bytes.len() {
        match (bytes[at], bytes.get(at + 1)) {
            (b'/', Some(b'*')) => depth += 1,
            (b'*', Some(b'/')) => depth -= 1,
            _ => {
                at += 1;
                continue;
            }
        }
        at += 2;
        if depth == 0 {
            blank(out, index + 2, at - 2);
            return at;
        }
    }
    blank(out, index + 2, bytes.len());
    bytes.len()
}

/// A raw string's `#` count and opening quote, for `r"`, `r#"`, `br"`...
/// starting at `index` outside an identifier.
fn raw_start(bytes: &[u8], index: usize) -> Option<(usize, usize)> {
    let before = index.checked_sub(1).map(|before| bytes[before]);
    if before.is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_') {
        return None;
    }
    if !matches!(bytes[index], b'r' | b'b') {
        return None;
    }
    let mut at = index;
    if bytes[at] == b'b' {
        at += 1;
    }
    if bytes.get(at) != Some(&b'r') {
        return None;
    }
    at += 1;
    let hashes = bytes[at..].iter().take_while(|&&byte| byte == b'#').count();
    (bytes.get(at + hashes) == Some(&b'"')).then_some((hashes, at + hashes))
}

/// Blanks a string body from `start` to its closing quote (followed by
/// the `#` count of a raw string, where `\` escapes nothing).
fn string(bytes: &[u8], out: &mut [u8], start: usize, raw: Option<usize>) -> usize {
    let hashes = raw.unwrap_or(0);
    let raw = raw.is_some();
    let mut at = start;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' if !raw => at += 2,
            b'"' if bytes[at + 1..]
                .iter()
                .take(hashes)
                .all(|&byte| byte == b'#')
                && bytes.len() >= at + 1 + hashes =>
            {
                blank(out, start, at);
                return at + 1 + hashes;
            }
            _ => at += 1,
        }
    }
    blank(out, start, bytes.len());
    bytes.len()
}

/// A character literal (`'x'`, `'\n'`, `'\u{1F600}'`, `'é'`), or a lifetime
/// or label, which is left as code.
fn character(bytes: &[u8], out: &mut [u8], index: usize) -> usize {
    let close = if bytes.get(index + 1) == Some(&b'\\') {
        bytes[index + 2..]
            .iter()
            .take(12)
            .position(|&byte| byte == b'\'')
            .map(|offset| index + 2 + offset)
    } else {
        let width = bytes.get(index + 1).map_or(1, |&byte| utf8_width(byte));
        (bytes.get(index + 1 + width) == Some(&b'\'')).then_some(index + 1 + width)
    };
    match close {
        Some(close) => {
            blank(out, index + 1, close);
            close + 1
        }
        None => index + 1,
    }
}

fn utf8_width(byte: u8) -> usize {
    match byte.leading_ones() {
        2 => 2,
        3 => 3,
        4 => 4,
        _ => 1,
    }
}
