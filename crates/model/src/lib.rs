//! botocore's service models, read at run time into one arena, a service's
//! paginators file parsed once ([`paginators`]), and the dynamic values,
//! JSON writer, validator and name transform every consumer of them shares.
#![forbid(unsafe_code)]

#[cfg(any(test, feature = "testing"))]
pub mod fuzz;
pub mod paginators;
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
