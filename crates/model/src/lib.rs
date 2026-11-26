//! botocore's service models, read at run time into one arena, a service's
//! paginators file parsed once ([`paginators`]), and the dynamic values,
//! JSON writer, validator and name transform every consumer of them shares.
//!
//! [`schema`] is the Smithy-shaped schema layer the arena moves onto;
//! [`frontend`], [`overlay`] and [`serde`] are its roots, filled step by
//! step.
#![forbid(unsafe_code)]

pub mod frontend;
#[cfg(any(test, feature = "testing"))]
pub mod fuzz;
pub mod overlay;
pub mod paginators;
pub mod payload;
pub mod pyjson;
pub mod pyrepr;
pub mod schema;
pub mod search;
pub mod serde;
pub mod shapes;
pub mod source;
pub mod validate;
pub mod value;
pub mod xform;

#[cfg(test)]
mod testing;
