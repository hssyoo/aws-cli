//! A Python `str` that holds lone surrogates, which no Rust `String` can:
//! [`Wtf8Buf`], and the code points of any text in its encoding.
//!
//! The encoding is UTF-8 generalized to every code point Python allows:
//! each code point is encoded on its own, a surrogate (U+D800 to U+DFFF) as
//! the three bytes UTF-8 would give it. Unlike strict WTF-8, a high
//! surrogate followed by a low one stays two code points, as
//! `'\ud800' + '\udc00'` does in Python, distinct from `'\U00010000'`.
//! Valid UTF-8 is such text unchanged, so a `&str` passes wherever these
//! bytes are taken. Comparing the bytes compares the code points, so
//! equality and order are Python's.

/// A Python `str` with at least one lone surrogate, in the encoding the
/// module describes. A `str` without one is a Rust `String`: nothing makes
/// a `Wtf8Buf` of valid UTF-8, so the two never stand for the same text.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Wtf8Buf(Box<[u8]>);

/// The text of a sequence of code points: a `String` when every one is a
/// Unicode scalar value, else a [`Wtf8Buf`].
pub type Text = Result<String, Wtf8Buf>;

impl Wtf8Buf {
    /// The text of `points`, each at most U+10FFFF: a `String` unless one
    /// is a surrogate.
    ///
    /// # Panics
    /// Never: a code point past U+10FFFF is taken as U+FFFD, which no
    /// caller passes.
    pub fn from_code_points(points: impl IntoIterator<Item = u32>) -> Text {
        let mut bytes = Vec::new();
        let mut surrogate = false;
        for point in points {
            surrogate |= is_surrogate(point);
            push_code_point(&mut bytes, point);
        }
        Self::from_generalized(bytes, surrogate)
    }

    /// The text of `bytes`, which must be text in this encoding (a `String`'s
    /// bytes, these, or both joined): a `String` unless it holds a
    /// surrogate.
    pub fn from_bytes(bytes: Vec<u8>) -> Text {
        let surrogate = code_points(&bytes).any(is_surrogate);
        Self::from_generalized(bytes, surrogate)
    }

    fn from_generalized(bytes: Vec<u8>, surrogate: bool) -> Text {
        if surrogate {
            return Err(Self(bytes.into_boxed_slice()));
        }
        // Without a surrogate the encoding is UTF-8.
        String::from_utf8(bytes).map_err(|error| Self(error.into_bytes().into_boxed_slice()))
    }

    /// The text's bytes, in the encoding the module describes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Its code points.
    pub fn code_points(&self) -> impl Iterator<Item = u32> + '_ {
        code_points(&self.0)
    }

    /// `len(text)`: its code points.
    pub fn len(&self) -> usize {
        self.code_points().count()
    }

    /// Never: a `Wtf8Buf` holds at least one surrogate.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The text with each surrogate as U+FFFD, for a message that is no
    /// Python output.
    pub fn to_string_lossy(&self) -> String {
        self.code_points()
            .map(|point| char::from_u32(point).unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect()
    }
}

/// Lists the code points, each surrogate as `\udXXX`, so a log names it.
impl std::fmt::Debug for Wtf8Buf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Wtf8Buf(\"")?;
        for point in self.code_points() {
            match char::from_u32(point) {
                Some(character) => write!(f, "{}", character.escape_debug())?,
                None => write!(f, "\\u{point:04x}")?,
            }
        }
        f.write_str("\")")
    }
}

/// Whether `point` is a surrogate, which only a Python `str` holds alone.
pub const fn is_surrogate(point: u32) -> bool {
    matches!(point, 0xD800..=0xDFFF)
}

/// Appends `point` to `bytes`, a surrogate as UTF-8's three-byte form.
pub fn push_code_point(bytes: &mut Vec<u8>, point: u32) {
    let point = if point > 0x10FFFF { 0xFFFD } else { point };
    // The truncating casts keep each byte's six or fewer bits.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "each byte takes the bits the mask leaves"
    )]
    match point {
        0..=0x7F => bytes.push(point as u8),
        0x80..=0x7FF => bytes.extend([0xC0 | (point >> 6) as u8, 0x80 | (point & 0x3F) as u8]),
        0x800..=0xFFFF => bytes.extend([
            0xE0 | (point >> 12) as u8,
            0x80 | ((point >> 6) & 0x3F) as u8,
            0x80 | (point & 0x3F) as u8,
        ]),
        _ => bytes.extend([
            0xF0 | (point >> 18) as u8,
            0x80 | ((point >> 12) & 0x3F) as u8,
            0x80 | ((point >> 6) & 0x3F) as u8,
            0x80 | (point & 0x3F) as u8,
        ]),
    }
}

/// The code points of `bytes`, text in the encoding the module describes.
/// A byte that starts no code point stands for U+FFFD, which only bytes
/// that are no such text contain.
pub fn code_points(bytes: &[u8]) -> impl Iterator<Item = u32> + '_ {
    let mut at = 0;
    std::iter::from_fn(move || {
        let first = *bytes.get(at)?;
        let (length, initial) = match first {
            0x00..=0x7F => (1, u32::from(first)),
            0xC0..=0xDF => (2, u32::from(first & 0x1F)),
            0xE0..=0xEF => (3, u32::from(first & 0x0F)),
            0xF0..=0xF7 => (4, u32::from(first & 0x07)),
            _ => {
                at += 1;
                return Some(0xFFFD);
            }
        };
        let tail = bytes.get(at + 1..at + length);
        at += length;
        Some(tail.map_or(0xFFFD, |tail| {
            tail.iter()
                .fold(initial, |point, byte| point << 6 | u32::from(byte & 0x3F))
        }))
    })
}

#[cfg(test)]
mod tests;
