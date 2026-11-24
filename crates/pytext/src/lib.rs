//! CPython's text primitives, one implementation each, for every crate that
//! must spell or read text as Python does: `float()` and `int()` of a
//! literal with the Unicode digits and spaces they read, `float.__repr__`,
//! `str.isprintable`, `str.isspace` and `str.strip`, the UTF-8 decode error,
//! the reprs of `str` and `bytes`, and base64. No dependencies.
#![forbid(unsafe_code)]

pub mod base64;
pub mod number;
pub mod repr;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod text;

#[cfg(test)]
mod tests;
