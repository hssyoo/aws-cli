//! Endpoint-rule parameters and owned resolutions from the CRT engine.

/// A rule parameter's value, of one of the kinds a rule set declares
/// (`String`, `Boolean`, `stringArray`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamValue<'a> {
    String(&'a str),
    Boolean(bool),
    StringArray(&'a [&'a str]),
}
/// The parameters of one resolution, each by the name the rule set gives
/// it, as botocore's `EndpointRulesetResolver` hands the provider its
/// parameter dict. A parameter left out is unset, which a rule set's
/// default then fills; a name given twice takes its last value.
pub type Parameters<'a> = &'a [(&'a str, ParamValue<'a>)];
/// The two JSON documents a resolution evaluates, named because both are
/// byte slices that a positional call could swap unnoticed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Documents<'a> {
    /// The service's endpoint rule set.
    pub rules_json: &'a [u8],
    /// The partitions that the rule set's partition function reads.
    pub partitions_json: &'a [u8],
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    Endpoint { url: Vec<u8>, properties: Vec<u8> },
    RuleError(Vec<u8>),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResolveError {
    OutOfMemory,
    InvalidRuleset,
    InvalidPartitions,
    ResolveFailed,
}
impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::OutOfMemory => "endpoint allocation failed",
            Self::InvalidRuleset => "invalid endpoint ruleset",
            Self::InvalidPartitions => "invalid endpoint partitions",
            Self::ResolveFailed => "endpoint resolution failed",
        })
    }
}
impl std::error::Error for ResolveError {}
mod native;
#[doc(inline)]
pub use native::resolve;
#[cfg(test)]
mod tests;
