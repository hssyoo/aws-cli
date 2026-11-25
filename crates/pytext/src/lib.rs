//! CPython's text primitives, one implementation each, for every crate that
//! must spell or read text as Python does: `float()` and `int()` of a
//! literal with the Unicode digits and spaces they read, `float.__repr__`,
//! `str.isprintable`, `str.isspace` and `str.strip`, the UTF-8 decode error,
//! the reprs of `str` and `bytes`, base64, a `str` that holds lone
//! surrogates ([`wtf8::Wtf8Buf`]) and the UTF-8 encoders the CLI's streams
//! use ([`codec`]). No dependencies.
#![forbid(unsafe_code)]

pub mod base64;
pub mod codec;
pub mod number;
pub mod repr;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod text;
pub mod wtf8;

#[cfg(test)]
mod tests;
