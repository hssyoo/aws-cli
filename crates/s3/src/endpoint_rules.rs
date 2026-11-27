//! Loads the lexicographically latest service endpoint model from a payload,
//! through `model::payload`, and evaluates it with the native rules engine.
use endpoint_rules::{
    DecodeError, EvalError, EvalValue, Library, Outcome, Params, Partitions, Program,
};
use pyload::View;
use std::sync::Arc;
/// A rule parameter's value, of one of the kinds the S3 client gives
/// (`String`, `Boolean`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamValue<'a> {
    String(&'a str),
    Boolean(bool),
}
/// The parameters of one resolution, each by the name the rule set gives
/// it, as botocore's `EndpointRulesetResolver` hands the provider its
/// parameter dict. A parameter left out is unset, which a rule set's
/// default then fills; a name given twice takes its last value; a name the
/// rule set does not declare is dropped.
pub type Parameters<'a> = &'a [(&'a str, ParamValue<'a>)];
#[derive(Debug)]
#[non_exhaustive]
pub enum RulesError {
    InvalidServiceName,
    MissingEndpointRules,
    InvalidEndpointRules,
    /// `DecodeFailed` or `OutputLengthMismatch`: a compressed resource is corrupt or does not
    /// decode to exactly its declared length.
    Decode(pyload::decompress::Error),
    /// A resource's bytes or its decoder scratch could not be reserved.
    OutOfMemory,
    Payload(pyload::Error),
    /// The rule set or the partitions the engine refuses.
    Rules(DecodeError),
    /// An evaluation that reaches neither an endpoint nor a rule's error.
    Evaluate(EvalError),
}
impl std::fmt::Display for RulesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidServiceName => f.write_str("invalid service name"),
            Self::MissingEndpointRules => f.write_str("missing endpoint rules"),
            Self::InvalidEndpointRules => f.write_str("invalid endpoint rules"),
            Self::Decode(error) => error.fmt(f),
            Self::OutOfMemory => f.write_str("out of memory"),
            Self::Payload(error) => write!(f, "invalid endpoint payload: {error}"),
            Self::Rules(error) => error.fmt(f),
            Self::Evaluate(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for RulesError {}
impl From<pyload::Error> for RulesError {
    fn from(error: pyload::Error) -> Self {
        Self::Payload(error)
    }
}
impl From<model::payload::ResourceError> for RulesError {
    fn from(error: model::payload::ResourceError) -> Self {
        use model::payload::ResourceError;
        use pyload::catalog::GroupError;
        match error {
            ResourceError::InvalidServiceName => Self::InvalidServiceName,
            ResourceError::Missing => Self::MissingEndpointRules,
            ResourceError::InvalidLength => Self::InvalidEndpointRules,
            ResourceError::Decode(error) => Self::Decode(error),
            ResourceError::OutOfMemory => Self::OutOfMemory,
            ResourceError::Payload(error) => Self::Payload(error),
            ResourceError::Catalog(GroupError::Payload(error)) => Self::Payload(error),
            ResourceError::Catalog(GroupError::Unsorted) => {
                Self::Payload(pyload::Error::UnsortedNames)
            }
            ResourceError::Catalog(GroupError::NameTooLong) => Self::InvalidServiceName,
            _ => Self::InvalidEndpointRules,
        }
    }
}
/// A service's decoded rule set and the partitions it reads, shared by
/// every evaluation for a command.
#[derive(Clone, PartialEq, Eq)]
pub struct Rules {
    program: Arc<Program>,
    partitions: Arc<Partitions>,
}
impl std::fmt::Debug for Rules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rules")
            .field("parameters", &self.program.params().len())
            .finish_non_exhaustive()
    }
}
impl Rules {
    /// Loads and decodes the service's latest rule set and the partitions
    /// from a payload; the rule set reports before the partitions, as the
    /// CRT engine read them.
    pub fn load(view: View<'_>, service: &str) -> Result<Self, RulesError> {
        let rules_name =
            model::payload::versioned_resource_name(view, service, "endpoint-rule-set-1")?;
        let rules = model::payload::load_resource(view, rules_name)?;
        let partitions =
            model::payload::load_resource(view, b"awscli/botocore/data/partitions.json")?;
        Ok(Self {
            program: Arc::new(Program::decode(&rules).map_err(RulesError::Rules)?),
            partitions: Arc::new(Partitions::decode(&partitions).map_err(RulesError::Rules)?),
        })
    }
    /// Evaluates the rule set for `parameters`.
    pub fn resolve(&self, parameters: Parameters<'_>) -> Result<Outcome, RulesError> {
        let mut args = Params::new(&self.program);
        for &(name, value) in parameters {
            if let Some(def) = self.program.param(name) {
                args.set(
                    def.slot(),
                    match value {
                        ParamValue::String(text) => EvalValue::str(text),
                        ParamValue::Boolean(flag) => EvalValue::Bool(flag),
                    },
                );
            }
        }
        let library = Library {
            partitions: &self.partitions,
        };
        self.program
            .evaluate(&args, &library)
            .map_err(RulesError::Evaluate)
    }
}
pub fn resolve(
    view: View<'_>,
    service: &str,
    parameters: Parameters<'_>,
) -> Result<Outcome, RulesError> {
    Rules::load(view, service)?.resolve(parameters)
}
#[cfg(test)]
mod tests;
