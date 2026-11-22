//! The command registry: what the router resolves a parsed lineage to, and
//! the shape of a bound, admitted command. Hand-written commands register
//! a [`CommandSpec`]; model-derived ones come from an [`OperationSource`],
//! so no service, operation or member is named here. The `s3` commands
//! keep the router's own enum path and are registered only as a pinned
//! grammar ([`Registry::new`]).
#![forbid(unsafe_code)]
mod registry;
mod spec;
#[doc(inline)]
pub use registry::{Operation, OperationSource, Registry, Resolution};
#[doc(inline)]
pub use spec::{Admission, BindFn, CommandSpec, Context, Invocation, Needs, StreamPolicy};
#[cfg(test)]
mod tests;
