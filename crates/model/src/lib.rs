//! botocore's service models, read at run time into one arena, and the
//! dynamic values, JSON writer, validator and name transform every consumer
//! of them shares.
#![forbid(unsafe_code)]

pub mod base64;
pub mod payload;
pub mod pyjson;
pub mod pyrepr;
pub mod search;
pub mod shapes;
pub mod source;
pub mod validate;
pub mod value;
pub mod xform;

#[cfg(test)]
mod testing;
