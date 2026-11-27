//! The arena's projection: what [`crate::shapes::Model`] read of a
//! description, in [`crate::schema::projection`]'s lines, for the C2J
//! front-end to be compared with until the arena goes.
use crate::schema::ShapeType;
use crate::schema::projection::{
    Botocore, OperationView, ServiceView, TraitsView, bound, sorted, type_label, uninterpreted,
};
use crate::shapes::{Flag, Model, Operation, ShapeId, ShapeKind, Traits};
use std::fmt::Write as _;

/// The arena's flags by their C2J key.
const FLAGS: [(&str, Flag); 17] = [
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

/// The projection of the arena's reading of `model`.
pub fn render(model: &Model) -> String {
    let mut out = service(model).text();
    for id in model.named_shapes() {
        let shape = model.shape(id);
        let name = &*shape.name;
        let _ = writeln!(
            out,
            "shape {name:?} {} {:?}{}",
            type_label(shape_type(&shape.kind, &shape.traits)),
            type_name(&shape.kind),
            traits_view(&shape.traits).text()
        );
        match &shape.kind {
            ShapeKind::Structure { .. } => {
                for member in model.members(id) {
                    let label = format!("member {:?}", member.name);
                    out.push_str(&child(model, &label, member.shape));
                }
            }
            ShapeKind::List { member } => out.push_str(&child(model, "list_member", *member)),
            ShapeKind::Map { key, value } => {
                out.push_str(&child(model, "map_key", *key));
                out.push_str(&child(model, "map_value", *value));
            }
            _ => {}
        }
    }
    for operation in model.operations() {
        out.push_str(&operation_view(operation).text());
        let roots = operation.input.iter().map(|id| ("input", *id));
        let roots = roots.chain(operation.output.iter().map(|id| ("output", *id)));
        for (label, id) in roots.chain(operation.errors.iter().map(|id| ("error", *id))) {
            out.push_str(&child(model, label, id));
        }
    }
    for key in model.uninterpreted() {
        out.push_str(&uninterpreted(key));
    }
    out
}

fn child(model: &Model, label: &str, id: ShapeId) -> String {
    let shape = model.shape(id);
    let name = &*shape.name;
    format!(
        "  {label} -> {name:?}{}\n",
        traits_view(&shape.traits).text()
    )
}

/// The schema type the C2J front-end gives a named shape of this kind.
fn shape_type(kind: &ShapeKind, traits: &Traits) -> ShapeType {
    match kind {
        ShapeKind::Structure { .. } if traits.is(Flag::Document) => ShapeType::Document,
        ShapeKind::Structure { .. } if traits.is(Flag::Union) => ShapeType::Union,
        ShapeKind::Structure { .. } => ShapeType::Structure,
        ShapeKind::List { .. } => ShapeType::List,
        ShapeKind::Map { .. } => ShapeType::Map,
        ShapeKind::String => ShapeType::String,
        ShapeKind::Integer => ShapeType::Integer,
        ShapeKind::Long => ShapeType::Long,
        ShapeKind::Boolean => ShapeType::Boolean,
        ShapeKind::Float => ShapeType::Float,
        ShapeKind::Double => ShapeType::Double,
        ShapeKind::Timestamp => ShapeType::Timestamp,
        ShapeKind::Blob => ShapeType::Blob,
        ShapeKind::Other(_) => ShapeType::Unknown,
    }
}

fn type_name(kind: &ShapeKind) -> &str {
    match kind {
        ShapeKind::Structure { .. } => "structure",
        ShapeKind::List { .. } => "list",
        ShapeKind::Map { .. } => "map",
        ShapeKind::String => "string",
        ShapeKind::Integer => "integer",
        ShapeKind::Long => "long",
        ShapeKind::Boolean => "boolean",
        ShapeKind::Float => "float",
        ShapeKind::Double => "double",
        ShapeKind::Timestamp => "timestamp",
        ShapeKind::Blob => "blob",
        ShapeKind::Other(text) => text,
    }
}

fn traits_view(traits: &Traits) -> TraitsView<'_> {
    let bound = |value: Option<crate::shapes::Bound>| {
        value.map(|value| match value {
            crate::shapes::Bound::Integer(n) => bound(crate::schema::Bound::Integer(n)),
            crate::shapes::Bound::Float(f) => bound(crate::schema::Bound::Float(f)),
        })
    };
    TraitsView {
        flags: (FLAGS.iter())
            .filter(|(_, flag)| traits.is(*flag))
            .map(|(name, _)| *name)
            .collect(),
        location: traits.location.as_deref(),
        location_name: traits.name.as_deref(),
        json_name: traits.name.as_deref(),
        xml_name: traits.name.as_deref(),
        ec2_query_name: traits.query_name.as_deref(),
        payload: traits.payload.as_deref(),
        timestamp_format: traits.timestamp_format.as_deref(),
        xml_namespace: (traits.xml_namespace.as_ref())
            .map(|ns| (ns.uri.as_deref(), ns.prefix.as_deref())),
        required: listed(&traits.required),
        enum_values: listed(&traits.enum_values),
        min: bound(traits.min),
        max: bound(traits.max),
        pattern: traits.pattern.as_deref(),
        error: (traits.error.as_ref())
            .map(|e| (e.code.as_deref(), e.http_status_code, e.sender_fault)),
        retryable: traits.retryable.map(|r| r.throttling),
        context_param: traits.context_param.as_deref(),
        result_wrapper: traits.result_wrapper.as_deref(),
        cold: Vec::new(),
    }
}

fn listed(list: &[Box<str>]) -> Vec<&str> {
    list.iter().map(|item| &**item).collect()
}

fn texts(list: &Option<Box<[Box<str>]>>) -> Option<Vec<&str>> {
    list.as_deref().map(listed)
}

fn service(model: &Model) -> ServiceView<'_> {
    let metadata = model.metadata();
    ServiceView {
        version: metadata.api_version.as_deref(),
        sdk_id: metadata.service_id.as_deref(),
        endpoint_prefix: metadata.endpoint_prefix.as_deref(),
        sigv4_name: metadata.signing_name.as_deref(),
        auth: texts(&metadata.auth),
        xml_namespace: metadata.xml_namespace.as_deref(),
        query_compatible: metadata.aws_query_compatible,
        protocol_traits: protocol_traits(model),
        client_context_params: (model.client_context_params().iter())
            .map(|param| (&*param.name, &*param.type_name))
            .collect(),
        botocore: Botocore {
            protocol: metadata.protocol.as_deref(),
            protocols: metadata.protocols.as_deref(),
            json_version: metadata.json_version.as_deref(),
            target_prefix: metadata.target_prefix.as_deref(),
            signature_version: metadata.signature_version.as_deref(),
            uid: metadata.uid.as_deref(),
        },
    }
}

/// The protocol traits of the metadata, by the front-end's mapping: the
/// texts it reads are compared on the `botocore` line.
fn protocol_traits(model: &Model) -> Vec<&'static str> {
    let metadata = model.metadata();
    let ids = super::read::protocol_traits(
        metadata.protocol.clone(),
        metadata.protocols.clone(),
        metadata.json_version.as_deref(),
    );
    sorted(ids.iter().map(|id| id.as_str()))
}

fn operation_view(operation: &Operation) -> OperationView<'_> {
    OperationView {
        name: &operation.name,
        wire_name: operation.wire_name.as_deref(),
        http_method: operation.http.method.as_deref(),
        http_request_uri: operation.http.request_uri.as_deref(),
        host_prefix: operation.host_prefix.as_deref(),
        auth: texts(&operation.auth),
        auth_type: operation.auth_type.as_deref(),
        unsigned_payload: operation.unsigned_payload,
        http_checksum_required: operation.http_checksum_required,
        endpoint_operation: operation.endpoint_operation,
        http_checksum: operation.http_checksum.as_ref().map(|checksum| {
            (
                checksum.request_algorithm_member.as_deref(),
                checksum.request_checksum_required,
                checksum.request_validation_mode_member.as_deref(),
                checksum.response_algorithms.iter().map(|a| &**a).collect(),
            )
        }),
        endpoint_discovery: operation.endpoint_discovery.map(|d| d.required),
        request_compression: texts(&operation.request_compression),
        static_context_params: (operation.static_context_params.iter())
            .map(|(name, value)| (&**name, value))
            .collect(),
        operation_context_params: (operation.operation_context_params.iter())
            .map(|(name, path)| (&**name, &**path))
            .collect(),
    }
}

#[cfg(test)]
mod tests;
