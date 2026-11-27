//! `RestXMLSerializer._serialize`: the body's element tree, built as
//! botocore's `ElementTree.SubElement` calls build it, then written as
//! `ElementTree.tostring(root, encoding='utf-8')` writes it.
//!
//! A structure's members are elements under their `locationName` (else
//! their name), in the caller's order, an `xmlAttribute` member an
//! attribute of the structure instead; a list's items are elements under
//! the member's `locationName` (else `member`), or, flattened, repeated
//! elements under the list's own name in its parent; a map's entries are
//! `entry` elements (or, flattened, repeated elements under the map's
//! name), each with its key and value under the key's and value's
//! `locationName` (else `key` and `value`). Every element whose shape has
//! an `xmlNamespace` gets its `xmlns` (or `xmlns:prefix`) attribute; a
//! flattened list's goes on its parent.
//!
//! Scalars are text: a boolean `true` or `false` by its truthiness, a blob
//! as base64, a timestamp in its `timestampFormat` (`iso8601` by default),
//! a float or double as its `str()` with the special values spelled out,
//! anything else as its `str()`. Text escapes `&`, `<` and `>`; attribute
//! values `"`, CR, LF and TAB as well; an element with neither text nor
//! children is written `<name />`; a lone surrogate is a decimal character
//! reference (`xmlcharrefreplace`). There is no XML declaration.
use crate::protocol::{base64_in, special_float, timestamp_out};
use crate::python::truthy;
use model::pyrepr::{py_repr, py_short_type_name, py_str};
use model::schema::{Flag, Schema};
use model::serde::{
    FinishSerializer, MapSerializer, SerdeError, SerdeResult, SerializableStruct, ShapeSerializer,
};
use model::value::{Key, Value};

/// An element as `ElementTree` holds it: attribute values as they were
/// set, which the writer refuses unless they are text.
#[derive(Debug)]
struct Node {
    tag: String,
    attributes: Vec<(String, Value)>,
    /// A `str`: [`Value::String`] or [`Value::PyString`].
    text: Option<Value>,
    children: Vec<Self>,
}

impl Node {
    const fn new(tag: String) -> Self {
        Self {
            tag,
            attributes: Vec::new(),
            text: None,
            children: Vec::new(),
        }
    }

    /// `node.attrib[name] = value`: a name already set keeps its place.
    fn set(&mut self, name: String, value: Value) {
        match self.attributes.iter_mut().find(|(other, _)| *other == name) {
            Some((_, slot)) => *slot = value,
            None => self.attributes.push((name, value)),
        }
    }
}

/// How the elements written into a frame are named.
#[derive(Debug)]
enum Names {
    /// A structure's: each member's `locationName`, else its name.
    Members,
    /// A list's items or a map entry's value: one name for each.
    Items(String),
    /// A map's entries: each an element named `entry` (the map's name when
    /// flattened) holding the key and value elements.
    Entries {
        entry: String,
        key: String,
        key_namespace: Option<(String, String)>,
        value: String,
    },
}

/// An open structure, list or map.
#[derive(Debug)]
struct Frame {
    /// The element its children go into; `None` for a flattened list or
    /// map, whose elements go into the parent's.
    node: Option<Node>,
    names: Names,
    /// `_serialize_type_structure` returns at a member set to `None`: the
    /// members after it are not written.
    stopped: bool,
}

/// `RestXMLSerializer._serialize_body_params`: the root element is named by
/// the `locationName` of the shape first written (the input's or the
/// payload member's), and [`FinishSerializer::finish`] writes it.
#[derive(Debug, Default)]
pub struct XmlSerializer {
    frames: Vec<Frame>,
    root: Option<Node>,
}

/// `shape.serialization['name']`, whose `KeyError` names the key.
fn location_name(schema: Schema<'_>) -> Result<&str, SerdeError> {
    schema
        .location_name()
        .ok_or_else(|| SerdeError::new("'name'"))
}

/// `_add_xml_namespace`'s attribute: `xmlns`, or `xmlns:prefix` with a
/// prefix, set to the `uri`.
fn namespace(schema: Schema<'_>) -> Result<Option<(String, String)>, SerdeError> {
    let Some(namespace) = schema.xml_namespace() else {
        return Ok(None);
    };
    let uri = namespace.uri.ok_or_else(|| SerdeError::new("'uri'"))?;
    let name = match namespace.prefix.filter(|prefix| !prefix.is_empty()) {
        Some(prefix) => format!("xmlns:{prefix}"),
        None => "xmlns".to_owned(),
    };
    Ok(Some((name, uri.to_owned())))
}

fn with_namespace(mut node: Node, schema: Schema<'_>) -> Result<Node, SerdeError> {
    if let Some((name, uri)) = namespace(schema)? {
        node.set(name, Value::String(uri));
    }
    Ok(node)
}

/// `str(value)`, a `str` kept as it is.
fn text_of(value: &Value) -> Value {
    match value {
        Value::String(_) | Value::PyString(_) => value.clone(),
        other => Value::String(py_str(other)),
    }
}

impl XmlSerializer {
    /// The name the next element written takes: the root's `locationName`,
    /// or what the open frame names its children.
    fn name(&self, schema: Schema<'_>) -> Result<String, SerdeError> {
        let Some(frame) = self.frames.last() else {
            return location_name(schema).map(str::to_owned);
        };
        Ok(match &frame.names {
            Names::Members => schema
                .location_name()
                .or_else(|| schema.member_name())
                .unwrap_or_default()
                .to_owned(),
            Names::Items(name) => name.clone(),
            Names::Entries { .. } => {
                return Err(SerdeError::new("a map value written outside its entry"));
            }
        })
    }

    /// Whether the open structure stopped at a `None` member.
    fn stopped(&self) -> bool {
        self.frames.last().is_some_and(|frame| frame.stopped)
    }

    /// The element children go into: the innermost open one.
    fn target(&mut self) -> Option<&mut Node> {
        self.frames
            .iter_mut()
            .rev()
            .find_map(|frame| frame.node.as_mut())
    }

    /// A finished element, into its parent or as the root.
    fn attach(&mut self, node: Node) {
        match self.target() {
            Some(parent) => parent.children.push(node),
            None => self.root = Some(node),
        }
    }

    /// The `xmlAttribute` member's name, when `schema` is one.
    fn attribute(&self, schema: Schema<'_>) -> Result<Option<String>, SerdeError> {
        let members = matches!(
            self.frames.last(),
            Some(Frame {
                names: Names::Members,
                ..
            })
        );
        if members && schema.flag(Flag::XmlAttribute) {
            return location_name(schema).map(|name| Some(name.to_owned()));
        }
        Ok(None)
    }

    /// A scalar element: `text` (from the value unless the member is an
    /// attribute or `None`) and the shape's namespace.
    fn scalar(
        &mut self,
        schema: Schema<'_>,
        value: &Value,
        text: impl FnOnce(&Value) -> Result<Value, SerdeError>,
    ) -> SerdeResult {
        if self.stopped() {
            return Ok(());
        }
        if *value == Value::Null
            && matches!(self.frames.last(), Some(frame) if matches!(frame.names, Names::Members))
        {
            if let Some(frame) = self.frames.last_mut() {
                frame.stopped = true;
            }
            return Ok(());
        }
        if let Some(name) = self.attribute(schema)? {
            if let Some(node) = self.frames.last_mut().and_then(|frame| frame.node.as_mut()) {
                node.set(name, value.clone());
            }
            return Ok(());
        }
        let mut node = Node::new(self.name(schema)?);
        node.text = Some(text(value)?);
        let node = with_namespace(node, schema)?;
        self.attach(node);
        Ok(())
    }

    /// An aggregate that is an `xmlAttribute` member: `ElementTree` cannot
    /// write it.
    fn refuse_attribute(&self, schema: Schema<'_>, kind: &str) -> SerdeResult {
        match self.attribute(schema)? {
            Some(_) => Err(SerdeError::new(format!(
                "cannot serialize a {kind} as an XML attribute"
            ))),
            None => Ok(()),
        }
    }

    /// `frame` open while `write` fills it, then its element attached.
    fn nested(
        &mut self,
        frame: Frame,
        write: impl FnOnce(&mut Self) -> SerdeResult,
    ) -> SerdeResult {
        self.frames.push(frame);
        write(self)?;
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| SerdeError::new("an unbalanced XML element"))?;
        if let Some(node) = frame.node {
            self.attach(node);
        }
        Ok(())
    }
}

impl MapSerializer for XmlSerializer {
    /// One `_serialize_type_map` entry: its element, the key's element as
    /// the key's `str()` with the key's namespace, then the value's.
    fn entry(
        &mut self,
        key: &Key,
        value: &mut dyn FnMut(&mut dyn ShapeSerializer) -> SerdeResult,
    ) -> SerdeResult {
        let Some(Frame {
            names:
                Names::Entries {
                    entry,
                    key: key_name,
                    key_namespace,
                    value: value_name,
                },
            ..
        }) = self.frames.last()
        else {
            return Err(SerdeError::new("a map entry outside a map"));
        };
        let mut node = Node::new(entry.clone());
        let mut key_node = Node::new(key_name.clone());
        key_node.text = Some(text_of(&key.to_value()));
        if let Some((name, uri)) = key_namespace {
            key_node.set(name.clone(), Value::String(uri.clone()));
        }
        node.children.push(key_node);
        let frame = Frame {
            node: Some(node),
            names: Names::Items(value_name.clone()),
            stopped: false,
        };
        self.nested(frame, |this| value(this))
    }
}

impl ShapeSerializer for XmlSerializer {
    /// `_serialize_type_structure`: the element and its namespace, then the
    /// members in the caller's order.
    fn write_struct(&mut self, schema: Schema<'_>, value: &dyn SerializableStruct) -> SerdeResult {
        if self.stopped() {
            return Ok(());
        }
        self.refuse_attribute(schema, "structure")?;
        let node = with_namespace(Node::new(self.name(schema)?), schema)?;
        let frame = Frame {
            node: Some(node),
            names: Names::Members,
            stopped: false,
        };
        self.nested(frame, |this| value.serialize_members(this))
    }

    /// `_serialize_type_list`: the items in a list element named for the
    /// member, or, flattened, each named as the list is, in the parent,
    /// which takes the list's namespace.
    fn write_list(
        &mut self,
        schema: Schema<'_>,
        _len: usize,
        items: &mut dyn FnMut(&mut dyn ShapeSerializer) -> SerdeResult,
    ) -> SerdeResult {
        if self.stopped() {
            return Ok(());
        }
        self.refuse_attribute(schema, "list")?;
        let name = self.name(schema)?;
        let frame = if schema.flag(Flag::XmlFlattened) {
            if let Some((attribute, uri)) = namespace(schema)?
                && let Some(parent) = self.target()
            {
                parent.set(attribute, Value::String(uri));
            }
            Frame {
                node: None,
                names: Names::Items(name),
                stopped: false,
            }
        } else {
            let item = schema.list_member().and_then(Schema::location_name);
            Frame {
                node: Some(with_namespace(Node::new(name), schema)?),
                names: Names::Items(item.unwrap_or("member").to_owned()),
                stopped: false,
            }
        };
        self.nested(frame, |this| items(this))
    }

    /// `_serialize_type_map`: a map element with its namespace holding an
    /// `entry` per entry or, flattened, an element named as the map is per
    /// entry, in the parent.
    fn write_map(
        &mut self,
        schema: Schema<'_>,
        _len: usize,
        entries: &mut dyn FnMut(&mut dyn MapSerializer) -> SerdeResult,
    ) -> SerdeResult {
        if self.stopped() {
            return Ok(());
        }
        self.refuse_attribute(schema, "map")?;
        let (Some(key), Some(value)) = (schema.map_key(), schema.map_value()) else {
            return Err(SerdeError::new(format!(
                "the map {} has no key or value",
                schema.target().id().name()
            )));
        };
        let name = self.name(schema)?;
        let flattened = schema.flag(Flag::XmlFlattened);
        let (node, entry) = if flattened {
            (None, name)
        } else {
            let node = with_namespace(Node::new(name), schema)?;
            (Some(node), "entry".to_owned())
        };
        let names = Names::Entries {
            entry,
            key: key.location_name().unwrap_or("key").to_owned(),
            key_namespace: namespace(key)?,
            value: value.location_name().unwrap_or("value").to_owned(),
        };
        let frame = Frame {
            node,
            names,
            stopped: false,
        };
        self.nested(frame, |this| entries(this))
    }

    /// `_serialize_type_boolean`: `true` when the value is truthy.
    fn write_boolean(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        self.scalar(schema, value, |value| {
            let text = if truthy(value) { "true" } else { "false" };
            Ok(Value::String(text.into()))
        })
    }

    fn write_byte(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        self.write_string(schema, value)
    }

    fn write_short(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        self.write_string(schema, value)
    }

    fn write_integer(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        self.write_string(schema, value)
    }

    fn write_long(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        self.write_string(schema, value)
    }

    /// `_serialize_type_float`: `str()`, the special values spelled out.
    fn write_float(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        self.scalar(schema, value, |value| Ok(text_of(&special_float(value))))
    }

    /// `_serialize_type_double`, as a float.
    fn write_double(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        self.write_float(schema, value)
    }

    fn write_big_integer(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        self.write_string(schema, value)
    }

    fn write_big_decimal(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        self.write_string(schema, value)
    }

    /// `_default_serialize`: `str(value)`.
    fn write_string(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        self.scalar(schema, value, |value| Ok(text_of(value)))
    }

    /// `_serialize_type_blob`: base64, text encoded as UTF-8 first.
    fn write_blob(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        self.scalar(schema, value, |value| {
            base64_in(value)
                .map(Value::String)
                .map_err(|error| SerdeError::new(error.message))
        })
    }

    /// `_serialize_type_timestamp`: in its `timestampFormat`, ISO 8601 by
    /// default.
    fn write_timestamp(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        let format = schema.timestamp_format_text().unwrap_or("iso8601");
        self.scalar(schema, value, |value| {
            timestamp_out(value, format)
                .map(|out| text_of(&out))
                .map_err(|error| SerdeError::new(error.message))
        })
    }

    /// A document has no handler of its own: `_default_serialize`.
    fn write_document(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        self.write_string(schema, value)
    }

    /// A `None` list item or map value, through its shape's handler: a
    /// boolean is `false`, text `None`; the other handlers fail on it.
    fn write_null(&mut self, schema: Schema<'_>) -> SerdeResult {
        use model::schema::ShapeType;
        let failure = match schema.shape_type() {
            ShapeType::Boolean => return self.write_boolean(schema, &Value::Bool(false)),
            ShapeType::Structure | ShapeType::Union | ShapeType::Map => {
                "'NoneType' object has no attribute 'items'"
            }
            ShapeType::List => "'NoneType' object is not iterable",
            ShapeType::Float | ShapeType::Double => "must be real number, not NoneType",
            ShapeType::Blob => "a bytes-like object is required, not 'NoneType'",
            ShapeType::Timestamp => "Invalid timestamp \"None\"",
            _ => return self.write_string(schema, &Value::String("None".into())),
        };
        Err(SerdeError::new(failure))
    }
}

impl FinishSerializer for XmlSerializer {
    /// `ElementTree.tostring(real_root, encoding='utf-8')`.
    fn finish(self: Box<Self>) -> Result<Vec<u8>, SerdeError> {
        if !self.frames.is_empty() {
            return Err(SerdeError::new("an unfinished XML element"));
        }
        let root = self
            .root
            .ok_or_else(|| SerdeError::new("list index out of range"))?;
        let mut out = Vec::new();
        write_node(&mut out, &root)?;
        Ok(out)
    }
}

/// `_serialize_xml` of one element and its children.
fn write_node(out: &mut Vec<u8>, node: &Node) -> SerdeResult {
    out.push(b'<');
    out.extend_from_slice(node.tag.as_bytes());
    for (name, value) in &node.attributes {
        out.push(b' ');
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(b"=\"");
        escape(out, value, true)?;
        out.push(b'"');
    }
    let text = node.text.as_ref().filter(|text| !is_empty_text(text));
    if text.is_none() && node.children.is_empty() {
        out.extend_from_slice(b" />");
        return Ok(());
    }
    out.push(b'>');
    if let Some(text) = text {
        escape(out, text, false)?;
    }
    for child in &node.children {
        write_node(out, child)?;
    }
    out.extend_from_slice(b"</");
    out.extend_from_slice(node.tag.as_bytes());
    out.push(b'>');
    Ok(())
}

fn is_empty_text(text: &Value) -> bool {
    match text {
        Value::String(text) => text.is_empty(),
        Value::PyString(text) => text.as_bytes().is_empty(),
        _ => false,
    }
}

/// `_escape_cdata` (or, for an attribute, `_escape_attrib`) of `text`,
/// encoded as UTF-8 with `xmlcharrefreplace`; anything but a `str` is
/// `_raise_serialization_error`'s.
fn escape(out: &mut Vec<u8>, text: &Value, attribute: bool) -> SerdeResult {
    let points: Box<dyn Iterator<Item = u32> + '_> = match text {
        Value::String(text) => Box::new(text.chars().map(u32::from)),
        Value::PyString(text) => Box::new(pytext::wtf8::code_points(text.as_bytes())),
        other => {
            return Err(SerdeError::new(format!(
                "cannot serialize {} (type {})",
                py_repr(other),
                py_short_type_name(other)
            )));
        }
    };
    for point in points {
        let replacement: Option<&[u8]> = match point {
            0x26 => Some(b"&amp;"),
            0x3c => Some(b"&lt;"),
            0x3e => Some(b"&gt;"),
            0x22 if attribute => Some(b"&quot;"),
            0x0d if attribute => Some(b"&#13;"),
            0x0a if attribute => Some(b"&#10;"),
            0x09 if attribute => Some(b"&#09;"),
            _ => None,
        };
        match (replacement, char::from_u32(point)) {
            (Some(replacement), _) => out.extend_from_slice(replacement),
            (None, Some(character)) => {
                let mut buffer = [0; 4];
                out.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
            }
            // A lone surrogate, which UTF-8 cannot encode.
            (None, None) => out.extend_from_slice(format!("&#{point};").as_bytes()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
