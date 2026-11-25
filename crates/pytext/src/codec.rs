//! `str.encode('utf-8', errors)` for the three error handlers the CLI's
//! streams use: `strict` (a pipe), `surrogateescape` (stdout) and
//! `backslashreplace` (stderr), over text in [`crate::wtf8`]'s encoding (a
//! `&str` passes as its bytes).
//!
//! As CPython's UTF-8 encoder does, a handler is given each run of
//! consecutive surrogates: `strict` fails on the whole run, and
//! `surrogateescape` writes the run only if every one is U+DC80 to U+DCFF,
//! else it fails from the first that is not to the run's end.
use crate::wtf8::{code_points, is_surrogate};
use std::borrow::Cow;

/// An error handler of `str.encode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Errors {
    Strict,
    /// U+DC80 to U+DCFF as the byte below U+DC00 it stands for.
    SurrogateEscape,
    /// Each surrogate as its `\udXXX` escape.
    BackslashReplace,
}

/// The `UnicodeEncodeError` of a run of surrogates the handler cannot
/// write: code points `start` to `end` (exclusive) of the text given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodeError {
    pub start: usize,
    pub end: usize,
    /// The run's first code point, which the text names for a run of one.
    pub first: u32,
}

/// `str(UnicodeEncodeError)`, as CPython words it.
impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.end - self.start == 1 {
            write!(
                f,
                "'utf-8' codec can't encode character '\\u{:04x}' in position {}: surrogates not \
                 allowed",
                self.first, self.start
            )
        } else {
            write!(
                f,
                "'utf-8' codec can't encode characters in position {}-{}: surrogates not allowed",
                self.start,
                self.end - 1
            )
        }
    }
}

impl std::error::Error for EncodeError {}

/// `text.encode('utf-8', errors)`. Text without a surrogate is its bytes,
/// borrowed.
///
/// # Errors
/// The first run of surrogates `errors` cannot write.
pub fn encode_utf8(text: &[u8], errors: Errors) -> Result<Cow<'_, [u8]>, EncodeError> {
    if !has_surrogate(text) {
        return Ok(Cow::Borrowed(text));
    }
    let mut out = Vec::with_capacity(text.len());
    let points: Vec<u32> = code_points(text).collect();
    let mut index = 0;
    while index < points.len() {
        let point = points[index];
        if !is_surrogate(point) {
            crate::wtf8::push_code_point(&mut out, point);
            index += 1;
            continue;
        }
        let start = index;
        while points.get(index).copied().is_some_and(is_surrogate) {
            index += 1;
        }
        let run = &points[start..index];
        let failure = EncodeError {
            start,
            end: index,
            first: point,
        };
        match errors {
            Errors::Strict => return Err(failure),
            Errors::SurrogateEscape => {
                // As `unicode_encode_utf8` does: the escapable surrogates
                // that lead the run are written, and the error runs from the
                // first one that is not to the run's end.
                let escapable = run
                    .iter()
                    .take_while(|point| (0xDC80..=0xDCFF).contains(*point))
                    .count();
                if let Some(bad) = run.get(escapable) {
                    return Err(EncodeError {
                        start: start + escapable,
                        end: index,
                        first: *bad,
                    });
                }
                // Each is U+DC80 to U+DCFF, so its low byte is the byte.
                out.extend(run.iter().map(|point| point.to_le_bytes()[0]));
            }
            Errors::BackslashReplace => {
                for point in run {
                    out.extend(format!("\\u{point:04x}").bytes());
                }
            }
        }
    }
    Ok(Cow::Owned(out))
}

/// Whether `text` holds a surrogate: in this encoding, an `ED` byte
/// followed by `A0` or more, which no other code point has.
fn has_surrogate(text: &[u8]) -> bool {
    let mut rest = text;
    while let Some(lead) = rest.iter().position(|byte| *byte == 0xED) {
        if rest.get(lead + 1).is_some_and(|byte| *byte >= 0xA0) {
            return true;
        }
        rest = &rest[lead + 1..];
    }
    false
}

/// `text.encode('utf-8', 'backslashreplace')`, which never fails, as Rust
/// text: what stderr writes.
pub fn backslashreplace(text: &[u8]) -> Cow<'_, str> {
    match encode_utf8(text, Errors::BackslashReplace) {
        Ok(Cow::Borrowed(bytes)) => String::from_utf8_lossy(bytes),
        Ok(Cow::Owned(bytes)) => Cow::Owned(String::from_utf8_lossy(&bytes).into_owned()),
        // backslashreplace writes every surrogate.
        Err(_) => String::from_utf8_lossy(text),
    }
}

#[cfg(test)]
mod tests;
