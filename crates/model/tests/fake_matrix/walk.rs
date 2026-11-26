//! The features of a `service-2.json`: metadata, operations, and every
//! shape an operation reaches, by the protocol botocore resolves.
use super::census::{Features, Ignored};
use super::forms::{add, entries, expression_forms, json_kind, strings};
use serde_json::{Map, Value as Json};
use std::collections::BTreeSet;

/// `botocore.utils.PRIORITY_ORDERED_SUPPORTED_PROTOCOLS`.
pub(crate) const PROTOCOL_PRIORITY: &[&str] = &[
    "json",
    "rest-json",
    "rest-xml",
    "smithy-rpc-v2-cbor",
    "query",
    "ec2",
];

/// Shape and reference keys whose reading depends on the protocol: they
/// name, place, frame or encode a value on the wire. The others (`min`,
/// `max`, `pattern`, `enum`, `required`, `sensitive`, `retryable`, ...) are
/// read the same way for every protocol.
const WIRE: &[&str] = &[
    "contextParam",
    "document",
    "endpointdiscoveryid",
    "error",
    "event",
    "eventheader",
    "eventpayload",
    "eventstream",
    "exception",
    "flattened",
    "hostLabel",
    "idempotencyToken",
    "jsonvalue",
    "locationName",
    "payload",
    "queryName",
    "requiresLength",
    "resultWrapper",
    "streaming",
    "timestampFormat",
    "union",
    "xmlAttribute",
    "xmlNamespace",
];
/// Trait keys whose string value is itself a feature.
const VALUED: &[&str] = &["timestampFormat"];
/// Operation keys that change a request's or response's encoding or
/// signing, crossed with the protocol.
const CROSSED: &[&str] = &[
    "auth",
    "authtype",
    "endpoint",
    "errors",
    "http",
    "httpChecksum",
    "httpChecksumRequired",
    "input",
    "output",
    "requestcompression",
    "unsignedPayload",
];
/// Metadata keys whose string value is itself a feature.
const VALUED_METADATA: &[&str] = &["protocol", "signatureVersion", "jsonVersion"];
/// The locations a list or map element has, not a member's.
const ELEMENT: &[&str] = &["member", "key", "value"];

/// `ServiceModel.resolved_protocol`: the first of `protocols` in
/// botocore's priority order, else `protocol`; `unsupported` when
/// `protocols` names none botocore speaks.
pub(crate) fn resolved_protocol(metadata: Option<&Json>) -> &str {
    let field = |key: &str| metadata.and_then(|metadata| metadata.get(key));
    let listed: Vec<&str> = strings(field("protocols")).collect();
    if listed.is_empty() {
        return field("protocol").and_then(Json::as_str).unwrap_or("-");
    }
    let resolved = PROTOCOL_PRIORITY
        .iter()
        .find(|protocol| listed.contains(protocol));
    resolved.copied().unwrap_or("unsupported")
}

/// The features of a `service-2.json`.
pub(crate) fn service_features(json: &Json, ignored: &Ignored, out: &mut Features) {
    for (key, _) in entries(Some(json)) {
        let spine = ["metadata", "operations", "shapes"].contains(&&**key);
        if !spine && !ignored.document.contains(key) {
            add(out, &["doc", key]);
        }
    }
    let metadata = json.get("metadata");
    metadata_features(metadata, ignored, out);
    client_context_features(json.get("clientContextParams"), ignored, out);
    let mut walk = Walk {
        protocol: resolved_protocol(metadata),
        shapes: json.get("shapes").and_then(Json::as_object),
        ignored,
        seen: BTreeSet::new(),
        out,
    };
    for (_, operation) in entries(json.get("operations")) {
        walk.operation(operation);
    }
}

fn metadata_features(metadata: Option<&Json>, ignored: &Ignored, out: &mut Features) {
    for (key, value) in entries(metadata) {
        if ignored.metadata.contains(key) {
            continue;
        }
        add(out, &["meta", key]);
        match value {
            Json::String(text) if VALUED_METADATA.contains(&&**key) => {
                add(out, &["meta", key, text]);
            }
            Json::Array(_) => strings(Some(value)).for_each(|item| add(out, &["meta", key, item])),
            _ => {}
        }
    }
}

fn client_context_features(parameters: Option<&Json>, ignored: &Ignored, out: &mut Features) {
    for (_, parameter) in entries(parameters) {
        for (key, value) in entries(Some(parameter)) {
            match (key.as_str(), value.as_str()) {
                _ if ignored.client_context_param.contains(key) => {}
                ("type", Some(kind)) => add(out, &["ccp", "type", &kind.to_lowercase()]),
                _ => add(out, &["ccp", key]),
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Direction {
    In,
    Out,
    Error,
}

impl Direction {
    fn as_str(self) -> &'static str {
        match self {
            Self::In => "in",
            Self::Out => "out",
            Self::Error => "err",
        }
    }
}

/// A walk of one service's operations and the shapes they reach; a shape
/// no operation reaches is no feature, as botocore never reads it.
struct Walk<'a, 'o> {
    protocol: &'a str,
    shapes: Option<&'a Map<String, Json>>,
    ignored: &'a Ignored,
    seen: BTreeSet<(&'a str, Direction)>,
    out: &'o mut Features,
}

impl<'a> Walk<'a, '_> {
    fn add(&mut self, parts: &[&str]) {
        add(self.out, parts);
    }

    fn operation(&mut self, operation: &'a Json) {
        let protocol = self.protocol;
        for (key, value) in entries(Some(operation)) {
            if self.ignored.operation.contains(key) || key == "name" {
                continue;
            }
            if CROSSED.contains(&&**key) {
                self.add(&["op", protocol, key]);
            } else {
                self.add(&["op", key]);
            }
            self.operation_key(key, value);
        }
    }

    fn operation_key(&mut self, key: &str, value: &'a Json) {
        match key {
            "http" => self.http(value),
            "input" => self.operation_reference(key, value, Direction::In),
            "output" => self.operation_reference(key, value, Direction::Out),
            "errors" => {
                let errors = value.as_array().into_iter().flatten();
                errors.for_each(|error| self.operation_reference(key, error, Direction::Error));
            }
            "endpoint" => self.endpoint(value),
            "authtype" => self.add(&["op", "authtype", value.as_str().unwrap_or("-")]),
            "auth" => strings(Some(value)).for_each(|item| self.add(&["op", "auth", item])),
            "staticContextParams" => {
                for (_, parameter) in entries(Some(value)) {
                    let kind = parameter.get("value").map_or("-", json_kind);
                    self.add(&["op", key, kind]);
                }
            }
            "operationContextParams" => {
                for (_, parameter) in entries(Some(value)) {
                    let path = parameter.get("path").and_then(Json::as_str);
                    for form in expression_forms(path.unwrap_or_default()) {
                        self.add(&["op", key, &form]);
                    }
                }
            }
            _ => self.nested(key, value),
        }
    }

    /// An object-valued operation key's own keys, and the strings a list
    /// under one of them holds (`requestcompression.encodings`,
    /// `httpChecksum.responseAlgorithms`).
    fn nested(&mut self, key: &str, value: &Json) {
        for (sub, item) in entries(Some(value)) {
            self.add(&["op", key, sub]);
            strings(Some(item)).for_each(|text| add(self.out, &["op", key, sub, text]));
        }
    }

    fn http(&mut self, http: &Json) {
        let protocol = self.protocol;
        for (key, value) in entries(Some(http)) {
            let text = value.as_str().unwrap_or_default();
            match key.as_str() {
                _ if self.ignored.http.contains(key) => {}
                "method" => self.add(&["op", protocol, "http", "method", text]),
                "requestUri" => {
                    let forms = [("label", "{"), ("greedy", "+}"), ("query", "?")];
                    for (form, marker) in forms {
                        if text.contains(marker) {
                            self.add(&["op", protocol, "uri", form]);
                        }
                    }
                }
                _ => self.add(&["op", protocol, "http", key]),
            }
        }
    }

    fn endpoint(&mut self, endpoint: &Json) {
        let protocol = self.protocol;
        for (key, value) in entries(Some(endpoint)) {
            if key == "hostPrefix" {
                let text = value.as_str().unwrap_or_default();
                let form = if text.contains('{') {
                    "label"
                } else {
                    "literal"
                };
                self.add(&["op", protocol, "hostPrefix", form]);
            } else {
                self.add(&["op", "endpoint", key]);
            }
        }
    }

    /// An operation's input, output or error reference: its own keys, then
    /// the shape it names.
    fn operation_reference(&mut self, which: &str, reference: &'a Json, direction: Direction) {
        let protocol = self.protocol;
        for (key, _) in entries(Some(reference)) {
            if key != "shape" && !self.ignored.reference.contains(key) {
                self.add(&["op", protocol, which, key]);
            }
        }
        self.shape(reference, direction);
    }

    fn target(&self, reference: &Json) -> Option<&'a Map<String, Json>> {
        let name = reference.get("shape")?.as_str()?;
        self.shapes?.get(name)?.as_object()
    }

    fn target_type(&self, reference: Option<&Json>) -> &'a str {
        let target = reference.and_then(|reference| self.target(reference));
        let kind = target.and_then(|shape| shape.get("type"));
        kind.and_then(Json::as_str).unwrap_or("-")
    }

    /// The shape a reference names, once per direction, and its members.
    fn shape(&mut self, reference: &'a Json, direction: Direction) {
        let Some(name) = reference.get("shape").and_then(Json::as_str) else {
            return;
        };
        let Some(shape) = self.target(reference) else {
            return;
        };
        if !self.seen.insert((name, direction)) {
            return;
        }
        let kind = self.target_type(Some(reference));
        let prefix = ["shape", self.protocol, direction.as_str(), kind];
        self.add(&prefix);
        for (key, value) in shape {
            if !self.ignored.shape.contains(key) && !self.ignored.shape_spine.contains(key) {
                self.classified(&prefix, kind, key, value);
            }
        }
        for (_, member) in entries(shape.get("members")) {
            let location = member.get("location").and_then(Json::as_str);
            self.member(member, location.unwrap_or("body"), direction);
        }
        for child in ELEMENT {
            if let Some(reference) = shape.get(*child) {
                self.member(reference, child, direction);
            }
        }
    }

    /// A member (or list member, map key or value) reference in `location`.
    /// A list or map outside the body also names its elements' types: a
    /// header list or a querystring map serializes them its own way.
    fn member(&mut self, reference: &'a Json, location: &str, direction: Direction) {
        let kind = self.target_type(Some(reference));
        let prefix = ["member", self.protocol, direction.as_str(), kind, location];
        self.add(&prefix);
        for (key, value) in entries(Some(reference)) {
            let structural = key == "shape" || key == "location";
            if !structural && !self.ignored.reference.contains(key) {
                self.classified(&prefix, kind, key, value);
            }
        }
        if location != "body" && !ELEMENT.contains(&location) {
            for child in ELEMENT {
                let element = self.target(reference).and_then(|shape| shape.get(*child));
                if element.is_some() {
                    let element = self.target_type(element);
                    let mut parts = prefix.to_vec();
                    parts.extend([*child, element]);
                    self.add(&parts);
                }
            }
        }
        self.shape(reference, direction);
    }

    /// A trait `key` on a shape of type `kind`: under `prefix` when it is a
    /// wire trait, by shape type alone otherwise. An object value adds each
    /// of its keys (`retryable.throttling`, `error.httpStatusCode`), a
    /// [`VALUED`] trait its value (`timestampFormat/iso8601`).
    fn classified(&mut self, prefix: &[&str], kind: &str, key: &str, value: &Json) {
        let mut parts = if WIRE.contains(&key) {
            prefix.to_vec()
        } else {
            vec!["trait", kind]
        };
        parts.push(key);
        self.add(&parts);
        for (sub, _) in entries(Some(value)) {
            let mut nested = parts.clone();
            nested.push(sub);
            self.add(&nested);
        }
        if let (true, Some(text)) = (VALUED.contains(&key), value.as_str()) {
            parts.push(text);
            self.add(&parts);
        }
    }
}
