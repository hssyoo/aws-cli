//! A shape's traits: the keys of botocore's `Shape.SERIALIZED_ATTRS` and
//! `Shape.METADATA_ATTRS` (with `locationName` read as `name`, as
//! `Shape.serialization` renames it), plus `deprecated`.
use super::ModelError;
use crate::pyrepr::float_repr;
use serde_json::Value as Json;

/// A boolean trait. Each is set when its key holds a value Python finds
/// true, as botocore's `serialization.get(...)` and `metadata.get(...)`
/// tests read them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Flag {
    Flattened = 1 << 0,
    Streaming = 1 << 1,
    XmlAttribute = 1 << 2,
    EventStream = 1 << 3,
    Event = 1 << 4,
    EventHeader = 1 << 5,
    EventPayload = 1 << 6,
    JsonValue = 1 << 7,
    HostLabel = 1 << 8,
    Sensitive = 1 << 9,
    IdempotencyToken = 1 << 10,
    Exception = 1 << 11,
    EndpointDiscoveryId = 1 << 12,
    Document = 1 << 13,
    Union = 1 << 14,
    RequiresLength = 1 << 15,
    Deprecated = 1 << 16,
}

/// A `min` or `max`, as the number the model wrote: `str()` prints an
/// integer bound without a fraction and a float one with Python's `repr`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Bound {
    Integer(i64),
    Float(f64),
}

impl std::fmt::Display for Bound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Integer(value) => value.fmt(f),
            Self::Float(value) => f.write_str(&float_repr(*value)),
        }
    }
}

/// The `xmlNamespace` object of a shape or member.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct XmlNamespace {
    pub uri: Option<Box<str>>,
    pub prefix: Option<Box<str>>,
}

/// An error shape's `error` object.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ErrorTrait {
    pub code: Option<Box<str>>,
    pub http_status_code: Option<u16>,
    pub sender_fault: bool,
}

/// An error shape's `retryable` object.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Retryable {
    pub throttling: bool,
}

/// The resolved traits of a shape as one reference sees it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Traits {
    flags: u32,
    /// `locationName`.
    pub name: Option<Box<str>>,
    pub query_name: Option<Box<str>>,
    pub location: Option<Box<str>>,
    pub payload: Option<Box<str>>,
    pub timestamp_format: Option<Box<str>>,
    pub xml_namespace: Option<XmlNamespace>,
    pub result_wrapper: Option<Box<str>>,
    pub required: Box<[Box<str>]>,
    pub min: Option<Bound>,
    pub max: Option<Bound>,
    pub enum_values: Box<[Box<str>]>,
    pub error: Option<ErrorTrait>,
    pub retryable: Option<Retryable>,
    /// `contextParam.name`, read only when present, as
    /// `OperationModel.context_parameters` reads it.
    pub context_param: Option<Box<str>>,
    pub pattern: Option<Box<str>>,
}

const FLAG_KEYS: [(&str, Flag); 17] = [
    ("flattened", Flag::Flattened),
    ("streaming", Flag::Streaming),
    ("xmlAttribute", Flag::XmlAttribute),
    ("eventstream", Flag::EventStream),
    ("event", Flag::Event),
    ("eventheader", Flag::EventHeader),
    ("eventpayload", Flag::EventPayload),
    ("jsonvalue", Flag::JsonValue),
    ("hostLabel", Flag::HostLabel),
    ("sensitive", Flag::Sensitive),
    ("idempotencyToken", Flag::IdempotencyToken),
    ("exception", Flag::Exception),
    ("endpointdiscoveryid", Flag::EndpointDiscoveryId),
    ("document", Flag::Document),
    ("union", Flag::Union),
    ("requiresLength", Flag::RequiresLength),
    ("deprecated", Flag::Deprecated),
];

impl Traits {
    pub const fn is(&self, flag: Flag) -> bool {
        self.flags & flag as u32 != 0
    }

    fn set(&mut self, flag: Flag, on: bool) {
        if on {
            self.flags |= flag as u32;
        } else {
            self.flags &= !(flag as u32);
        }
    }

    /// Applies every trait key present in `object` over the traits already
    /// held, as `ShapeResolver.get_shape_by_name` updates the target's model
    /// with a reference's keys.
    pub(super) fn apply(
        &mut self,
        object: &serde_json::Map<String, Json>,
        shape: &str,
    ) -> Result<(), ModelError> {
        for (key, flag) in FLAG_KEYS {
            if let Some(value) = object.get(key) {
                self.set(flag, truthy(value));
            }
        }
        let read = Reader { object, shape };
        read.text("locationName", &mut self.name)?;
        read.text("queryName", &mut self.query_name)?;
        read.text("location", &mut self.location)?;
        read.text("payload", &mut self.payload)?;
        read.text("timestampFormat", &mut self.timestamp_format)?;
        read.text("resultWrapper", &mut self.result_wrapper)?;
        read.text("pattern", &mut self.pattern)?;
        read.texts("required", &mut self.required)?;
        read.texts("enum", &mut self.enum_values)?;
        read.bound("min", &mut self.min)?;
        read.bound("max", &mut self.max)?;
        read.namespace(&mut self.xml_namespace)?;
        read.error(&mut self.error)?;
        read.retryable(&mut self.retryable)?;
        read.context_param(&mut self.context_param)
    }
}

/// Python truthiness of a JSON value.
pub(super) fn truthy(value: &Json) -> bool {
    match value {
        Json::Null => false,
        Json::Bool(value) => *value,
        Json::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Json::String(text) => !text.is_empty(),
        Json::Array(items) => !items.is_empty(),
        Json::Object(map) => !map.is_empty(),
    }
}

struct Reader<'a> {
    object: &'a serde_json::Map<String, Json>,
    shape: &'a str,
}

impl Reader<'_> {
    fn invalid(&self, name: &'static str) -> ModelError {
        ModelError::InvalidTrait {
            shape: self.shape.into(),
            name,
        }
    }

    fn text(&self, key: &'static str, slot: &mut Option<Box<str>>) -> Result<(), ModelError> {
        if let Some(value) = self.object.get(key) {
            *slot = Some(value.as_str().ok_or_else(|| self.invalid(key))?.into());
        }
        Ok(())
    }

    fn texts(&self, key: &'static str, slot: &mut Box<[Box<str>]>) -> Result<(), ModelError> {
        if let Some(value) = self.object.get(key) {
            *slot = strings(value).ok_or_else(|| self.invalid(key))?;
        }
        Ok(())
    }

    fn bound(&self, key: &'static str, slot: &mut Option<Bound>) -> Result<(), ModelError> {
        let Some(value) = self.object.get(key) else {
            return Ok(());
        };
        let number = value.as_number().ok_or_else(|| self.invalid(key))?;
        let bound = match number.as_i64() {
            Some(integer) if !number.as_str().contains(['.', 'e', 'E']) => Bound::Integer(integer),
            _ => Bound::Float(number.as_f64().ok_or_else(|| self.invalid(key))?),
        };
        *slot = Some(bound);
        Ok(())
    }

    fn object(
        &self,
        key: &'static str,
    ) -> Result<Option<&serde_json::Map<String, Json>>, ModelError> {
        self.object
            .get(key)
            .map(|value| value.as_object().ok_or_else(|| self.invalid(key)))
            .transpose()
    }

    fn namespace(&self, slot: &mut Option<XmlNamespace>) -> Result<(), ModelError> {
        if let Some(object) = self.object("xmlNamespace")? {
            let field = |name| object.get(name).and_then(Json::as_str).map(Box::from);
            *slot = Some(XmlNamespace {
                uri: field("uri"),
                prefix: field("prefix"),
            });
        }
        Ok(())
    }

    fn error(&self, slot: &mut Option<ErrorTrait>) -> Result<(), ModelError> {
        if let Some(object) = self.object("error")? {
            let status = object.get("httpStatusCode").and_then(Json::as_u64);
            *slot = Some(ErrorTrait {
                code: object.get("code").and_then(Json::as_str).map(Box::from),
                http_status_code: status.and_then(|code| u16::try_from(code).ok()),
                sender_fault: object.get("senderFault").is_some_and(truthy),
            });
        }
        Ok(())
    }

    fn retryable(&self, slot: &mut Option<Retryable>) -> Result<(), ModelError> {
        if let Some(object) = self.object("retryable")? {
            *slot = Some(Retryable {
                throttling: object.get("throttling").is_some_and(truthy),
            });
        }
        Ok(())
    }

    fn context_param(&self, slot: &mut Option<Box<str>>) -> Result<(), ModelError> {
        if let Some(object) = self.object("contextParam")? {
            *slot = object.get("name").and_then(Json::as_str).map(Box::from);
        }
        Ok(())
    }
}

/// A JSON list of strings.
pub(super) fn strings(value: &Json) -> Option<Box<[Box<str>]>> {
    value
        .as_array()?
        .iter()
        .map(|item| item.as_str().map(Box::from))
        .collect()
}
