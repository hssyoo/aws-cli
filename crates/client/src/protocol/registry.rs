//! botocore's choice of a wire protocol (`ServiceModel.resolved_protocol`)
//! and the implementations it can pick, under the botocore names that
//! select them.
use super::{ClientProtocol, PRIORITY};
use crate::admission::Unsupported;
use model::schema::{TraitId, traits};
use model::shapes::Metadata;

/// Why a model has no protocol botocore would speak.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unresolved {
    /// `UnsupportedServiceProtocolsError`: `protocols` lists none botocore
    /// supports.
    Unsupported(Vec<String>),
    /// `UndefinedModelAttributeError`: neither `protocols` nor `protocol`.
    Missing,
}

/// The protocol botocore speaks to a service (`ServiceModel.
/// resolved_protocol`): with a nonempty `protocols`, the first of
/// [`PRIORITY`] it lists; otherwise `protocol`. The unordered set of
/// protocol traits is never read.
///
/// # Errors
/// See [`Unresolved`].
pub fn resolved_protocol(metadata: &Metadata) -> Result<&str, Unresolved> {
    match metadata.protocols.as_deref() {
        Some(listed) if !listed.is_empty() => PRIORITY
            .into_iter()
            .find(|protocol| listed.iter().any(|name| &**name == *protocol))
            .ok_or_else(|| Unresolved::Unsupported(listed.iter().map(|n| n.to_string()).collect())),
        _ => metadata.protocol.as_deref().ok_or(Unresolved::Missing),
    }
}

/// The implemented protocols, each under the botocore name that selects
/// it. One name may hold several, told apart by their protocol trait.
#[derive(Debug)]
pub struct ProtocolRegistry {
    entries: &'static [(&'static str, &'static dyn ClientProtocol)],
}

impl ProtocolRegistry {
    pub const fn new(entries: &'static [(&'static str, &'static dyn ClientProtocol)]) -> Self {
        Self { entries }
    }

    /// The implementation botocore's resolved protocol names, the one
    /// whose trait `jsonVersion` names when the name holds several, else
    /// its first.
    ///
    /// # Errors
    /// [`Unsupported::Protocol`] with the resolved name (empty when none
    /// resolves) when no implementation is registered under it, even if
    /// one is under another name the model lists.
    pub fn resolve(&self, metadata: &Metadata) -> Result<&'static dyn ClientProtocol, Unsupported> {
        let name = resolved_protocol(metadata).unwrap_or_default();
        let wanted = json_version_trait(metadata);
        let named = || self.named_all(name);
        named()
            .find(|protocol| Some(protocol.protocol_id()) == wanted)
            .or_else(|| named().next())
            .ok_or_else(|| Unsupported::Protocol(name.to_owned()))
    }

    /// The first implementation registered under the botocore `name`.
    pub fn named(&self, name: &str) -> Option<&'static dyn ClientProtocol> {
        self.named_all(name).next()
    }

    /// The implementation of the protocol trait `id`.
    pub fn get(&self, id: TraitId) -> Option<&'static dyn ClientProtocol> {
        let mut entries = self.entries.iter().map(|(_, protocol)| *protocol);
        entries.find(|protocol| protocol.protocol_id() == id)
    }

    fn named_all<'a>(
        &self,
        name: &'a str,
    ) -> impl Iterator<Item = &'static dyn ClientProtocol> + use<'a> {
        let entries = self.entries.iter();
        entries
            .filter(move |(key, _)| *key == name)
            .map(|(_, protocol)| *protocol)
    }
}

/// The awsJson trait a model's `jsonVersion` names; the JSON serializer
/// reads the version itself, so this only tells the instances apart.
fn json_version_trait(metadata: &Metadata) -> Option<TraitId> {
    match metadata.json_version.as_deref()? {
        "1.0" => Some(traits::AWS_JSON_1_0),
        "1.1" => Some(traits::AWS_JSON_1_1),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
