//! The one command for every model operation, `aws <service> <operation>`:
//! the admission that decides, before any request, whether the native
//! route runs it, and the request it binds.
//!
//! Admission runs in [`admit::admit`]'s order: the protocol, the traits,
//! the customizations that touch the invocation ([`coverage`]), the
//! supplied arguments ([`bind`]), then validation. Settings, the output
//! format and the endpoint are the router's stages, after binding.
#![forbid(unsafe_code)]

pub mod admit;
pub mod bind;
pub mod coverage;
pub mod source;

pub use source::ServiceSource;

#[cfg(test)]
mod testing;
