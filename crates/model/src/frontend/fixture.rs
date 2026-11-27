//! A test-only front-end: a [`SchemaSet`] from a Smithy 2.0 JSON AST, read
//! with botocore's meaning, through the [`SchemaSetBuilder`] calls the C2J
//! front-end makes and nothing else of it. It shows the builder interface is
//! format-agnostic: hand-written twins of protocol-suite models project as
//! their C2J originals do (`tests.rs`).
//!
//! It translates the traits its twins use, the flags and the HTTP bindings.
//! A known trait it does not translate is refused ([`FixtureError`]), never
//! dropped; an unknown one is kept cold and recorded as uninterpreted, as
//! the C2J front-end records an unknown key; an ignored one (documentation,
//! `@input`, `@idempotent`) is dropped, as C2J drops its ignored keys.
//! What botocore reads beside the schema comes from an [`Overlay`], the
//! stand-in for `cli-overlay-1.json`.
use crate::schema::projection::Botocore;
use crate::schema::{
    BuildError, ColdTraits, DraftId, FLAGS, KnownTraits, MemberTraits, OperationData,
    OperationShapes, PRELUDE_NAMESPACE, SchemaSet, SchemaSetBuilder, ServiceData, ShapeType, Site,
    Storage, TraitId, traits,
};
use serde_json::{Map, Value as Json};
use std::collections::HashMap;

type Object = Map<String, Json>;

/// The front-end's name, as `ModelFrontEnd::name` will give it.
pub const NAME: &str = "fixture";

/// Why a model cannot be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixtureError(pub String);

impl std::fmt::Display for FixtureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FixtureError {}

impl From<BuildError> for FixtureError {
    fn from(error: BuildError) -> Self {
        Self(error.to_string())
    }
}

fn fail<T>(message: impl Into<String>) -> Result<T, FixtureError> {
    Err(FixtureError(message.into()))
}

fn text<'a>(value: &'a Json, what: &str) -> Result<&'a str, FixtureError> {
    value
        .as_str()
        .map_or_else(|| fail(format!("{what}: not a string")), Ok)
}

fn object<'a>(value: &'a Json, what: &str) -> Result<&'a Object, FixtureError> {
    value
        .as_object()
        .map_or_else(|| fail(format!("{what}: not an object")), Ok)
}

fn texts(value: &Json, what: &str) -> Result<Box<[Box<str>]>, FixtureError> {
    let items = value
        .as_array()
        .map_or_else(|| fail(format!("{what}: not a list")), Ok)?;
    items
        .iter()
        .map(|item| text(item, what).map(Box::from))
        .collect()
}

/// An optional text field of an object.
fn field(object: &Object, key: &str) -> Result<Option<Box<str>>, FixtureError> {
    object
        .get(key)
        .map(|value| text(value, key).map(Box::from))
        .transpose()
}

/// A shape's traits, in document order.
fn traits_of(shape: &Object) -> impl Iterator<Item = (&String, &Json)> {
    shape
        .get("traits")
        .and_then(Json::as_object)
        .into_iter()
        .flatten()
}

fn short(id: &str) -> &str {
    id.split_once('#').map_or(id, |(_, name)| name)
}

fn type_of(shape: &Json) -> &str {
    shape.get("type").and_then(Json::as_str).unwrap_or_default()
}

fn kind(shape: &Object) -> &str {
    shape.get("type").and_then(Json::as_str).unwrap_or_default()
}

fn target(reference: &Json, what: &str) -> Result<String, FixtureError> {
    let target = reference
        .get("target")
        .map_or_else(|| fail(format!("{what}: no target")), Ok)?;
    text(target, what).map(str::to_owned)
}

/// The botocore facts a Smithy model has no trait for (DESIGN §4.3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overlay {
    pub protocol: Option<Box<str>>,
    pub protocols: Option<Box<[Box<str>]>>,
    pub json_version: Option<Box<str>>,
    pub target_prefix: Option<Box<str>>,
    pub signature_version: Option<Box<str>>,
    pub uid: Option<Box<str>>,
}

impl Overlay {
    pub fn parse(bytes: &[u8]) -> Result<Self, FixtureError> {
        let json: Json = serde_json::from_slice(bytes).map_err(|e| FixtureError(e.to_string()))?;
        let object = object(&json, "overlay")?;
        let field = |key: &str| object.get(key).map(|value| text(value, key).map(Box::from));
        if let Some(key) = object
            .keys()
            .find(|key| !Self::KEYS.contains(&key.as_str()))
        {
            return fail(format!("overlay: unknown key {key}"));
        }
        Ok(Self {
            protocol: field("protocol").transpose()?,
            protocols: object
                .get("protocols")
                .map(|v| texts(v, "protocols"))
                .transpose()?,
            json_version: field("jsonVersion").transpose()?,
            target_prefix: field("targetPrefix").transpose()?,
            signature_version: field("signatureVersion").transpose()?,
            uid: field("uid").transpose()?,
        })
    }

    const KEYS: [&str; 6] = [
        "protocol",
        "protocols",
        "jsonVersion",
        "targetPrefix",
        "signatureVersion",
        "uid",
    ];

    pub fn botocore(&self) -> Botocore<'_> {
        Botocore {
            protocol: self.protocol.as_deref(),
            protocols: self.protocols.as_deref(),
            json_version: self.json_version.as_deref(),
            target_prefix: self.target_prefix.as_deref(),
            signature_version: self.signature_version.as_deref(),
            uid: self.uid.as_deref(),
        }
    }
}

/// `ModelFrontEnd::load(FrontEndInput::Bytes)` for a Smithy JSON AST: the
/// schemas, with no botocore metadata (the [`Overlay`] carries it).
pub fn load(bytes: &[u8]) -> Result<SchemaSet, FixtureError> {
    let ast: Json = serde_json::from_slice(bytes).map_err(|e| FixtureError(e.to_string()))?;
    build(&ast)
}

/// A named shape's own traits, which its references overlay.
struct Named {
    draft: DraftId,
    known: KnownTraits,
    cold: ColdTraits,
}

/// One reference's or shape's translated traits.
#[derive(Default)]
struct Translated {
    known: KnownTraits,
    cold: ColdTraits,
}

struct Walk<'a> {
    builder: SchemaSetBuilder,
    named: HashMap<&'a str, Named>,
    /// Whether body names are `xmlName` (awsQuery, ec2Query, restXml) rather
    /// than `jsonName`: botocore has one `locationName` per member.
    xml: bool,
    /// Whether an operation without `@http` is botocore's `POST /`.
    rpc: bool,
}

/// The schemas of a Smithy 2.0 JSON AST holding one service.
pub fn build(ast: &Json) -> Result<SchemaSet, FixtureError> {
    if ast.get("smithy").and_then(Json::as_str) != Some("2.0") {
        return fail("not a Smithy 2.0 JSON AST");
    }
    let shapes = object(ast.get("shapes").unwrap_or(&Json::Null), "shapes")?;
    let mut services = shapes
        .iter()
        .filter(|(_, shape)| type_of(shape) == "service");
    let Some((service_id, service)) = services.next() else {
        return fail("no service shape");
    };
    if services.next().is_some() {
        return fail("more than one service shape");
    }
    let Some((namespace, _)) = service_id.split_once('#') else {
        return fail(format!("{service_id}: not a shape id"));
    };
    let mut walk = Walk {
        builder: SchemaSetBuilder::new(namespace)?,
        named: HashMap::new(),
        xml: false,
        rpc: false,
    };
    let data = walk.service(object(service, service_id)?)?;
    walk.builder.set_service(data);
    let data_shapes = || {
        shapes
            .iter()
            .filter(|(_, shape)| !matches!(type_of(shape), "service" | "operation"))
    };
    for (id, shape) in data_shapes() {
        walk.declare(id, object(shape, id)?)?;
    }
    for (id, shape) in data_shapes() {
        walk.children(id, object(shape, id)?)?;
    }
    let bound = match service.get("operations") {
        None => Vec::new(),
        Some(Json::Array(items)) => items
            .iter()
            .map(|item| target(item, "operations"))
            .collect::<Result<_, _>>()?,
        Some(_) => return fail("operations: not a list"),
    };
    let unbound = shapes
        .iter()
        .find(|(id, shape)| type_of(shape) == "operation" && !bound.contains(id));
    if let Some((id, _)) = unbound {
        return fail(format!("{id}: not bound to the service"));
    }
    for id in &bound {
        let shape = shapes
            .get(id)
            .map_or_else(|| fail(format!("no operation {id}")), Ok)?;
        walk.operation(id, object(shape, id)?)?;
    }
    Ok(walk.builder.finish()?)
}

/// A known trait this front-end has no translation for.
fn known_untranslated(id: &str) -> Result<bool, FixtureError> {
    match TraitId::parse(id) {
        None => Ok(false),
        Some(known) if known.storage() == Storage::Ignored => Ok(true),
        Some(_) => fail(format!("{id}: not translated by the fixture front-end")),
    }
}

impl<'a> Walk<'a> {
    fn intern(&mut self, value: &Json, what: &str) -> Result<crate::schema::StrId, FixtureError> {
        Ok(self.builder.intern(text(value, what)?)?)
    }

    fn service(&mut self, service: &Object) -> Result<ServiceData, FixtureError> {
        let mut data = ServiceData {
            version: field(service, "version")?,
            ..ServiceData::default()
        };
        let mut protocols = Vec::new();
        for (id, value) in traits_of(service) {
            match id.as_str() {
                "aws.api#service" => self.aws_service(object(value, id)?, &mut data)?,
                "aws.auth#sigv4" => data.sigv4_name = field(object(value, id)?, "name")?,
                "smithy.api#auth" => data.auth = Some(texts(value, id)?),
                _ => match TraitId::parse(id) {
                    Some(protocol) if PROTOCOLS.contains(&protocol) => protocols.push(protocol),
                    _ if !known_untranslated(id)? => {
                        self.builder.record_uninterpreted(Site::Service, id);
                    }
                    _ => {}
                },
            }
        }
        // Smithy's effective auth without `@auth`: the service's schemes.
        if data.auth.is_none()
            && service
                .get("traits")
                .is_some_and(|t| t.get("aws.auth#sigv4").is_some())
        {
            data.auth = Some(Box::new(["aws.auth#sigv4".into()]));
        }
        let xml = [traits::AWS_QUERY, traits::EC2_QUERY, traits::REST_XML];
        self.xml = protocols.iter().any(|id| xml.contains(id));
        if self.xml && protocols.iter().any(|id| !xml.contains(id)) {
            return fail("both XML and JSON protocols: botocore has one locationName per member");
        }
        let rest = [traits::REST_JSON_1, traits::REST_XML];
        self.rpc = protocols.iter().any(|id| !rest.contains(id));
        protocols.sort();
        protocols.dedup();
        data.protocol_traits = protocols.into();
        Ok(data)
    }

    /// `aws.api#service`: the fields botocore reads, and its unknown ones
    /// as uninterpreted.
    fn aws_service(&mut self, fields: &Object, data: &mut ServiceData) -> Result<(), FixtureError> {
        data.sdk_id = field(fields, "sdkId")?;
        data.endpoint_prefix = field(fields, "endpointPrefix")?;
        let read = ["sdkId", "endpointPrefix"];
        for key in fields.keys().filter(|key| !read.contains(&key.as_str())) {
            let key = format!("aws.api#service.{key}");
            self.builder.record_uninterpreted(Site::Service, &key);
        }
        Ok(())
    }

    /// Declares a named shape with its own traits; an enum's member values
    /// are its `enum` list.
    fn declare(&mut self, id: &'a str, shape: &Object) -> Result<(), FixtureError> {
        let (shape_type, type_name) = match kind(shape) {
            "blob" => (ShapeType::Blob, None),
            "boolean" => (ShapeType::Boolean, None),
            "string" | "enum" => (ShapeType::String, None),
            // botocore has no enum integers: an intEnum's values are dropped.
            "integer" | "intEnum" => (ShapeType::Integer, None),
            "long" => (ShapeType::Long, None),
            "float" => (ShapeType::Float, None),
            "double" => (ShapeType::Double, None),
            "timestamp" => (ShapeType::Timestamp, None),
            "list" => (ShapeType::List, None),
            "map" => (ShapeType::Map, None),
            "structure" => (ShapeType::Structure, None),
            "union" => (ShapeType::Union, Some("structure")),
            other => return fail(format!("{id}: shape type {other:?} not translated")),
        };
        let name = short(id);
        let mut own = self.traits(shape.get("traits"), &Site::Shape(name.into()), "", None)?;
        let members = shape.get("members").and_then(Json::as_object);
        if kind(shape) == "enum" {
            let mut values = Vec::new();
            for (member, definition) in members.into_iter().flatten() {
                let value = definition.pointer("/traits/smithy.api#enumValue");
                values.push(match value {
                    Some(value) => self.intern(value, "enumValue")?,
                    None => self.builder.intern(member)?,
                });
            }
            own.known.enum_values = Some(values.into());
        }
        let draft = self.builder.declare_id(id, shape_type, type_name)?;
        self.builder
            .set_traits(draft, own.known.clone(), own.cold.clone())?;
        let (known, cold) = (own.known, own.cold);
        self.named.insert(id, Named { draft, known, cold });
        Ok(())
    }

    fn children(&mut self, id: &str, shape: &Object) -> Result<(), FixtureError> {
        let parent = self.named[id].draft;
        let name = short(id);
        let child = |key: &str| {
            shape
                .get(key)
                .map_or_else(|| fail(format!("{id}: no {key}")), Ok)
        };
        match kind(shape) {
            "structure" | "union" => {
                let members = shape.get("members").map(|m| object(m, id)).transpose()?;
                for (member, reference) in members.into_iter().flatten() {
                    let at = format!("members.{member}.");
                    let (target, traits) = self.reference(name, &at, Some(member), reference)?;
                    self.builder.add_member(parent, member, target, traits)?;
                }
            }
            "list" => {
                let member = self.reference(name, "member.", None, child("member")?)?;
                self.builder.set_list_member(parent, member.0, member.1)?;
            }
            "map" => {
                let key = self.reference(name, "key.", None, child("key")?)?;
                let value = self.reference(name, "value.", None, child("value")?)?;
                self.builder.set_map_entry(parent, key, value)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// A member's target and its traits over the target's: Smithy's merge,
    /// sharing the target's when the member changes nothing.
    fn reference(
        &mut self,
        owner: &str,
        at: &str,
        member: Option<&str>,
        reference: &Json,
    ) -> Result<(DraftId, MemberTraits), FixtureError> {
        let target = target(reference, owner)?;
        let over = self.traits(
            reference.get("traits"),
            &Site::Shape(owner.into()),
            at,
            member,
        )?;
        let (draft, known, cold) = if target.starts_with(&format!("{PRELUDE_NAMESPACE}#")) {
            let draft = self.builder.prelude(short(&target));
            let draft = draft.map_or_else(|| fail(format!("no prelude shape {target}")), Ok)?;
            (draft, KnownTraits::default(), ColdTraits::default())
        } else {
            let named = self.named.get(target.as_str());
            let named = named.map_or_else(|| fail(format!("Shape not found: {target}")), Ok)?;
            (named.draft, named.known.clone(), named.cold.clone())
        };
        let merged = known.overlaid(&over.known);
        if merged == known && over.cold.is_empty() {
            return Ok((draft, MemberTraits::Inherit));
        }
        Ok((
            draft,
            MemberTraits::resolved(merged, cold.overlaid(&over.cold)),
        ))
    }

    /// The traits of a shape or member, at `site` with key prefix `at`.
    fn traits(
        &mut self,
        traits: Option<&Json>,
        site: &Site,
        at: &str,
        member: Option<&str>,
    ) -> Result<Translated, FixtureError> {
        let mut out = Translated::default();
        let Some(traits) = traits else {
            return Ok(out);
        };
        for (id, value) in object(traits, at)? {
            if !self.apply(id, value, member, &mut out.known)? && !known_untranslated(id)? {
                let key = format!("{at}{id}");
                out.cold
                    .insert_unknown(id, value.clone(), site.clone(), &key)?;
            }
        }
        Ok(out)
    }

    /// Sets the slot `id` fills; false for an id with no translation here.
    fn apply(
        &mut self,
        id: &str,
        value: &Json,
        member: Option<&str>,
        known: &mut KnownTraits,
    ) -> Result<bool, FixtureError> {
        let flag = FLAGS.iter().find(|(_, flag)| flag.as_str() == id);
        if let Some((flag, _)) = flag.filter(|_| !id.starts_with("c2j#")) {
            known.flags.set(*flag, true);
            return Ok(true);
        }
        if self.binding(id, value, member, known)? {
            return Ok(true);
        }
        match id {
            "smithy.api#jsonName" | "smithy.api#xmlName" => {
                // The other family's name is unread, as botocore never sees it.
                if (id == "smithy.api#xmlName") == self.xml && known.location.is_none() {
                    let name = Some(self.intern(value, id)?);
                    (known.location_name, known.json_name, known.xml_name) = (name, name, name);
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Sets the HTTP binding `id` gives; false for another id.
    fn binding(
        &mut self,
        id: &str,
        value: &Json,
        member: Option<&str>,
        known: &mut KnownTraits,
    ) -> Result<bool, FixtureError> {
        let (location, name) = match id {
            "smithy.api#httpHeader" => ("header", Some(text(value, id)?)),
            "smithy.api#httpPrefixHeaders" => ("headers", Some(text(value, id)?)),
            "smithy.api#httpQuery" => ("querystring", Some(text(value, id)?)),
            "smithy.api#httpQueryParams" => ("querystring", None),
            "smithy.api#httpLabel" => ("uri", member),
            "smithy.api#httpResponseCode" => ("statusCode", None),
            _ => return Ok(false),
        };
        self.locate(known, location, name)?;
        Ok(true)
    }

    /// An HTTP binding: botocore's raw `location`, and its `locationName`
    /// as every name slot, whatever the location (DESIGN §4.2).
    fn locate(
        &mut self,
        known: &mut KnownTraits,
        location: &str,
        name: Option<&str>,
    ) -> Result<(), FixtureError> {
        known.location = Some(self.builder.intern(location)?);
        if let Some(name) = name {
            let name = Some(self.builder.intern(name)?);
            (known.location_name, known.json_name, known.xml_name) = (name, name, name);
        }
        Ok(())
    }

    fn operation(&mut self, id: &str, shape: &Object) -> Result<(), FixtureError> {
        let name = short(id);
        let site = Site::Operation(name.into());
        let mut data = OperationData {
            name: name.into(),
            wire_name: Some(name.into()),
            ..OperationData::default()
        };
        for (trait_id, value) in traits_of(shape) {
            let fields = || object(value, trait_id);
            match trait_id.as_str() {
                "smithy.api#http" => {
                    data.http_method = field(fields()?, "method")?;
                    data.http_request_uri = field(fields()?, "uri")?;
                }
                _ if !known_untranslated(trait_id)? => {
                    self.builder.record_uninterpreted(site.clone(), trait_id);
                }
                _ => {}
            }
        }
        // botocore's RPC operations are `POST /`; Smithy writes no `@http`.
        if data.http_method.is_none() && self.rpc {
            (data.http_method, data.http_request_uri) = (Some("POST".into()), Some("/".into()));
        }
        let mut shapes = OperationShapes::default();
        for (key, slot) in [("input", &mut shapes.input), ("output", &mut shapes.output)] {
            if let Some(reference) = shape.get(key) {
                *slot = self.root(&target(reference, id)?)?;
            }
        }
        for error in shape
            .get("errors")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            let root = self.root(&target(error, id)?)?;
            shapes
                .errors
                .push(root.map_or_else(|| fail(format!("{id}: a Unit error")), Ok)?);
        }
        self.builder.add_operation(data, shapes)?;
        Ok(())
    }

    /// An operation reference's root; `Unit` is none. Smithy puts no traits
    /// on these references, so a root is its target.
    fn root(&self, target: &str) -> Result<Option<DraftId>, FixtureError> {
        if target == format!("{PRELUDE_NAMESPACE}#Unit") {
            return Ok(None);
        }
        let named = self.named.get(target);
        named.map_or_else(
            || fail(format!("Shape not found: {target}")),
            |n| Ok(Some(n.draft)),
        )
    }
}

/// The protocol traits a service may carry.
const PROTOCOLS: [TraitId; 7] = [
    traits::AWS_JSON_1_0,
    traits::AWS_JSON_1_1,
    traits::REST_JSON_1,
    traits::REST_XML,
    traits::AWS_QUERY,
    traits::EC2_QUERY,
    traits::RPC_V2_CBOR,
];

#[cfg(test)]
mod tests;
