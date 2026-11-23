//! The command registry: what the router resolves a parsed lineage to, and
//! the shape of a bound, admitted command. Model-derived commands come from
//! an [`OperationSource`], which supplies their tables, and a [`Binder`],
//! which binds a completed parse; no service, operation or member is named
//! here. The `s3` commands keep the router's own enum path and are
//! registered only as a pinned grammar ([`Registry::new`]).
#![forbid(unsafe_code)]
pub mod models;
pub mod output;
pub mod patterns;
mod registry;
mod spec;
#[doc(inline)]
pub use output::{OutputFormat, OutputGlobals, OutputSettings, PagerProfile, PagerSettings};
#[doc(inline)]
pub use registry::{Binder, Operation, OperationSource, Registry, Resolution};
pub use spec::{Admission, Context, Invocation, Needs, Preflight, StreamPolicy};
#[cfg(test)]
mod tests;
