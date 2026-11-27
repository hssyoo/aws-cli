//! The projection: a service's schemas as text, one line per service,
//! shape, member, operation, operation root and uninterpreted key, with
//! every slot, flag, cold trait and operation read in botocore's terms.
//! Two readings of one description are the same exactly when their
//! projections are, which the C2J front-end's arena comparison, the frozen
//! digests and a later `schema-diff` use.
//!
//! The lines are built from views ([`ServiceView`], [`OperationView`],
//! [`TraitsView`]) any reader of a description can fill; [`render`] fills
//! them from a [`SchemaSet`]. Texts are quoted (`{:?}`), absent values are
//! `-`, and lists keep their order.
use super::{
    Flag, OperationSchema, Schema, SchemaSet, ShapeType, Site, TraitId, Uninterpreted, traits,
};
use crate::pyjson::{Options, dumps};
use crate::value::Value;
use std::fmt::Write as _;

/// The flags by their C2J key, in the order a line lists them.
pub const FLAG_NAMES: [&str; 17] = [
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
];

/// The slot flags by their C2J key.
const SLOT_FLAGS: [(&str, Flag); 14] = [
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

/// What botocore reads of `metadata` beside the schema: the overlay's.
#[derive(Clone, Copy, Debug, Default)]
pub struct Botocore<'a> {
    pub protocol: Option<&'a str>,
    pub protocols: Option<&'a [Box<str>]>,
    pub json_version: Option<&'a str>,
    pub target_prefix: Option<&'a str>,
    pub signature_version: Option<&'a str>,
    pub uid: Option<&'a str>,
}

/// The service's reads.
#[derive(Clone, Debug, Default)]
pub struct ServiceView<'a> {
    pub version: Option<&'a str>,
    pub sdk_id: Option<&'a str>,
    pub endpoint_prefix: Option<&'a str>,
    pub sigv4_name: Option<&'a str>,
    pub auth: Option<Vec<&'a str>>,
    pub xml_namespace: Option<&'a str>,
    pub query_compatible: bool,
    pub protocol_traits: Vec<&'a str>,
    /// Each name with its type.
    pub client_context_params: Vec<(&'a str, &'a str)>,
    pub botocore: Botocore<'a>,
}

/// An `httpChecksum`: `requestAlgorithmMember`, `requestChecksumRequired`,
/// `requestValidationModeMember`, `responseAlgorithms`.
pub type ChecksumView<'a> = (Option<&'a str>, bool, Option<&'a str>, Vec<&'a str>);

/// An operation's own reads; its roots are lines of their own.
#[derive(Clone, Debug, Default)]
pub struct OperationView<'a> {
    pub name: &'a str,
    pub wire_name: Option<&'a str>,
    pub http_method: Option<&'a str>,
    pub http_request_uri: Option<&'a str>,
    pub host_prefix: Option<&'a str>,
    pub auth: Option<Vec<&'a str>>,
    pub auth_type: Option<&'a str>,
    pub unsigned_payload: bool,
    pub http_checksum_required: bool,
    pub endpoint_operation: bool,
    pub http_checksum: Option<ChecksumView<'a>>,
    /// `endpointdiscovery.required`.
    pub endpoint_discovery: Option<bool>,
    pub request_compression: Option<Vec<&'a str>>,
    pub static_context_params: Vec<(&'a str, &'a Value)>,
    pub operation_context_params: Vec<(&'a str, &'a str)>,
}

/// A schema's effective traits.
#[derive(Clone, Debug, Default)]
pub struct TraitsView<'a> {
    /// Flag names of [`FLAG_NAMES`], in any order.
    pub flags: Vec<&'static str>,
    pub location: Option<&'a str>,
    pub location_name: Option<&'a str>,
    pub json_name: Option<&'a str>,
    pub xml_name: Option<&'a str>,
    pub ec2_query_name: Option<&'a str>,
    pub payload: Option<&'a str>,
    pub timestamp_format: Option<&'a str>,
    pub xml_namespace: Option<(Option<&'a str>, Option<&'a str>)>,
    /// Empty reads as absent, as botocore's `get('required', [])`.
    pub required: Vec<&'a str>,
    pub enum_values: Vec<&'a str>,
    /// A bound as written: `i<integer>` or `f<float repr>`.
    pub min: Option<String>,
    pub max: Option<String>,
    pub pattern: Option<&'a str>,
    /// `code`, `httpStatusCode`, `senderFault`.
    pub error: Option<(Option<&'a str>, Option<u16>, bool)>,
    /// `throttling`.
    pub retryable: Option<bool>,
    pub context_param: Option<&'a str>,
    pub result_wrapper: Option<&'a str>,
    /// Cold traits other than C2J's flag residues, by id with their JSON.
    pub cold: Vec<(&'a str, String)>,
}

fn text(value: Option<&str>) -> String {
    value.map_or_else(|| "-".to_owned(), |text| format!("{text:?}"))
}

fn texts<'a>(values: impl IntoIterator<Item = &'a str>) -> String {
    let quoted: Vec<String> = values.into_iter().map(|text| format!("{text:?}")).collect();
    format!("[{}]", quoted.join(","))
}

fn optional_texts(values: Option<&Vec<&str>>) -> String {
    values.map_or_else(|| "-".to_owned(), |values| texts(values.iter().copied()))
}

/// A value as compact JSON.
pub fn json(value: &Value) -> String {
    dumps(value, &Options::compact(), None).unwrap_or_else(|_| "<unserializable>".to_owned())
}

impl ServiceView<'_> {
    pub fn text(&self) -> String {
        let b = &self.botocore;
        let mut out = format!(
            "service version={} sdk_id={} endpoint_prefix={} sigv4_name={} auth={} \
             xml_namespace={} query_compatible={} protocol_traits={}\n\
             botocore protocol={} protocols={} json_version={} target_prefix={} \
             signature_version={} uid={}\n",
            text(self.version),
            text(self.sdk_id),
            text(self.endpoint_prefix),
            text(self.sigv4_name),
            optional_texts(self.auth.as_ref()),
            text(self.xml_namespace),
            self.query_compatible,
            texts(self.protocol_traits.iter().copied()),
            text(b.protocol),
            b.protocols.map_or_else(
                || "-".to_owned(),
                |list| texts(list.iter().map(|item| &**item))
            ),
            text(b.json_version),
            text(b.target_prefix),
            text(b.signature_version),
            text(b.uid),
        );
        for (name, type_name) in &self.client_context_params {
            let _ = writeln!(out, "client_context_param {name:?} {type_name:?}");
        }
        out
    }
}

impl OperationView<'_> {
    pub fn text(&self) -> String {
        let checksum = self.http_checksum.as_ref().map_or_else(
            || "-".to_owned(),
            |(member, required, mode, algorithms)| {
                format!(
                    "({},{required},{},{})",
                    text(*member),
                    text(*mode),
                    texts(algorithms.iter().copied())
                )
            },
        );
        let statics: Vec<String> = (self.static_context_params.iter())
            .map(|(name, value)| format!("{name:?}={}", json(value)))
            .collect();
        let paths: Vec<String> = (self.operation_context_params.iter())
            .map(|(name, path)| format!("{name:?}={path:?}"))
            .collect();
        format!(
            "operation {:?} wire_name={} http={} {} host_prefix={} auth={} auth_type={} \
             unsigned_payload={} http_checksum_required={} endpoint_operation={} \
             http_checksum={checksum} endpoint_discovery={} request_compression={} \
             static_context_params=[{}] operation_context_params=[{}]\n",
            self.name,
            text(self.wire_name),
            text(self.http_method),
            text(self.http_request_uri),
            text(self.host_prefix),
            optional_texts(self.auth.as_ref()),
            text(self.auth_type),
            self.unsigned_payload,
            self.http_checksum_required,
            self.endpoint_operation,
            self.endpoint_discovery
                .map_or_else(|| "-".to_owned(), |required| required.to_string()),
            optional_texts(self.request_compression.as_ref()),
            statics.join(","),
            paths.join(","),
        )
    }
}

impl TraitsView<'_> {
    /// The present traits, each ` name=value`, in a fixed order.
    pub fn text(&self) -> String {
        let mut out = String::new();
        let flags: Vec<&str> = FLAG_NAMES
            .into_iter()
            .filter(|name| self.flags.contains(name))
            .collect();
        if !flags.is_empty() {
            let _ = write!(out, " flags={}", flags.join(","));
        }
        self.texts(&mut out);
        if let Some((code, status, sender)) = self.error {
            let status = status.map_or_else(|| "-".to_owned(), |code| code.to_string());
            let _ = write!(out, " error=({},{status},{sender})", text(code));
        }
        if let Some(throttling) = self.retryable {
            let _ = write!(out, " retryable=({throttling})");
        }
        for (id, value) in &self.cold {
            let _ = write!(out, " cold:{id}={value}");
        }
        out
    }

    /// The texts, lists and bounds.
    fn texts(&self, out: &mut String) {
        let single = [
            ("location", self.location),
            ("locationName", self.location_name),
            ("jsonName", self.json_name),
            ("xmlName", self.xml_name),
            ("queryName", self.ec2_query_name),
            ("payload", self.payload),
            ("timestampFormat", self.timestamp_format),
            ("pattern", self.pattern),
            ("contextParam", self.context_param),
            ("resultWrapper", self.result_wrapper),
        ];
        for (name, value) in single {
            if let Some(value) = value {
                let _ = write!(out, " {name}={value:?}");
            }
        }
        if let Some((uri, prefix)) = self.xml_namespace {
            let _ = write!(out, " xmlNamespace=({},{})", text(uri), text(prefix));
        }
        for (name, list) in [("required", &self.required), ("enum", &self.enum_values)] {
            if !list.is_empty() {
                let _ = write!(out, " {name}={}", texts(list.iter().copied()));
            }
        }
        for (name, bound) in [("min", &self.min), ("max", &self.max)] {
            if let Some(bound) = bound {
                let _ = write!(out, " {name}={bound}");
            }
        }
    }
}

/// An uninterpreted key's line.
pub fn uninterpreted(key: &Uninterpreted) -> String {
    let site = match &key.site {
        Site::Service => "service".to_owned(),
        Site::Operation(name) => format!("operation {name:?}"),
        Site::Shape(name) => format!("shape {name:?}"),
    };
    format!("uninterpreted {site} {:?}\n", key.key)
}

/// The type a line names: botocore's `type` text, and the schema type.
pub fn type_label(shape_type: ShapeType) -> &'static str {
    match shape_type {
        ShapeType::Boolean => "boolean",
        ShapeType::Byte => "byte",
        ShapeType::Short => "short",
        ShapeType::Integer => "integer",
        ShapeType::Long => "long",
        ShapeType::Float => "float",
        ShapeType::Double => "double",
        ShapeType::BigInteger => "bigInteger",
        ShapeType::BigDecimal => "bigDecimal",
        ShapeType::String => "string",
        ShapeType::Blob => "blob",
        ShapeType::Timestamp => "timestamp",
        ShapeType::Document => "document",
        ShapeType::List => "list",
        ShapeType::Map => "map",
        ShapeType::Structure => "structure",
        ShapeType::Union => "union",
        ShapeType::Unknown => "unknown",
    }
}

/// The projection of `schema`, with botocore's own metadata.
pub fn render(schema: &SchemaSet, botocore: &Botocore<'_>) -> String {
    let mut out = service(schema, *botocore).text();
    for shape in schema.named() {
        let _ = writeln!(
            out,
            "shape {:?} {} {:?}{}",
            shape.id().name(),
            type_label(shape.shape_type()),
            shape.type_name(),
            traits_view(shape).text()
        );
        for member in shape.members() {
            let name = member.member_name().unwrap_or_default();
            out.push_str(&child(&format!("member {name:?}"), member));
        }
        if let Some(member) = shape.list_member() {
            out.push_str(&child("list_member", member));
        }
        if let (Some(key), Some(value)) = (shape.map_key(), shape.map_value()) {
            out.push_str(&child("map_key", key));
            out.push_str(&child("map_value", value));
        }
    }
    for operation in schema.service().operations() {
        out.push_str(&operation_view(operation).text());
        let roots = operation.input().map(|root| ("input", root)).into_iter();
        let roots = roots.chain(operation.output().map(|root| ("output", root)));
        for (label, root) in roots.chain(operation.errors().map(|root| ("error", root))) {
            out.push_str(&child(label, root));
        }
    }
    for key in schema.uninterpreted() {
        out.push_str(&uninterpreted(key));
    }
    out
}

/// A member's or root's line: its target's short name and its traits.
fn child(label: &str, schema: Schema<'_>) -> String {
    let target = schema.target().id().name();
    format!("  {label} -> {target:?}{}\n", traits_view(schema).text())
}

fn service<'a>(schema: &'a SchemaSet, botocore: Botocore<'a>) -> ServiceView<'a> {
    let service = schema.service();
    ServiceView {
        version: service.version(),
        sdk_id: service.sdk_id(),
        endpoint_prefix: service.endpoint_prefix(),
        sigv4_name: service.sigv4_name(),
        auth: service.auth().map(Iterator::collect),
        xml_namespace: service.xml_namespace(),
        query_compatible: service.query_compatible(),
        protocol_traits: sorted(service.protocol_traits().iter().map(|id| id.as_str())),
        client_context_params: (service.client_context_params().iter())
            .map(|param| (&*param.name, &*param.type_name))
            .collect(),
        botocore,
    }
}

/// Trait ids as a set: sorted by their text, not their place in `KNOWN`.
pub fn sorted<'a>(ids: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let mut ids: Vec<&str> = ids.collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn operation_view(operation: OperationSchema<'_>) -> OperationView<'_> {
    OperationView {
        name: operation.name(),
        wire_name: operation.wire_name(),
        http_method: operation.http_method(),
        http_request_uri: operation.http_request_uri(),
        host_prefix: operation.host_prefix(),
        auth: operation.auth().map(Iterator::collect),
        auth_type: operation.auth_type(),
        unsigned_payload: operation.unsigned_payload(),
        http_checksum_required: operation.http_checksum_required(),
        endpoint_operation: operation.endpoint_operation(),
        http_checksum: operation.http_checksum().map(|checksum| {
            (
                checksum.request_algorithm_member.as_deref(),
                checksum.request_checksum_required,
                checksum.request_validation_mode_member.as_deref(),
                checksum.response_algorithms.iter().map(|a| &**a).collect(),
            )
        }),
        endpoint_discovery: operation.endpoint_discovery().map(|d| d.required),
        request_compression: operation.request_compression().map(Iterator::collect),
        static_context_params: operation.static_context_params().collect(),
        operation_context_params: operation.operation_context_params().collect(),
    }
}

/// A bound's text: `i` and the integer, or `f` and Python's float repr.
pub fn bound(bound: super::Bound) -> String {
    match bound {
        super::Bound::Integer(_) => format!("i{bound}"),
        super::Bound::Float(_) => format!("f{bound}"),
    }
}

/// C2J's residues with no slot: a flag the cold map holds, by key.
const COLD_FLAGS: [(&str, TraitId); 3] = [
    ("event", traits::EVENT),
    ("document", traits::DOCUMENT),
    ("union", traits::UNION),
];

fn traits_view(schema: Schema<'_>) -> TraitsView<'_> {
    let mut flags: Vec<&'static str> = SLOT_FLAGS
        .into_iter()
        .filter(|(_, flag)| schema.flag(*flag))
        .map(|(name, _)| name)
        .collect();
    // A cold `document` or `union` says what the type does not; else the
    // type says it.
    let typed = [
        ("document", ShapeType::Document),
        ("union", ShapeType::Union),
    ];
    for (name, id) in COLD_FLAGS {
        let typed = typed.iter().find(|(key, _)| *key == name);
        let on = match schema.cold_trait(id.as_str()) {
            Some(value) => truthy(value),
            None => typed.is_some_and(|(_, ty)| schema.shape_type() == *ty),
        };
        if on {
            flags.push(name);
        }
    }
    let cold = schema
        .cold_traits()
        .filter(|(id, _)| !COLD_FLAGS.iter().any(|(_, flag)| flag.as_str() == *id))
        .map(|(id, value)| (id, value.to_string()))
        .collect();
    TraitsView {
        flags,
        location: schema.location(),
        location_name: schema.location_name(),
        json_name: schema.json_name(),
        xml_name: schema.xml_name(),
        ec2_query_name: schema.ec2_query_name(),
        payload: schema.payload(),
        timestamp_format: schema.timestamp_format_text(),
        xml_namespace: schema.xml_namespace().map(|ns| (ns.uri, ns.prefix)),
        required: schema.required().collect(),
        enum_values: schema.enum_values().collect(),
        min: schema.min().map(bound),
        max: schema.max().map(bound),
        pattern: schema.pattern(),
        error: (schema.error()).map(|e| (e.code, e.http_status, e.sender_fault)),
        retryable: schema.retryable().map(|r| r.throttling),
        context_param: schema.context_param(),
        result_wrapper: schema.result_wrapper(),
        cold,
    }
}

/// Python truthiness of a JSON value.
fn truthy(value: &serde_json::Value) -> bool {
    use serde_json::Value as Json;
    match value {
        Json::Null => false,
        Json::Bool(value) => *value,
        Json::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Json::String(text) => !text.is_empty(),
        Json::Array(items) => !items.is_empty(),
        Json::Object(map) => !map.is_empty(),
    }
}

#[cfg(test)]
mod tests;
