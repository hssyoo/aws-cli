//! A shape's traits: the keys of botocore's `Shape.SERIALIZED_ATTRS` and
//! `Shape.METADATA_ATTRS` (with `locationName` read as `name`, as
//! `Shape.serialization` renames it), plus `deprecated`.
use super::ModelError;
use crate::pyrepr::float_repr;
use serde_json::Value as Json;
use std::sync::Arc;

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

/// The resolved traits of a shape as one reference sees it. The lists are
/// shared, so a reference that overlays a few keys copies little.
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
    pub required: Arc<[Box<str>]>,
    pub min: Option<Bound>,
    pub max: Option<Bound>,
    pub enum_values: Arc<[Box<str>]>,
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
    /// with a reference's keys. Other keys are ignored.
    pub(super) fn apply(
        &mut self,
        object: &serde_json::Map<String, Json>,
        shape: &str,
    ) -> Result<(), ModelError> {
        for (key, value) in object {
            self.apply_key(key, value)
                .map_err(|name| ModelError::InvalidTrait {
                    shape: shape.into(),
                    name,
                })?;
        }
        Ok(())
    }

    /// One key; `Err` names a trait whose value has the wrong JSON type.
    fn apply_key(&mut self, key: &str, value: &Json) -> Result<(), &'static str> {
        if let Some((_, flag)) = FLAG_KEYS.iter().find(|(name, _)| *name == key) {
            self.set(*flag, truthy(value));
            return Ok(());
        }
        match key {
            "locationName" => self.name = Some(text(value, "locationName")?),
            "queryName" => self.query_name = Some(text(value, "queryName")?),
            "location" => self.location = Some(text(value, "location")?),
            "payload" => self.payload = Some(text(value, "payload")?),
            "timestampFormat" => self.timestamp_format = Some(text(value, "timestampFormat")?),
            "resultWrapper" => self.result_wrapper = Some(text(value, "resultWrapper")?),
            "pattern" => self.pattern = Some(text(value, "pattern")?),
            "required" => self.required = strings(value).ok_or("required")?.into(),
            "enum" => self.enum_values = strings(value).ok_or("enum")?.into(),
            "min" => self.min = Some(bound(value).ok_or("min")?),
            "max" => self.max = Some(bound(value).ok_or("max")?),
            "xmlNamespace" => self.xml_namespace = Some(namespace(value)?),
            "error" => self.error = Some(error(object(value, "error")?)),
            "retryable" => self.retryable = Some(retryable(object(value, "retryable")?)),
            "contextParam" => self.context_param = context_param(object(value, "contextParam")?),
            _ => {}
        }
        Ok(())
    }
}

/// Whether a reference holds any key [`Traits::apply`] reads.
pub(super) fn has_trait_keys(object: &serde_json::Map<String, Json>) -> bool {
    object.keys().any(|key| {
        FLAG_KEYS.iter().any(|(name, _)| name == key) || TEXT_KEYS.contains(&key.as_str())
    })
}

const TEXT_KEYS: [&str; 15] = [
    "locationName",
    "queryName",
    "location",
    "payload",
    "timestampFormat",
    "resultWrapper",
    "pattern",
    "required",
    "enum",
    "min",
    "max",
    "xmlNamespace",
    "error",
    "retryable",
    "contextParam",
];

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

fn text(value: &Json, key: &'static str) -> Result<Box<str>, &'static str> {
    value.as_str().map(Box::from).ok_or(key)
}

fn object<'a>(
    value: &'a Json,
    key: &'static str,
) -> Result<&'a serde_json::Map<String, Json>, &'static str> {
    value.as_object().ok_or(key)
}

/// A number as the model wrote it: integer text stays an integer.
fn bound(value: &Json) -> Option<Bound> {
    let number = value.as_number()?;
    match number.as_i64() {
        Some(integer) if !number.as_str().contains(['.', 'e', 'E']) => {
            Some(Bound::Integer(integer))
        }
        _ => number.as_f64().map(Bound::Float),
    }
}

/// An `xmlNamespace`: `{"uri", "prefix"}`, or the URI alone as text, as the
/// protocol suite's models also write it.
fn namespace(value: &Json) -> Result<XmlNamespace, &'static str> {
    if let Some(uri) = value.as_str() {
        return Ok(XmlNamespace {
            uri: Some(uri.into()),
            prefix: None,
        });
    }
    let object = object(value, "xmlNamespace")?;
    let field = |name| object.get(name).and_then(Json::as_str).map(Box::from);
    Ok(XmlNamespace {
        uri: field("uri"),
        prefix: field("prefix"),
    })
}

fn error(object: &serde_json::Map<String, Json>) -> ErrorTrait {
    let status = object.get("httpStatusCode").and_then(Json::as_u64);
    ErrorTrait {
        code: object.get("code").and_then(Json::as_str).map(Box::from),
        http_status_code: status.and_then(|code| u16::try_from(code).ok()),
        sender_fault: object.get("senderFault").is_some_and(truthy),
    }
}

fn retryable(object: &serde_json::Map<String, Json>) -> Retryable {
    Retryable {
        throttling: object.get("throttling").is_some_and(truthy),
    }
}

fn context_param(object: &serde_json::Map<String, Json>) -> Option<Box<str>> {
    object.get("name").and_then(Json::as_str).map(Box::from)
}

/// A JSON list of strings.
pub(super) fn strings(value: &Json) -> Option<Box<[Box<str>]>> {
    value
        .as_array()?
        .iter()
        .map(|item| item.as_str().map(Box::from))
        .collect()
}
