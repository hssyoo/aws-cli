//! CPython's text primitives, one implementation each, for every crate that
//! must spell or read text as Python does: `float()` and `int()` of a
//! literal, `float.__repr__`, `str.isprintable`, the reprs of `str` and
//! `bytes`, and base64. No dependencies.
#![forbid(unsafe_code)]

pub mod base64;
pub mod number;
pub mod repr;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

#[cfg(test)]
mod tests;
