//! The feature census: every model feature a set of botocore-format models
//! uses, as `family/part/...` names, each with the services that use it.
//!
//! A feature is a key botocore reads, crossed with what changes how it is
//! read: the protocol and direction (`in` for what is serialized, `out` for
//! what is parsed) for wire keys, the shape type for the validator's keys.
//! Keys the model loader ignores (the `*_IGNORED` lists of
//! `crates/model/src/shapes/keys.rs`, read from its source) are no feature.
//! Any other key is one, so a key AWS adds becomes a feature by itself.
use model::source::{DirSource, Kind, Source, load_service_model};
use serde_json::{Map, Value as Json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub(crate) type Error = Box<dyn std::error::Error>;
/// Each feature, with the services that use it.
pub(crate) type Census = BTreeMap<String, BTreeSet<String>>;
type Features = BTreeSet<String>;

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

/// The loader's ignored keys and the shape keys that are structure, not
/// traits, as `keys.rs` declares them.
#[derive(Debug)]
pub(crate) struct Ignored {
    document: BTreeSet<String>,
    metadata: BTreeSet<String>,
    operation: BTreeSet<String>,
    http: BTreeSet<String>,
    shape: BTreeSet<String>,
    shape_spine: BTreeSet<String>,
    reference: BTreeSet<String>,
    client_context_param: BTreeSet<String>,
}

impl Ignored {
    /// The lists in `keys.rs` under the repository `root`.
    pub(crate) fn read(root: &Path) -> Result<Self, Error> {
        let path = root.join("crates/model/src/shapes/keys.rs");
        let source = std::fs::read_to_string(&path)?;
        let list = |name: &str| string_list(&source, name);
        Ok(Self {
            document: list("DOCUMENT_IGNORED")?,
            metadata: list("METADATA_IGNORED")?,
            operation: list("OPERATION_IGNORED")?,
            http: list("HTTP_IGNORED")?,
            shape: list("SHAPE_IGNORED")?,
            shape_spine: list("SHAPE")?,
            reference: list("REFERENCE_IGNORED")?,
            client_context_param: list("CLIENT_CONTEXT_PARAM_IGNORED")?,
        })
    }
}

/// The string literals of `const <name>: &[&str] = &[...];` in `source`.
pub(crate) fn string_list(source: &str, name: &str) -> Result<BTreeSet<String>, Error> {
    let head = format!("const {name}: &[&str] = &[");
    let start = source
        .find(&head)
        .ok_or_else(|| format!("keys.rs declares no `{head}`"))?;
    let body = &source[start + head.len()..];
    let end = body
        .find("];")
        .ok_or_else(|| format!("keys.rs: `{name}` is not closed"))?;
    let items: BTreeSet<String> = body[..end]
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect();
    if items.is_empty() {
        return Err(format!("keys.rs: `{name}` lists no key").into());
    }
    Ok(items)
}

/// The census of every service in the botocore data directory `root`.
pub(crate) fn census(root: &Path, ignored: &Ignored) -> Result<Census, Error> {
    let source = DirSource::new(root);
    let kinds = [
        Kind::Service,
        Kind::EndpointRuleSet,
        Kind::Paginators,
        Kind::Waiters,
    ];
    let mut listed = Vec::new();
    for kind in kinds {
        listed.push(source.services(kind)?);
    }
    let mut census = Census::new();
    for service in &listed[0] {
        let mut features = Features::new();
        for (kind, services) in kinds.iter().zip(&listed) {
            if !services.contains(service) {
                continue;
            }
            let json = load_service_model(&source, service, *kind)
                .map_err(|error| format!("{service} {}: {error}", kind.type_name()))?
                .json;
            match kind {
                Kind::Service => service_features(&json, ignored, &mut features),
                Kind::EndpointRuleSet => rule_set_features(&json, &mut features),
                Kind::Paginators => paginator_features(&json, &mut features),
                Kind::Waiters => waiter_features(&json, &mut features),
            }
        }
        for feature in features {
            census.entry(feature).or_default().insert(service.clone());
        }
    }
    Ok(census)
}

fn add(out: &mut Features, parts: &[&str]) {
    out.insert(parts.join("/"));
}

fn entries(value: Option<&Json>) -> impl Iterator<Item = (&String, &Json)> {
    value.and_then(Json::as_object).into_iter().flatten()
}

fn strings(value: Option<&Json>) -> impl Iterator<Item = &str> {
    let items = value.and_then(Json::as_array).into_iter().flatten();
    items.filter_map(Json::as_str)
}

/// The JSON type of a value, as a feature part.
fn json_kind(value: &Json) -> &'static str {
    match value {
        Json::Null => "null",
        Json::Bool(_) => "bool",
        Json::Number(number) if number.is_f64() => "float",
        Json::Number(_) => "int",
        Json::String(_) => "string",
        Json::Array(_) => "list",
        Json::Object(_) => "object",
    }
}

/// The features of a `service-2.json`.
pub(crate) fn service_features(json: &Json, ignored: &Ignored, out: &mut Features) {
    for (key, _) in entries(Some(json)) {
        if !ignored.document.contains(key)
            && !["metadata", "operations", "shapes"].contains(&&**key)
        {
            add(out, &["doc", key]);
        }
    }
    let metadata = json.get("metadata");
    metadata_features(metadata, ignored, out);
    client_context_features(json.get("clientContextParams"), ignored, out);
    let protocol = metadata
        .and_then(|metadata| metadata.get("protocol"))
        .and_then(Json::as_str)
        .unwrap_or("-");
    let mut walk = Walk {
        protocol,
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
}

impl Direction {
    fn as_str(self) -> &'static str {
        match self {
            Self::In => "in",
            Self::Out => "out",
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
            match key.as_str() {
                "http" => self.http(value),
                "input" => self.operation_reference(key, value, Direction::In),
                "output" => self.operation_reference(key, value, Direction::Out),
                "errors" => {
                    let errors = value.as_array().into_iter().flatten();
                    errors.for_each(|error| self.operation_reference(key, error, Direction::Out));
                }
                "endpoint" => self.endpoint(value),
                "authtype" => {
                    let kind = value.as_str().unwrap_or("-");
                    self.add(&["op", "authtype", kind]);
                }
                "auth" => strings(Some(value)).for_each(|item| self.add(&["op", "auth", item])),
                "staticContextParams" => {
                    for (_, parameter) in entries(Some(value)) {
                        let kind = parameter.get("value").map_or("-", json_kind);
                        self.add(&["op", key, kind]);
                    }
                }
                _ => self.nested(key, value),
            }
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
        let kind = shape.get("type").and_then(Json::as_str).unwrap_or("-");
        let prefix = ["shape", self.protocol, direction.as_str(), kind];
        self.add(&prefix);
        for key in shape.keys() {
            if !self.ignored.shape.contains(key) && !self.ignored.shape_spine.contains(key) {
                self.classified(&prefix, kind, key);
            }
        }
        for (_, member) in entries(shape.get("members")) {
            let location = member.get("location").and_then(Json::as_str);
            self.member(member, location.unwrap_or("body"), direction);
        }
        for child in ["member", "key", "value"] {
            if let Some(reference) = shape.get(child) {
                self.member(reference, child, direction);
            }
        }
    }

    /// A member (or list member, map key or value) reference in `location`.
    fn member(&mut self, reference: &'a Json, location: &str, direction: Direction) {
        let target = self.target(reference);
        let kind = target
            .and_then(|shape| shape.get("type"))
            .and_then(Json::as_str)
            .unwrap_or("-");
        let prefix = ["member", self.protocol, direction.as_str(), kind, location];
        self.add(&prefix);
        for (key, _) in entries(Some(reference)) {
            let structural = key == "shape" || key == "location";
            if !structural && !self.ignored.reference.contains(key) {
                self.classified(&prefix, kind, key);
            }
        }
        self.shape(reference, direction);
    }

    /// A trait `key` on a shape of type `kind`: under `prefix` when it is a
    /// wire trait, by shape type alone otherwise.
    fn classified(&mut self, prefix: &[&str], kind: &str, key: &str) {
        if WIRE.contains(&key) {
            let mut parts = prefix.to_vec();
            parts.push(key);
            self.add(&parts);
        } else {
            self.add(&["trait", kind, key]);
        }
    }
}

/// The features of an `endpoint-rule-set-1.json`.
pub(crate) fn rule_set_features(json: &Json, out: &mut Features) {
    for (_, parameter) in entries(json.get("parameters")) {
        for (key, value) in entries(Some(parameter)) {
            let text = value.as_str().unwrap_or("-");
            match key.as_str() {
                "documentation" => {}
                "type" => add(out, &["rules", "param", &text.to_lowercase()]),
                "builtIn" => add(out, &["rules", "builtIn", text]),
                _ => add(out, &["rules", "param-key", key]),
            }
        }
    }
    if let Some(rules) = json.get("rules") {
        rule_features(rules, out);
    }
}

/// Every function, rule type and endpoint form under `value`.
fn rule_features(value: &Json, out: &mut Features) {
    match value {
        Json::Array(items) => items.iter().for_each(|item| rule_features(item, out)),
        Json::Object(object) => {
            if let Some(function) = object.get("fn").and_then(Json::as_str) {
                add(out, &["rules", "fn", function]);
            }
            let kind = object.get("type").and_then(Json::as_str);
            if let (Some(kind), true) = (kind, object.contains_key("conditions")) {
                add(out, &["rules", "rule", kind]);
            }
            if let Some(endpoint) = object.get("endpoint").filter(|value| value.is_object()) {
                endpoint_features(endpoint, out);
            }
            object.values().for_each(|item| rule_features(item, out));
        }
        _ => {}
    }
}

fn endpoint_features(endpoint: &Json, out: &mut Features) {
    for (key, _) in entries(Some(endpoint)) {
        add(out, &["rules", "endpoint", key]);
    }
    let properties = endpoint.get("properties");
    for (key, _) in entries(properties) {
        add(out, &["rules", "property", key]);
    }
    let schemes = properties.and_then(|properties| properties.get("authSchemes"));
    for scheme in schemes.and_then(Json::as_array).into_iter().flatten() {
        for (key, value) in entries(Some(scheme)) {
            if key == "name" {
                add(out, &["rules", "authScheme", value.as_str().unwrap_or("-")]);
            } else {
                add(out, &["rules", "authScheme-key", key]);
            }
        }
    }
}

/// The features of a `paginators-1.json`: each key with its value's kind,
/// and the expression forms its strings use.
pub(crate) fn paginator_features(json: &Json, out: &mut Features) {
    for (_, paginator) in entries(json.get("pagination")) {
        for (key, value) in entries(Some(paginator)) {
            add(out, &["pages", key, json_kind(value)]);
            let texts: Vec<&str> = match value {
                Json::String(text) => vec![text],
                _ => strings(Some(value)).collect(),
            };
            for text in texts {
                if text.contains("||") {
                    add(out, &["pages", key, "or"]);
                }
                if text.contains('[') {
                    add(out, &["pages", key, "index"]);
                } else if text.contains('.') {
                    add(out, &["pages", key, "dotted"]);
                }
            }
        }
    }
}

/// The features of a `waiters-2.json`: waiter keys, each matcher with each
/// state it ends in, and the kind of value it expects.
pub(crate) fn waiter_features(json: &Json, out: &mut Features) {
    for (_, waiter) in entries(json.get("waiters")) {
        for (key, _) in entries(Some(waiter)) {
            add(out, &["wait", "key", key]);
        }
        for acceptor in waiter
            .get("acceptors")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            let field = |key: &str| acceptor.get(key).and_then(Json::as_str).unwrap_or("-");
            let matcher = field("matcher");
            add(out, &["wait", matcher, field("state")]);
            let expected = acceptor.get("expected").map_or("-", json_kind);
            add(out, &["wait", matcher, "expected", expected]);
            for (key, _) in entries(Some(acceptor)) {
                if !["matcher", "state", "expected", "argument"].contains(&&**key) {
                    add(out, &["wait", "acceptor-key", key]);
                }
            }
        }
    }
}
