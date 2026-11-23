//! Loads the lexicographically latest service endpoint model from a payload,
//! through `model::payload`.
use pyload::View;
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
    Resolve(crt::endpoints::ResolveError),
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
            Self::Resolve(error) => error.fmt(f),
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
/// A service's decompressed rule set and the partitions it reads, shared by
/// every evaluation for a command.
#[derive(Clone, PartialEq, Eq)]
pub struct Rules {
    pub rules: std::sync::Arc<[u8]>,
    pub partitions: std::sync::Arc<[u8]>,
}
impl std::fmt::Debug for Rules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rules")
            .field("rules", &self.rules.len())
            .field("partitions", &self.partitions.len())
            .finish()
    }
}
impl Rules {
    /// Loads the service's latest rule set and the partitions from a payload.
    pub fn load(view: View<'_>, service: &str) -> Result<Self, RulesError> {
        let rules_name =
            model::payload::versioned_resource_name(view, service, "endpoint-rule-set-1")?;
        let rules = model::payload::load_resource(view, rules_name)?;
        let partitions =
            model::payload::load_resource(view, b"awscli/botocore/data/partitions.json")?;
        Ok(Self {
            rules: rules.into(),
            partitions: partitions.into(),
        })
    }
    pub fn resolve(
        &self,
        parameters: crt::endpoints::Parameters<'_>,
    ) -> Result<crt::endpoints::Resolved, RulesError> {
        let documents = crt::endpoints::Documents {
            rules_json: &self.rules,
            partitions_json: &self.partitions,
        };
        crt::endpoints::resolve(documents, parameters).map_err(RulesError::Resolve)
    }
}
pub fn resolve(
    view: View<'_>,
    service: &str,
    parameters: crt::endpoints::Parameters<'_>,
) -> Result<crt::endpoints::Resolved, RulesError> {
    Rules::load(view, service)?.resolve(parameters)
}
#[cfg(test)]
mod tests;
