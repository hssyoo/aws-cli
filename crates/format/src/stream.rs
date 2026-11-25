//! A Python text stream's writes, as bytes: `TextIOWrapper.write` encodes
//! each write on its own, so a write it cannot encode raises its
//! `UnicodeEncodeError` and writes nothing, while the writes before it stay
//! written. stdout encodes with surrogateescape and the pager's pipe, a
//! `Popen` text stream, strictly ([`crate::Destination::errors`]).
use crate::text::PythonError;
use pytext::codec::{Errors, encode_utf8};

/// The bytes of the writes made so far.
#[derive(Debug)]
pub struct Encoded {
    bytes: Vec<u8>,
    errors: Errors,
    /// A write was made, even one that failed: the pager's stream starts
    /// at the first (`LazyStdin`).
    wrote: bool,
}

impl Encoded {
    pub const fn new(errors: Errors) -> Self {
        Self {
            bytes: Vec::new(),
            errors,
            wrote: false,
        }
    }

    /// `stream.write(chunk)` of text in `pytext::wtf8`'s encoding, which a
    /// `&str` passes as its bytes.
    ///
    /// # Errors
    /// The `UnicodeEncodeError` of a lone surrogate the stream's error
    /// handler cannot write, as `str()` gives it.
    pub fn write(&mut self, chunk: &[u8]) -> Result<(), PythonError> {
        self.wrote = true;
        match encode_utf8(chunk, self.errors) {
            Ok(encoded) => {
                self.bytes.extend_from_slice(&encoded);
                Ok(())
            }
            Err(error) => Err(PythonError {
                message: error.to_string(),
            }),
        }
    }

    /// Whether any write was made.
    pub const fn wrote(&self) -> bool {
        self.wrote
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod tests;
