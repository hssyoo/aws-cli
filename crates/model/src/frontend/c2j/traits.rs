//! C2J trait keys to [`KnownTraits`] slots and `c2j#` cold traits, with
//! botocore's meaning: a reference's keys are laid over its target's
//! before either is read, as `ShapeResolver.resolve_shape_ref` updates the
//! target's model with them, so a falsy flag clears the target's and a
//! `location` combines with the target's `locationName`.
use crate::schema::{
    Bound, BuildError, ColdTraits, ErrorInfo, Flag, KnownTraits, Retryable, SchemaSetBuilder,
    ShapeType, XmlNamespace, traits,
};
use crate::shapes::ModelError;
use serde_json::{Map, Value as Json};
use std::sync::Arc;

type Object = Map<String, Json>;

/// The flag keys with their [`Flag`]. `event`, `document` and `union` have
/// no slot: [`C2jTraits`] keeps their values for the cold map and the type.
const FLAG_KEYS: [(&str, Flag); 14] = [
    ("flattened", Flag::XmlFlattened),
    ("streaming", Flag::Streaming),
    ("xmlAttribute", Flag::XmlAttribute),
    ("eventstream", Flag::EventStream),
    ("eventheader", Flag::EventHeader),
    ("eventpayload", Flag::EventPayload),
    ("jsonvalue", Flag::JsonValue),
    ("hostLabel", Flag::HostLabel),
    ("sensitive", Flag::Sensitive),
    ("idempotencyToken", Flag::IdempotencyToken),
    ("exception", Flag::Error),
    ("endpointdiscoveryid", Flag::EndpointDiscoveryId),
    ("requiresLength", Flag::RequiresLength),
    ("deprecated", Flag::Deprecated),
];

/// The C2J trait keys of a shape or reference, as `match` runs them for
/// every key of every reference at no measurable cost (the tests keep it
/// and [`C2jTraits::apply`]'s keys in step).
pub(super) fn is_trait_key(key: &str) -> bool {
    matches!(
        key,
        "flattened"
            | "streaming"
            | "xmlAttribute"
            | "eventstream"
            | "event"
            | "eventheader"
            | "eventpayload"
            | "jsonvalue"
            | "hostLabel"
            | "sensitive"
            | "idempotencyToken"
            | "exception"
            | "endpointdiscoveryid"
            | "document"
            | "union"
            | "requiresLength"
            | "deprecated"
            | "locationName"
            | "queryName"
            | "location"
            | "payload"
            | "timestampFormat"
            | "resultWrapper"
            | "pattern"
            | "required"
            | "enum"
            | "min"
            | "max"
            | "xmlNamespace"
            | "error"
            | "retryable"
            | "contextParam"
    )
}

/// The keys read inside a trait whose value is an object; `None` for the
/// others.
pub(super) fn trait_subkeys(key: &str) -> Option<&'static [&'static str]> {
    match key {
        "error" => Some(&["code", "httpStatusCode", "senderFault"]),
        "retryable" => Some(&["throttling"]),
        "xmlNamespace" => Some(&["uri", "prefix"]),
        "contextParam" => Some(&["name"]),
        _ => None,
    }
}

/// The resolved trait keys of a shape as one reference sees it: the slots,
/// and the raw values of the keys with no slot.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct C2jTraits {
    pub(super) known: KnownTraits,
    pub(super) event: Option<Json>,
    pub(super) document: Option<Json>,
    pub(super) union: Option<Json>,
}

impl C2jTraits {
    /// Lays every trait key of `object` over the traits held; `shape`
    /// names the shape in an error, as the arena names it.
    pub(super) fn apply(
        &mut self,
        object: &Object,
        shape: &str,
        builder: &mut SchemaSetBuilder,
    ) -> Result<(), ModelError> {
        for (key, value) in object {
            self.apply_key(key, value, builder)
                .map_err(|name| match name {
                    Invalid::Key(name) => ModelError::InvalidTrait {
                        shape: shape.into(),
                        name,
                    },
                    Invalid::Build(error) => error.into(),
                })?;
        }
        Ok(())
    }

    fn apply_key(
        &mut self,
        key: &str,
        value: &Json,
        builder: &mut SchemaSetBuilder,
    ) -> Result<(), Invalid> {
        if let Some((_, flag)) = FLAG_KEYS.iter().find(|(name, _)| *name == key) {
            self.known.flags.set(*flag, truthy(value));
            return Ok(());
        }
        let known = &mut self.known;
        let mut text = |key: &'static str| -> Result<_, Invalid> {
            let text = value.as_str().ok_or(Invalid::Key(key))?;
            Ok(Some(builder.intern(text)?))
        };
        match key {
            "event" => self.event = Some(value.clone()),
            "document" => self.document = Some(value.clone()),
            "union" => self.union = Some(value.clone()),
            "locationName" => {
                let name = text("locationName")?;
                (known.location_name, known.json_name, known.xml_name) = (name, name, name);
            }
            "queryName" => known.ec2_query_name = text("queryName")?,
            "location" => known.location = text("location")?,
            "payload" => known.payload = text("payload")?,
            "timestampFormat" => known.timestamp_format = text("timestampFormat")?,
            "resultWrapper" => known.result_wrapper = text("resultWrapper")?,
            "pattern" => known.pattern = text("pattern")?,
            "required" => known.required = Some(strings(value, "required", builder)?),
            "enum" => known.enum_values = Some(strings(value, "enum", builder)?),
            "min" => known.min = Some(bound(value).ok_or(Invalid::Key("min"))?),
            "max" => known.max = Some(bound(value).ok_or(Invalid::Key("max"))?),
            "xmlNamespace" => known.xml_namespace = Some(namespace(value, builder)?),
            "error" => known.error = Some(error(object(value, "error")?, builder)?),
            "retryable" => known.retryable = Some(retryable(object(value, "retryable")?)),
            "contextParam" => {
                let name = object(value, "contextParam")?.get("name");
                known.context_param = optional_text(name, builder)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// The type a named shape of botocore type `type_name` takes: a
    /// `structure` with a truthy `document` is a document, else with a
    /// truthy `union` a union.
    pub(super) fn shape_type(&self, type_name: &str) -> ShapeType {
        let truthy = |value: &Option<Json>| value.as_ref().is_some_and(truthy);
        match type_name {
            "structure" if truthy(&self.document) => ShapeType::Document,
            "structure" if truthy(&self.union) => ShapeType::Union,
            "structure" => ShapeType::Structure,
            "list" => ShapeType::List,
            "map" => ShapeType::Map,
            "string" => ShapeType::String,
            "integer" => ShapeType::Integer,
            "long" => ShapeType::Long,
            "boolean" => ShapeType::Boolean,
            "float" => ShapeType::Float,
            "double" => ShapeType::Double,
            "timestamp" => ShapeType::Timestamp,
            "blob" => ShapeType::Blob,
            _ => ShapeType::Unknown,
        }
    }

    /// The cold traits of a schema of `shape_type`: `event` as written, and
    /// a `document` or `union` whose truthiness the type does not say.
    pub(super) fn cold(&self, shape_type: ShapeType) -> Result<ColdTraits, BuildError> {
        let mut cold = ColdTraits::default();
        if let Some(event) = &self.event {
            cold.insert(traits::EVENT.as_str(), event.clone())?;
        }
        let unsaid = [
            (traits::DOCUMENT, &self.document, ShapeType::Document),
            (traits::UNION, &self.union, ShapeType::Union),
        ];
        for (id, value, expressed) in unsaid {
            if let Some(value) = value.as_ref()
                && truthy(value) != (shape_type == expressed)
            {
                cold.insert(id.as_str(), value.clone())?;
            }
        }
        Ok(cold)
    }

    /// The keys of `document` and `union` whose truthiness differs from
    /// `target`'s: a reference that changes them changes what botocore
    /// reads of a structure, which a member's type cannot say.
    pub(super) fn changed_kind(&self, target: &Self) -> impl Iterator<Item = &'static str> {
        let truthy = |value: &Option<Json>| value.as_ref().is_some_and(truthy);
        [
            (
                "document",
                truthy(&self.document) != truthy(&target.document),
            ),
            ("union", truthy(&self.union) != truthy(&target.union)),
        ]
        .into_iter()
        .filter_map(|(key, changed)| changed.then_some(key))
    }
}

/// Why a key's value cannot be read.
enum Invalid {
    /// The value has the wrong JSON type.
    Key(&'static str),
    Build(BuildError),
}

impl From<BuildError> for Invalid {
    fn from(error: BuildError) -> Self {
        Self::Build(error)
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

fn object<'a>(value: &'a Json, key: &'static str) -> Result<&'a Object, Invalid> {
    value.as_object().ok_or(Invalid::Key(key))
}

fn optional_text(
    value: Option<&Json>,
    builder: &mut SchemaSetBuilder,
) -> Result<Option<crate::schema::StrId>, Invalid> {
    match value.and_then(Json::as_str) {
        Some(text) => Ok(Some(builder.intern(text)?)),
        None => Ok(None),
    }
}

fn strings(
    value: &Json,
    key: &'static str,
    builder: &mut SchemaSetBuilder,
) -> Result<Arc<[crate::schema::StrId]>, Invalid> {
    let items = value.as_array().ok_or(Invalid::Key(key))?;
    let mut ids = Vec::with_capacity(items.len());
    for item in items {
        ids.push(builder.intern(item.as_str().ok_or(Invalid::Key(key))?)?);
    }
    Ok(ids.into())
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

/// An `xmlNamespace`: `{"uri", "prefix"}`, or the URI alone as text.
fn namespace(value: &Json, builder: &mut SchemaSetBuilder) -> Result<XmlNamespace, Invalid> {
    if let Some(uri) = value.as_str() {
        return Ok(XmlNamespace {
            uri: Some(builder.intern(uri)?),
            prefix: None,
        });
    }
    let object = object(value, "xmlNamespace")?;
    Ok(XmlNamespace {
        uri: optional_text(object.get("uri"), builder)?,
        prefix: optional_text(object.get("prefix"), builder)?,
    })
}

fn error(object: &Object, builder: &mut SchemaSetBuilder) -> Result<ErrorInfo, Invalid> {
    let status = object.get("httpStatusCode").and_then(Json::as_u64);
    Ok(ErrorInfo {
        code: optional_text(object.get("code"), builder)?,
        http_status: status.and_then(|code| u16::try_from(code).ok()),
        sender_fault: object.get("senderFault").is_some_and(truthy),
    })
}

fn retryable(object: &Object) -> Retryable {
    Retryable {
        throttling: object.get("throttling").is_some_and(truthy),
    }
}

/// Every key [`C2jTraits::apply`] reads.
#[cfg(test)]
pub(super) const TRAIT_KEYS: [&str; 32] = [
    "flattened",
    "streaming",
    "xmlAttribute",
    "eventstream",
    "event",
    "eventheader",
    "eventpayload",
    "jsonvalue",
    "hostLabel",
    "sensitive",
    "idempotencyToken",
    "exception",
    "endpointdiscoveryid",
    "document",
    "union",
    "requiresLength",
    "deprecated",
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
