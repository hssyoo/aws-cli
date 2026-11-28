//! The one command for every model operation, `aws <service> <operation>`:
//! the admission that decides, before any request, whether the native
//! route runs it, and the request it binds.
//!
//! Admission runs in [`admit::admit`]'s order: the protocol, the traits,
//! a paginator the native one does not run, the customizations that touch
//! the invocation ([`coverage`]), the supplied arguments and the paging
//! options ([`bind`]), then validation. A paginated call pages natively
//! ([`invoke`]). Settings, the output
//! format and the endpoint are the router's stages, after binding.
#![forbid(unsafe_code)]

pub mod admit;
pub mod bind;
pub mod coverage;
pub mod credentials;
pub mod invoke;
pub mod query;
pub mod source;

pub use source::ServiceSource;

#[cfg(test)]
mod testing;
