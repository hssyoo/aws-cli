//! The botocore overlay: what botocore reads beside the Smithy-shaped
//! schema. Its `metadata` keys no Smithy trait holds, the paginators file
//! loaded with the service, the waiters probe, and the schema patches a
//! Smithy front-end will apply (empty for C2J, which is the truth).
//!
//! [`ServiceFacts`] is what protocols and signers read: the overlay first,
//! then the service schema.
use crate::paginators::Paginators;
use crate::schema::ServiceSchema;
use crate::source::{Kind, LoadError, Source, load_service_model};
use serde_json::Value as Json;
use std::sync::OnceLock;

/// The `metadata` keys botocore reads that no Smithy trait holds: the
/// protocol selection (`protocol` and the ordered `protocols`),
/// `jsonVersion`, `targetPrefix`, `signatureVersion` and `uid`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BotocoreMetadata {
    pub protocol: Option<Box<str>>,
    pub protocols: Option<Box<[Box<str>]>>,
    pub json_version: Option<Box<str>>,
    pub target_prefix: Option<Box<str>>,
    pub signature_version: Option<Box<str>>,
    pub uid: Option<Box<str>>,
}

impl BotocoreMetadata {
    /// The keys of a C2J document's `metadata`, from the parse the C2J
    /// front-end read; it has already refused a key of the wrong type.
    pub fn from_c2j(document: &Json) -> Self {
        let metadata = document.get("metadata");
        let text = |key: &str| {
            let value = metadata.and_then(|metadata| metadata.get(key));
            value.and_then(Json::as_str).map(Box::from)
        };
        let protocols = metadata.and_then(|metadata| metadata.get("protocols"));
        let protocols = protocols.and_then(Json::as_array).map(|names| {
            let names = names.iter().filter_map(Json::as_str);
            names.map(Box::from).collect()
        });
        Self {
            protocol: text("protocol"),
            protocols,
            json_version: text("jsonVersion"),
            target_prefix: text("targetPrefix"),
            signature_version: text("signatureVersion"),
            uid: text("uid"),
        }
    }
}

/// A shape or member name, as a patch names it.
pub type Name = Box<str>;

/// What a Smithy front-end changes in its reading to match botocore's,
/// checked by `schema-diff`. Declared now; C2J needs none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SchemaPatches {
    /// A structure's members in botocore's order.
    pub member_order: Vec<(Name, Vec<Name>)>,
    /// A structure's `required` list in botocore's order.
    pub required_order: Vec<(Name, Vec<Name>)>,
    /// A member's `jsonName` (by `Shape$member`), or its absence.
    pub json_names: Vec<(Name, Option<Name>)>,
    /// An error shape's `awsQueryError` code, or its absence.
    pub error_codes: Vec<(Name, Option<Name>)>,
}

/// The overlay of one service.
#[derive(Clone, Debug, PartialEq)]
pub struct BotocoreOverlay {
    /// The model directory, which is the CLI's service name.
    pub command_name: Box<str>,
    pub metadata: BotocoreMetadata,
    /// The paginators file, loaded with the service: one that cannot be
    /// read fails the load, which leaves the service to Python.
    pub paginators: Paginators,
    /// Whether the waiters file holds a waiter, once probed.
    has_waiters: OnceLock<bool>,
    pub patches: SchemaPatches,
}

impl BotocoreOverlay {
    /// The overlay of a model with no search path (`Model::from_json`):
    /// its metadata, no paginators and no waiters.
    pub fn detached(command_name: &str, metadata: BotocoreMetadata) -> Self {
        Self {
            command_name: command_name.into(),
            metadata,
            paginators: Paginators::default(),
            has_waiters: OnceLock::from(false),
            patches: SchemaPatches::default(),
        }
    }

    /// The overlay of `service` on `source`: `metadata` and the side files.
    ///
    /// # Errors
    /// The paginators file could not be read.
    pub fn load(
        source: &impl Source,
        service: &str,
        metadata: BotocoreMetadata,
    ) -> Result<Self, LoadError> {
        Ok(Self {
            command_name: service.into(),
            metadata,
            paginators: Paginators::load(source, service)?,
            has_waiters: OnceLock::new(),
            patches: SchemaPatches::default(),
        })
    }

    /// `get_waiter_model_from_service_model` and `waiter_names`: a waiter
    /// model the search of `source` finds, holding a waiter. Probed once;
    /// a detached overlay has none.
    pub fn has_waiters(&self, source: &impl Source) -> bool {
        *self.has_waiters.get_or_init(|| {
            let found = load_service_model(source, &self.command_name, Kind::Waiters);
            found.is_ok_and(|document| {
                let waiters = document.json.get("waiters").and_then(Json::as_object);
                waiters.is_some_and(|waiters| !waiters.is_empty())
            })
        })
    }
}

/// What protocols and signers read of the service: the overlay first, then
/// the service schema.
#[derive(Clone, Copy, Debug)]
pub struct ServiceFacts<'m> {
    overlay: &'m BotocoreOverlay,
    service: ServiceSchema<'m>,
}

impl<'m> ServiceFacts<'m> {
    pub const fn new(overlay: &'m BotocoreOverlay, service: ServiceSchema<'m>) -> Self {
        Self { overlay, service }
    }

    pub const fn metadata(self) -> &'m BotocoreMetadata {
        &self.overlay.metadata
    }

    pub fn command_name(self) -> &'m str {
        &self.overlay.command_name
    }

    /// `apiVersion`, which the awsQuery `Version=` sends.
    pub fn api_version(self) -> Option<&'m str> {
        self.service.version()
    }

    pub fn endpoint_prefix(self) -> Option<&'m str> {
        self.service.endpoint_prefix()
    }

    /// `ServiceId.hyphenize()`: `serviceId` lowercased, as Python's
    /// `str.lower` does, with each space a hyphen. botocore keys its
    /// per-call events on it.
    pub fn hyphenized_service_id(self) -> Option<String> {
        let id = self.service.sdk_id()?;
        Some(id.to_lowercase().replace(' ', "-"))
    }
}

#[cfg(test)]
mod tests;
