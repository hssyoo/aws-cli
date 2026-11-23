use pytext::repr::{quote_for, write_escaped};
use std::fmt::Write as _;

pub(super) fn lower(bytes: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(bytes.len());
    for (code, encoded) in CodePoints(bytes) {
        if code == 0x212a {
            result.push(b'k');
        } else {
            result.extend(encoded.iter().map(u8::to_ascii_lowercase));
        }
    }
    result
}

struct CodePoints<'a>(&'a [u8]);

impl<'a> Iterator for CodePoints<'a> {
    type Item = (u32, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let first = *self.0.first()?;
        let width = match first {
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => 1,
        };
        if let Some(encoded) = self.0.get(..width)
            && let Ok(decoded) = std::str::from_utf8(encoded)
            && let Some(character) = decoded.chars().next()
        {
            self.0 = &self.0[width..];
            return Some((character as u32, encoded));
        }
        let encoded = &self.0[..1];
        self.0 = &self.0[1..];
        Some((0xdc00 + u32::from(first), encoded))
    }
}

pub(super) struct Words<'a> {
    remaining: &'a [u8],
}

impl<'a> Words<'a> {
    pub(super) fn new(bytes: &'a [u8]) -> Self {
        Self { remaining: bytes }
    }
}

impl<'a> Iterator for Words<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(width) = whitespace_width(self.remaining) {
            self.remaining = &self.remaining[width..];
        }
        if self.remaining.is_empty() {
            return None;
        }
        let mut length = 0;
        while length < self.remaining.len() && whitespace_width(&self.remaining[length..]).is_none()
        {
            length += 1;
        }
        let (word, rest) = self.remaining.split_at(length);
        self.remaining = rest;
        Some(word)
    }
}

fn whitespace_width(bytes: &[u8]) -> Option<usize> {
    let (code, encoded) = CodePoints(bytes).next()?;
    matches!(code,
        0x09..=0x0d | 0x1c..=0x20 | 0x85 | 0xa0 | 0x1680 | 0x2000..=0x200a |
        0x2028 | 0x2029 | 0x202f | 0x205f | 0x3000
    )
    .then_some(encoded.len())
}

/// `repr()` of a str as CPython spells it, the bytes decoded as CPython
/// decodes a path: UTF-8, with each byte of an invalid sequence its own lone
/// surrogate. The quoting and escapes are [`pytext::repr`]'s, what
/// `str.isprintable` rejects escaped; a lone surrogate is `\udcXX`.
pub(crate) fn repr(bytes: &[u8]) -> Vec<u8> {
    let quote = quote_for(bytes);
    let mut result = String::with_capacity(bytes.len() + 2);
    result.push(quote);
    let mut buffer = [0; 4];
    for (code, _) in CodePoints(bytes) {
        match char::from_u32(code) {
            Some(c) => write_escaped(&mut result, c.encode_utf8(&mut buffer), quote),
            None => {
                let _ = write!(result, "\\u{code:04x}");
            }
        }
    }
    result.push(quote);
    result.into_bytes()
}

#[cfg(test)]
mod tests;
