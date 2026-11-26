//! The cold trait map: traits no code reads, keyed by their id text.
use super::builder::BuildError;
use super::id::ShapeId;
use super::known::{Storage, TraitId};
use super::uninterpreted::Site;
use serde_json::Value as Json;

/// The namespace of the C2J keys with no faithful Smithy trait. Only
/// [`super::KNOWN`] ids use it; the Smithy front-end never writes it.
pub const C2J_NAMESPACE: &str = "c2j";

/// Cold traits by id text, in insertion order.
pub(super) type ColdEntries = Vec<(Box<str>, Json)>;

/// A schema's cold traits while a front-end builds them, in insertion
/// order. Only known cold or ignored ids go in through [`Self::insert`]; an
/// unknown id goes in only through [`Self::insert_unknown`], which records
/// it as uninterpreted, so an unknown trait always refuses.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ColdTraits {
    entries: ColdEntries,
    unknown: Vec<(Site, Box<str>)>,
}

impl ColdTraits {
    /// Adds a known trait whose storage is not a slot.
    pub fn insert(&mut self, id: &str, value: Json) -> Result<(), BuildError> {
        let known = TraitId::parse(id).ok_or_else(|| BuildError::UnknownTrait(id.into()))?;
        if known.storage() == Storage::Slot {
            return Err(BuildError::SlotTrait(id.into()));
        }
        self.push(id, value)
    }

    /// Adds a trait no [`super::KNOWN`] entry names, and records `key` at
    /// `site` as uninterpreted when the set takes these traits.
    pub fn insert_unknown(
        &mut self,
        id: &str,
        value: Json,
        site: Site,
        key: &str,
    ) -> Result<(), BuildError> {
        if TraitId::parse(id).is_some() {
            return Err(BuildError::KnownTrait(id.into()));
        }
        let parsed = ShapeId::parse(id).map_err(BuildError::ShapeId)?;
        if parsed.member().is_some() || parsed.namespace() == C2J_NAMESPACE {
            return Err(BuildError::UnknownTrait(id.into()));
        }
        self.push(id, value)?;
        self.unknown.push((site, key.into()));
        Ok(())
    }

    fn push(&mut self, id: &str, value: Json) -> Result<(), BuildError> {
        if self.get(id).is_some() {
            return Err(BuildError::DuplicateTrait(id.into()));
        }
        self.entries.push((id.into(), value));
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&Json> {
        let entry = self.entries.iter().find(|(key, _)| &**key == id);
        entry.map(|(_, value)| value)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Smithy's member-over-target merge: `over`'s entries replace this
    /// set's with the same id and the rest follow in `over`'s order. The
    /// unknown records stay `over`'s own: the target recorded its own.
    #[must_use]
    pub fn overlaid(&self, over: &Self) -> Self {
        let mut entries: Vec<_> = self
            .entries
            .iter()
            .map(|(id, value)| (id.clone(), over.get(id).unwrap_or(value).clone()))
            .collect();
        let added = over.entries.iter().filter(|(id, _)| self.get(id).is_none());
        entries.extend(added.cloned());
        Self {
            entries,
            unknown: over.unknown.clone(),
        }
    }

    pub(super) fn into_parts(self) -> (ColdEntries, Vec<(Site, Box<str>)>) {
        (self.entries, self.unknown)
    }
}
