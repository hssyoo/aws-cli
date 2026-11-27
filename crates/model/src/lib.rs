//! botocore's service models, read at run time: a C2J front-end
//! ([`frontend`]) builds a model's Smithy-shaped schemas ([`schema`]), and
//! the botocore overlay ([`overlay`]) holds what botocore reads beside
//! them. Beside those, a service's paginators file parsed once
//! ([`paginators`]), and the dynamic values, JSON writer, validator and
//! name transform every consumer of them shares; [`serde`] is a root
//! filled step by step.
#![forbid(unsafe_code)]

pub mod frontend;
#[cfg(any(test, feature = "testing"))]
pub mod fuzz;
pub mod overlay;
pub mod paginators;
pub mod payload;
pub mod pyjmespath;
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
