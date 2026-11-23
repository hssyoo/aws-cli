//! One service model in one arena: botocore's `ServiceModel`,
//! `OperationModel` and `Shape` objects, resolved once at load.
//!
//! Every shape reference resolves to a [`ShapeId`]. A reference that adds
//! traits gets its own shape: the target's kind with the reference's keys
//! laid over the target's, as `ShapeResolver.resolve_shape_ref` copies the
//! target's model and updates it. A reference whose keys change no trait
//! (only `documentation`, say) resolves to the target itself.
use crate::source::{Kind, LoadError, Source, load_service_model};
use crate::value::Value;

mod build;
mod traits;

pub use traits::{Bound, ErrorTrait, Flag, Retryable, Traits, XmlNamespace};

/// A shape in a [`Model`]'s arena.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct ShapeId(u32);

impl ShapeId {
    fn index(self) -> usize {
        self.0 as usize
    }
}

/// A structure's members: a range of the model's member table.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Members {
    start: u32,
    len: u32,
}

/// A shape's `type`, with its children for the compound ones.
#[derive(Clone, Debug, PartialEq)]
pub enum ShapeKind {
    Structure {
        members: Members,
    },
    List {
        member: ShapeId,
    },
    Map {
        key: ShapeId,
        value: ShapeId,
    },
    String,
    Integer,
    Long,
    Boolean,
    Float,
    Double,
    Timestamp,
    Blob,
    /// A type botocore has no class or validator for; botocore loads it and
    /// fails only when it is used.
    Other(Box<str>),
}

/// A shape: its model name (shared by the shapes references derive from
/// it), kind and resolved traits.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub name: Box<str>,
    pub kind: ShapeKind,
    pub traits: Traits,
}

/// A structure member, in model order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub name: Box<str>,
    pub shape: ShapeId,
}

/// The service's `metadata` keys a client reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Metadata {
    pub api_version: Option<Box<str>>,
    pub endpoint_prefix: Option<Box<str>>,
    pub protocol: Option<Box<str>>,
    pub protocols: Option<Box<[Box<str>]>>,
    pub json_version: Option<Box<str>>,
    pub target_prefix: Option<Box<str>>,
    pub service_id: Option<Box<str>>,
    pub service_full_name: Option<Box<str>>,
    pub service_abbreviation: Option<Box<str>>,
    pub signature_version: Option<Box<str>>,
    pub signing_name: Option<Box<str>>,
    pub uid: Option<Box<str>>,
    pub xml_namespace: Option<Box<str>>,
    pub global_endpoint: Option<Box<str>>,
    pub checksum_format: Option<Box<str>>,
    pub auth: Option<Box<[Box<str>]>>,
    /// `ServiceModel.is_query_compatible`: the key is present.
    pub aws_query_compatible: bool,
}

/// One of `ServiceModel.client_context_parameters`.
impl Metadata {
    /// `ServiceId.hyphenize()`: `serviceId` lowercased, as Python's
    /// `str.lower` does, with each space a hyphen. botocore keys its
    /// per-call events on it.
    pub fn hyphenized_service_id(&self) -> Option<String> {
        self.service_id
            .as_deref()
            .map(|id| id.to_lowercase().replace(' ', "-"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientContextParam {
    pub name: Box<str>,
    pub type_name: Box<str>,
}

/// An operation's `http` object.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Http {
    pub method: Option<Box<str>>,
    pub request_uri: Option<Box<str>>,
    pub response_code: Option<u16>,
}

/// An operation's `endpointdiscovery` object.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EndpointDiscovery {
    pub required: bool,
}

/// An operation's `httpChecksum` object.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HttpChecksum {
    pub request_algorithm_member: Option<Box<str>>,
    pub request_checksum_required: bool,
    pub request_validation_mode_member: Option<Box<str>>,
    pub response_algorithms: Box<[Box<str>]>,
}

/// `staticContextParams`: each name with its `value`.
pub type StaticContextParams = Box<[(Box<str>, Value)]>;
/// `operationContextParams`: each name with its `path`.
pub type OperationContextParams = Box<[(Box<str>, Box<str>)]>;

/// An operation, as `OperationModel` reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct Operation {
    /// The key in `operations`, the name callers use.
    pub name: Box<str>,
    /// The `name` field, sent on the wire (`OperationModel.wire_name`).
    pub wire_name: Option<Box<str>>,
    pub http: Http,
    pub input: Option<ShapeId>,
    pub output: Option<ShapeId>,
    pub errors: Box<[ShapeId]>,
    pub deprecated: bool,
    pub endpoint_discovery: Option<EndpointDiscovery>,
    /// `endpointoperation`.
    pub endpoint_operation: bool,
    pub auth: Option<Box<[Box<str>]>>,
    /// `authtype`.
    pub auth_type: Option<Box<str>>,
    pub unsigned_payload: bool,
    pub http_checksum_required: bool,
    pub http_checksum: Option<HttpChecksum>,
    /// `requestcompression.encodings`.
    pub request_compression: Option<Box<[Box<str>]>>,
    /// `staticContextParams`: each name with its `value` (`None` when absent).
    pub static_context_params: StaticContextParams,
    /// `operationContextParams`: each name with its `path`.
    pub operation_context_params: OperationContextParams,
    /// `endpoint.hostPrefix`.
    pub host_prefix: Option<Box<str>>,
}

/// Why a service description is not one the arena can hold. Each names the
/// exception botocore would raise once the part is used.
#[derive(Debug)]
#[non_exhaustive]
pub enum ModelError {
    Json(serde_json::Error),
    /// The document is not a JSON object.
    NotAnObject,
    /// `NoShapeFoundError`: a reference names no shape.
    NoShapeFound(Box<str>),
    /// `InvalidShapeError`: the shape has no `type`, or a compound shape
    /// lacks its `members`, `member`, `key` or `value`.
    InvalidShape(Box<str>),
    /// `InvalidShapeReferenceError`: a reference without `shape`, in the
    /// named shape or operation.
    InvalidShapeReference(Box<str>),
    /// A trait whose value has the wrong JSON type.
    InvalidTrait {
        shape: Box<str>,
        name: &'static str,
    },
    /// More shapes than a `ShapeId` can number.
    TooLarge,
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => write!(f, "invalid model JSON: {error}"),
            Self::NotAnObject => f.write_str("the model is not a JSON object"),
            Self::NoShapeFound(name) => write!(f, "Shape not found: {name}"),
            Self::InvalidShape(name) => write!(f, "invalid shape: {name}"),
            Self::InvalidShapeReference(name) => write!(f, "invalid shape reference in {name}"),
            Self::InvalidTrait { shape, name } => write!(f, "invalid {name} in {shape}"),
            Self::TooLarge => f.write_str("the model has too many shapes"),
        }
    }
}

impl std::error::Error for ModelError {}

/// A service model.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    metadata: Metadata,
    client_context_params: Box<[ClientContextParam]>,
    operations: Box<[Operation]>,
    shapes: Box<[Shape]>,
    members: Box<[Member]>,
    /// Named shapes come first, in model order.
    named: usize,
    /// Named shape ids sorted by name.
    by_name: Box<[ShapeId]>,
}

impl Model {
    /// The service's latest `service-2` model from a source, extras merged.
    pub fn load(source: &impl Source, service: &str) -> Result<Self, LoadError> {
        let document = load_service_model(source, service, Kind::Service)?;
        Ok(Self::from_value(&document.json)?)
    }

    /// A model from `service-2.json` bytes, as the protocol suite builds one
    /// per case.
    pub fn from_json(bytes: &[u8]) -> Result<Self, ModelError> {
        let json: serde_json::Value = serde_json::from_slice(bytes).map_err(ModelError::Json)?;
        Self::from_value(&json)
    }

    /// A model from a parsed service description.
    pub fn from_value(json: &serde_json::Value) -> Result<Self, ModelError> {
        build::build(json)
    }

    pub const fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    pub fn client_context_params(&self) -> &[ClientContextParam] {
        &self.client_context_params
    }

    /// The operations in model order (`ServiceModel.operation_names`).
    pub fn operations(&self) -> &[Operation] {
        &self.operations
    }

    pub fn operation(&self, name: &str) -> Option<&Operation> {
        self.operations
            .iter()
            .find(|operation| &*operation.name == name)
    }

    pub fn shape(&self, id: ShapeId) -> &Shape {
        &self.shapes[id.index()]
    }

    /// The number of named shapes (`len(ServiceModel.shape_names)`).
    pub const fn shape_count(&self) -> usize {
        self.named
    }

    /// `ServiceModel.shape_for(name)` without member traits.
    pub fn shape_named(&self, name: &str) -> Option<ShapeId> {
        let found = self
            .by_name
            .binary_search_by(|id| (*self.shape(*id).name).cmp(name))
            .ok()?;
        Some(self.by_name[found])
    }

    /// A structure's members in model order; empty for any other kind.
    pub fn members(&self, id: ShapeId) -> &[Member] {
        match self.shape(id).kind {
            ShapeKind::Structure { members } => {
                let start = members.start as usize;
                &self.members[start..start + members.len as usize]
            }
            _ => &[],
        }
    }

    /// `ServiceModel.error_shapes`: named shapes marked `exception`, in
    /// model order.
    pub fn error_shapes(&self) -> impl Iterator<Item = ShapeId> + '_ {
        (0..self.named)
            .map(build::id)
            .filter(|id| self.shape(*id).traits.is(Flag::Exception))
    }

    /// `StructureShape.error_code`: `error.code`, else the shape's name, for
    /// an exception shape.
    pub fn error_code(&self, id: ShapeId) -> Option<&str> {
        let shape = self.shape(id);
        if !shape.traits.is(Flag::Exception) {
            return None;
        }
        let code = shape
            .traits
            .error
            .as_ref()
            .and_then(|error| error.code.as_deref());
        Some(code.filter(|code| !code.is_empty()).unwrap_or(&shape.name))
    }

    /// `ServiceModel.shape_for_error_code`: the last error shape with the code.
    pub fn shape_for_error_code(&self, code: &str) -> Option<ShapeId> {
        self.error_shapes()
            .filter(|id| self.error_code(*id) == Some(code))
            .last()
    }
}

#[cfg(test)]
mod tests;
