//! `XmlCodec`: the XML responses botocore's `BaseXMLResponseParser` reads.
//! A body parses into the ElementTree tree [`crate::xml`] builds, with
//! botocore's `ResponseParserError` text for one that is not XML, and an
//! [`XmlDeserializer`] walks it the way `_parse_shape` does: a
//! structure's members by their XML names, a list's or map's entries as
//! its children (or the repeated elements themselves when flattened), and
//! a scalar's text converted as the query parser converts it.
//!
//! The ElementTree helpers botocore's XML parsers share,
//! `_build_name_to_xml_node` ([`by_name`]) and `_replace_nodes`
//! ([`replaced`]), live here too. There is no serializer: rest-xml is not
//! implemented.
use super::value::build_value;
use crate::protocol::ParseOptions;
use crate::xml::{self, Element};
use model::pyrepr::{float_from_text, int_from_text, py_repr};
use model::schema::{Flag, Schema, ShapeType};
use model::serde::{
    Codec, FinishSerializer, MapSerializer, Read, SerdeError, SerdeResult, SerializableStruct,
    ShapeDeserializer, ShapeSerializer,
};
use model::value::{Key, Map, Value};

/// The XML codec. It reads only.
#[derive(Debug)]
pub struct XmlCodec;

/// The one [`XmlCodec`].
pub static XML: XmlCodec = XmlCodec;

impl Codec for XmlCodec {
    fn media_type(&self) -> &'static str {
        "text/xml"
    }

    fn serializer(&self) -> Box<dyn FinishSerializer + '_> {
        Box::new(Unwritable)
    }

    fn deserializer<'a>(
        &self,
        body: &'a [u8],
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, SerdeError> {
        Ok(Box::new(Document(parse(body)?)))
    }
}

/// `_parse_xml_string_to_dom`: the body's tree, or `ResponseParserError`'s
/// text with expat's message and the body as bytes.
///
/// # Errors
/// The body is not well-formed XML.
pub fn parse(body: &[u8]) -> Result<Element, SerdeError> {
    xml::parse(body).map_err(|error| {
        let shown = py_repr(&Value::Blob(body.to_vec()));
        SerdeError::new(format!(
            "Unable to parse response ({error}), invalid XML received. Further retries may \
             succeed:\n{shown}"
        ))
    })
}

/// `_build_name_to_xml_node`'s value: one element, or every element with a
/// repeated name.
#[derive(Clone, Debug)]
pub enum Node<'a> {
    One(&'a Element),
    Many(Vec<&'a Element>),
}

impl<'a> Node<'a> {
    /// The element whose children a mapping is built from: the first of a
    /// repeated name.
    pub fn first(&self) -> &'a Element {
        match self {
            Self::One(element) => element,
            Self::Many(elements) => elements[0],
        }
    }

    /// The elements a list or map reads: a repeated name's, the element
    /// itself when `flattened`, else its children.
    fn entries(&self, flattened: bool) -> Vec<&'a Element> {
        match self {
            Self::Many(elements) => elements.clone(),
            Self::One(element) if flattened => vec![*element],
            Self::One(element) => element.children.iter().collect(),
        }
    }
}

/// `_build_name_to_xml_node`: `element`'s children by local name in
/// document order, a repeated name collecting its elements.
pub fn by_name(element: &Element) -> Vec<(String, Node<'_>)> {
    let mut mapping: Vec<(String, Node<'_>)> = Vec::new();
    for child in &element.children {
        let name = child.local_name().to_owned();
        match mapping.iter_mut().find(|(other, _)| *other == name) {
            Some((_, Node::Many(elements))) => elements.push(child),
            Some((_, slot)) => *slot = Node::Many(vec![slot.first(), child]),
            None => mapping.push((name, Node::One(child))),
        }
    }
    mapping
}

/// The node `mapping` holds under `name`.
pub fn lookup<'m, 'a>(mapping: &'m [(String, Node<'a>)], name: &str) -> Option<&'m Node<'a>> {
    mapping
        .iter()
        .find(|(other, _)| other == name)
        .map(|(_, node)| node)
}

/// `_replace_nodes`: an element with children as a dict of them, any other
/// as its text.
pub fn replaced(mapping: Vec<(String, Node<'_>)>) -> Map {
    let mut parsed = Map::new();
    for (name, node) in mapping {
        let value = match &node {
            Node::One(element) if element.children.is_empty() => {
                element.text.clone().map_or(Value::Null, Value::String)
            }
            _ => Value::Map(replaced(by_name(node.first()))),
        };
        parsed.insert(name, value);
    }
    parsed
}

/// A parsed body that owns its tree, read from its root.
struct Document(Element);

impl ShapeDeserializer for Document {
    fn read_struct(
        &mut self,
        schema: Schema<'_>,
        each: &mut dyn FnMut(Schema<'_>, &mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError> {
        XmlDeserializer::new(Node::One(&self.0)).read_struct(schema, each)
    }

    fn read_list(
        &mut self,
        schema: Schema<'_>,
        each: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError> {
        XmlDeserializer::new(Node::One(&self.0)).read_list(schema, each)
    }

    fn read_map(
        &mut self,
        schema: Schema<'_>,
        each: &mut dyn FnMut(Key, &mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError> {
        XmlDeserializer::new(Node::One(&self.0)).read_map(schema, each)
    }

    fn read_scalar(&mut self, schema: Schema<'_>) -> Result<Value, SerdeError> {
        XmlDeserializer::new(Node::One(&self.0)).read_scalar(schema)
    }

    fn read_document(&mut self, schema: Schema<'_>) -> Result<Value, SerdeError> {
        XmlDeserializer::new(Node::One(&self.0)).read_document(schema)
    }

    fn is_null(&self) -> bool {
        false
    }

    fn union_tags(&self) -> Option<Vec<Key>> {
        XmlDeserializer::new(Node::One(&self.0)).union_tags()
    }
}

/// Where a deserializer stands: a node of the tree, or the text of an
/// `xmlAttribute` member's attribute.
#[derive(Clone, Debug)]
enum At<'a> {
    Node(Node<'a>),
    Attribute(&'a str),
}

/// A cursor over a parsed tree, at the node a value is read from. XML
/// holds no `null`.
#[derive(Clone, Debug)]
pub struct XmlDeserializer<'a> {
    at: At<'a>,
}

impl<'a> XmlDeserializer<'a> {
    pub const fn new(node: Node<'a>) -> Self {
        Self { at: At::Node(node) }
    }

    /// The node a structure, list or map reads; an attribute holds only
    /// text.
    fn node(&self) -> Result<&Node<'a>, SerdeError> {
        match &self.at {
            At::Node(node) => Ok(node),
            At::Attribute(_) => Err(SerdeError::new(
                "an XML attribute holds text, not a structure, list or map",
            )),
        }
    }
}

impl ShapeDeserializer for XmlDeserializer<'_> {
    /// `_handle_structure`: members by their XML names, attributes for
    /// `xmlAttribute` members, header and event-header members skipped; an
    /// error shape reads from an `ErrorResponse`'s `Error`.
    fn read_struct(
        &mut self,
        schema: Schema<'_>,
        each: &mut dyn FnMut(Schema<'_>, &mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError> {
        let node = self.node()?;
        let root = if schema.flag(Flag::Error) {
            error_root(node.first())
        } else {
            node.first()
        };
        let mapping = by_name(root);
        for member in schema.members() {
            if member.location().is_some() || member.flag(Flag::EventHeader) {
                continue;
            }
            if let Some(child) = lookup(&mapping, xml_name(member)) {
                each(member, &mut XmlDeserializer::new(child.clone()))?;
            } else if member.flag(Flag::XmlAttribute)
                && let Some(text) = attribute(root, member.location_name().unwrap_or_default())
            {
                let mut at = XmlDeserializer {
                    at: At::Attribute(text),
                };
                each(member, &mut at)?;
            }
        }
        Ok(Read::Present)
    }

    /// `_handle_list`: a flattened list's repeated elements, else the
    /// element's children.
    fn read_list(
        &mut self,
        schema: Schema<'_>,
        each: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError> {
        let items = self.node()?.entries(schema.flag(Flag::XmlFlattened));
        for item in items {
            each(&mut XmlDeserializer::new(Node::One(item)))?;
        }
        Ok(Read::Present)
    }

    /// `_handle_map`: each entry's key and value elements by their names.
    /// The loop's `key_name` and `val_name` outlive an entry, so one that
    /// lacks a key or value keeps the previous entry's; each value element
    /// parses when the loop reaches it, so its failure comes before a
    /// later unknown tag's.
    fn read_map(
        &mut self,
        schema: Schema<'_>,
        each: &mut dyn FnMut(Key, &mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError> {
        let (Some(key_shape), Some(value_shape)) = (schema.map_key(), schema.map_value()) else {
            return Err(SerdeError::new(format!(
                "the map {} has no key or value",
                schema.target().id().name()
            )));
        };
        let key_name = key_shape.location_name().unwrap_or("key");
        let value_name = value_shape.location_name().unwrap_or("value");
        let entries = self.node()?.entries(schema.flag(Flag::XmlFlattened));
        // `None` before any key; `Some(None)` for a key that is no `str`.
        let mut key: Option<Option<String>> = None;
        let mut value: Option<&Element> = None;
        for entry in entries {
            let mut values = Vec::new();
            let mut failure = None;
            for pair in &entry.children {
                let tag = pair.local_name();
                if tag == key_name {
                    match key_text(key_shape, pair) {
                        Ok(text) => key = Some(text),
                        Err(error) => {
                            failure = Some(error);
                            break;
                        }
                    }
                } else if tag == value_name {
                    values.push(pair);
                    value = Some(pair);
                } else {
                    failure = Some(SerdeError::new(format!("Unknown tag: {tag}")));
                    break;
                }
            }
            let name = match (&key, failure) {
                (Some(Some(name)), None) if value.is_some() => Key::from(name.as_str()),
                (_, failure) => {
                    // The entry fails, after the values before the failure parse.
                    for pair in values {
                        each(Key::from(""), &mut XmlDeserializer::new(Node::One(pair)))?;
                    }
                    return Err(failure.unwrap_or_else(|| {
                        SerdeError::new("cannot access local variable 'key_name'")
                    }));
                }
            };
            if values.is_empty() {
                values.extend(value);
            }
            for pair in values {
                each(name.clone(), &mut XmlDeserializer::new(Node::One(pair)))?;
            }
        }
        Ok(Read::Present)
    }

    /// The scalar handlers, each reading the element's text (`''` when it
    /// has none). A timestamp's or blob's text is raw, for the parse
    /// hooks; an attribute's text is every type's value as it is.
    fn read_scalar(&mut self, schema: Schema<'_>) -> Result<Value, SerdeError> {
        match &self.at {
            At::Attribute(text) => Ok(Value::String((*text).to_owned())),
            At::Node(Node::One(element)) => scalar(schema.shape_type(), element),
            At::Node(Node::Many(_)) => Err(SerdeError::new(
                "expected one element, found a repeated one",
            )),
        }
    }

    /// A document is a `structure` to the XML parsers: its members as
    /// [`ShapeDeserializer::read_struct`] finds them, a timestamp or blob
    /// among them as written (the hooks are the caller's).
    fn read_document(&mut self, schema: Schema<'_>) -> Result<Value, SerdeError> {
        let mut parsed = Map::new();
        self.read_struct(schema, &mut |member, deserializer| {
            let name = member.member_name().unwrap_or_default().to_owned();
            parsed.insert(name, build_value(member, deserializer, &AS_WRITTEN)?);
            Ok(())
        })?;
        Ok(Value::Map(parsed))
    }

    fn is_null(&self) -> bool {
        false
    }

    /// The local names of the element's children, each once, in document
    /// order.
    fn union_tags(&self) -> Option<Vec<Key>> {
        let At::Node(node) = &self.at else {
            return None;
        };
        let tags = by_name(node.first())
            .into_iter()
            .map(|(name, _)| Key::from(name.as_str()))
            .collect();
        Some(tags)
    }
}

/// Parse hooks that keep a timestamp's or blob's text.
const AS_WRITTEN: ParseOptions<'static> = ParseOptions {
    timestamp: &as_written,
    blob: &as_written,
};

#[expect(
    clippy::unnecessary_wraps,
    reason = "the parse hooks' signature is fallible"
)]
fn as_written(value: &Value) -> Result<Value, crate::error::ParseError> {
    Ok(value.clone())
}

/// The scalar handler of `shape_type` over `element`'s text.
fn scalar(shape_type: ShapeType, element: &Element) -> Result<Value, SerdeError> {
    let text = element.text.as_deref().unwrap_or("");
    match shape_type {
        ShapeType::Boolean => Ok(Value::Bool(text == "true")),
        ShapeType::Float | ShapeType::Double => {
            float_from_text(text).map(Value::Float).ok_or_else(|| {
                SerdeError::new(format!(
                    "could not convert string to float: {}",
                    py_repr(&Value::String(text.into()))
                ))
            })
        }
        ShapeType::Integer | ShapeType::Long => int_from_text(text).map_err(SerdeError::new),
        _ => Ok(Value::String(text.into())),
    }
}

/// A map key element's text, `None` for a key shape whose handler gives no
/// `str`.
fn key_text(shape: Schema<'_>, element: &Element) -> Result<Option<String>, SerdeError> {
    let shape_type = shape.shape_type();
    if matches!(
        shape_type,
        ShapeType::Structure
            | ShapeType::Union
            | ShapeType::Document
            | ShapeType::List
            | ShapeType::Map
    ) {
        return Ok(None);
    }
    Ok(match scalar(shape_type, element)? {
        Value::String(text) => Some(text),
        _ => None,
    })
}

/// `_member_key_name`: a flattened list's member name, else the member's
/// `locationName`, else its name.
fn xml_name(member: Schema<'_>) -> &str {
    if member.shape_type() == ShapeType::List
        && member.flag(Flag::XmlFlattened)
        && let Some(name) = member.list_member().and_then(Schema::location_name)
    {
        return name;
    }
    member
        .location_name()
        .or_else(|| member.member_name())
        .unwrap_or_default()
}

/// An `xmlAttribute` member's value: the attribute whose name, with its
/// namespace replaced by the location name's prefix, is the location name.
fn attribute<'a>(element: &'a Element, location: &str) -> Option<&'a str> {
    let prefix = location.split(':').next().unwrap_or_default();
    element.attributes.iter().find_map(|(name, value)| {
        let renamed = match (name.find('{'), name.rfind('}')) {
            (Some(start), Some(end)) => format!("{}{prefix}:{}", &name[..start], &name[end + 1..]),
            _ => name.clone(),
        };
        (renamed == location).then_some(value.as_str())
    })
}

/// `_get_error_root`: an `ErrorResponse`'s `Error` child.
fn error_root(root: &Element) -> &Element {
    if root.local_name() == "ErrorResponse"
        && let Some(error) = root
            .children
            .iter()
            .find(|child| child.local_name() == "Error")
    {
        return error;
    }
    root
}

/// The serializer an [`XmlCodec`] gives: rest-xml is not implemented, so
/// every write fails.
struct Unwritable;

impl Unwritable {
    #[expect(clippy::unused_self, reason = "every write refuses the same way")]
    fn refuse(&self) -> SerdeResult {
        Err(SerdeError::new("the XML codec does not serialize"))
    }
}

macro_rules! refuse_scalars {
    ($($name:ident),+) => {$(
        fn $name(&mut self, _: Schema<'_>, _: &Value) -> SerdeResult {
            self.refuse()
        }
    )+};
}

impl ShapeSerializer for Unwritable {
    fn write_struct(&mut self, _: Schema<'_>, _: &dyn SerializableStruct) -> SerdeResult {
        self.refuse()
    }

    fn write_list(
        &mut self,
        _: Schema<'_>,
        _: usize,
        _: &mut dyn FnMut(&mut dyn ShapeSerializer) -> SerdeResult,
    ) -> SerdeResult {
        self.refuse()
    }

    fn write_map(
        &mut self,
        _: Schema<'_>,
        _: usize,
        _: &mut dyn FnMut(&mut dyn MapSerializer) -> SerdeResult,
    ) -> SerdeResult {
        self.refuse()
    }

    refuse_scalars!(
        write_boolean,
        write_byte,
        write_short,
        write_integer,
        write_long,
        write_float,
        write_double,
        write_big_integer,
        write_big_decimal,
        write_string,
        write_blob,
        write_timestamp,
        write_document
    );

    fn write_null(&mut self, _: Schema<'_>) -> SerdeResult {
        self.refuse()
    }
}

impl FinishSerializer for Unwritable {
    fn finish(self: Box<Self>) -> Result<Vec<u8>, SerdeError> {
        Err(SerdeError::new("the XML codec does not serialize"))
    }
}

#[cfg(test)]
mod tests;
