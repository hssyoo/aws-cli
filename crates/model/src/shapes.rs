//! One service model: the C2J front-end's schemas of a `service-2.json`
//! and the botocore overlay read beside them.
use crate::overlay::{BotocoreMetadata, BotocoreOverlay, ServiceFacts};
use crate::schema::projection::{self, Botocore};
use crate::schema::{BuildError, SchemaSet};
use crate::source::{Kind, LoadError, Source, load_service_model};

pub use crate::overlay::BotocoreMetadata as Metadata;
pub use crate::schema::{Site, Uninterpreted};

/// Why a service description is not one the model can hold. Each names
/// the exception botocore would raise once the part is used.
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
    /// More shapes than a schema id can number.
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

/// A schema builder's refusal, as the model's error for the same fault.
impl From<BuildError> for ModelError {
    fn from(error: BuildError) -> Self {
        match error {
            BuildError::NoShapeFound(name) => Self::NoShapeFound(name),
            BuildError::TooLarge => Self::TooLarge,
            other => Self::InvalidShape(other.to_string().into()),
        }
    }
}

/// The service directory of a model read from bytes, which names its
/// shape ids' namespace.
const UNNAMED: &str = "model";

/// A service model.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    schemas: SchemaSet,
    overlay: BotocoreOverlay,
}

impl Model {
    /// The service's latest `service-2` model from a source, extras
    /// merged, with its overlay.
    ///
    /// # Errors
    /// The model or its paginators file could not be read.
    pub fn load(source: &impl Source, service: &str) -> Result<Self, LoadError> {
        let document = load_service_model(source, service, Kind::Service)?;
        let namespace = crate::frontend::c2j::namespace(service);
        let schemas = crate::frontend::c2j::build(&document.json, &namespace)?;
        let metadata = BotocoreMetadata::from_c2j(&document.json);
        let overlay = BotocoreOverlay::load(source, service, metadata)?;
        Ok(Self { schemas, overlay })
    }

    /// A model from `service-2.json` bytes, as the protocol suite builds one
    /// per case.
    ///
    /// # Errors
    /// The bytes are not a model.
    pub fn from_json(bytes: &[u8]) -> Result<Self, ModelError> {
        let json: serde_json::Value = serde_json::from_slice(bytes).map_err(ModelError::Json)?;
        Self::from_value(&json)
    }

    /// A model from a parsed service description: no paginators and no
    /// waiters.
    ///
    /// # Errors
    /// The description is not a model.
    pub fn from_value(json: &serde_json::Value) -> Result<Self, ModelError> {
        let namespace = crate::frontend::c2j::namespace(UNNAMED);
        let schemas = crate::frontend::c2j::build(json, &namespace)?;
        let metadata = BotocoreMetadata::from_c2j(json);
        let overlay = BotocoreOverlay::detached(UNNAMED, metadata);
        Ok(Self { schemas, overlay })
    }

    /// The schemas the C2J front-end built.
    pub const fn schema(&self) -> &SchemaSet {
        &self.schemas
    }

    pub const fn overlay(&self) -> &BotocoreOverlay {
        &self.overlay
    }

    /// What protocols and signers read of the service.
    pub fn facts(&self) -> ServiceFacts<'_> {
        ServiceFacts::new(&self.overlay, self.schemas.service())
    }

    /// The overlay's botocore metadata.
    pub const fn metadata(&self) -> &Metadata {
        &self.overlay.metadata
    }

    /// [`projection::render`] of the schemas with the overlay's botocore
    /// metadata: the text the frozen projection digests hold.
    pub fn projection(&self) -> String {
        projection::render(&self.schemas, &Botocore::from(&self.overlay.metadata))
    }
}
