//! The `query` protocol: botocore's `QuerySerializer` (a form-encoded POST
//! body) and `QueryParser` (an XML response).
use super::common::{self, Family};
use super::{ParseOptions, Protocol, Serialized};
use crate::error::{ParseError, SerializeError, ServiceError};
use crate::http::{Headers, HttpResponse, Method};
use crate::xml::{self, Element};
use model::pyrepr::{float_from_text, int_from_text, py_repr, py_str};
use model::shapes::{Flag, Model, Operation, ShapeId, ShapeKind};
use model::value::{Map, Value};

/// The `query` protocol.
#[derive(Debug)]
pub struct Query;

impl Protocol for Query {
    fn name(&self) -> &'static str {
        "query"
    }

    /// `QuerySerializer.serialize_to_request`.
    fn serialize(
        &self,
        model: &Model,
        operation: &Operation,
        input: &Value,
    ) -> Result<Serialized, SerializeError> {
        let version = model
            .metadata()
            .api_version
            .as_deref()
            .ok_or_else(|| SerializeError::new("'apiVersion'"))?;
        let mut form = Form::default();
        form.set("Action", operation.name.to_string());
        form.set("Version", version.to_owned());
        if let Some(shape) = operation.input {
            form.value(model, input, shape, "")?;
        }
        let mut headers = Headers::new();
        headers.set(
            "Content-Type",
            "application/x-www-form-urlencoded; charset=utf-8",
        );
        Ok(Serialized {
            method: method(operation)?,
            path: "/".into(),
            query: String::new(),
            headers,
            body: form.encoded().into_bytes(),
            host_prefix: common::host_prefix(model, operation, input)?,
        })
    }

    fn parse(
        &self,
        model: &Model,
        operation: &Operation,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Value, ParseError> {
        common::parse(self, model, operation.output, response, options)
    }

    fn parse_error(
        &self,
        model: &Model,
        operation: &Operation,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<ServiceError, ParseError> {
        common::parse_error(self, model, operation, response, options)
    }
}

/// `http.method`, `POST` by default.
pub(super) fn method(operation: &Operation) -> Result<Method, SerializeError> {
    let name = operation.http.method.as_deref().unwrap_or("POST");
    Method::parse(name).ok_or_else(|| SerializeError::new(format!("unsupported method {name}")))
}

/// The form's fields in insertion order; setting a field again replaces
/// its value in place, as the serializer's dict does.
#[derive(Default)]
struct Form(Vec<(String, String)>);

impl Form {
    fn set(&mut self, name: &str, value: String) {
        match self.0.iter_mut().find(|(other, _)| other == name) {
            Some((_, old)) => *old = value,
            None => self.0.push((name.to_owned(), value)),
        }
    }

    /// The body on the wire: `AWSRequestPreparer._prepare_body`'s
    /// `urlencode(params, doseq=True)` of the serializer's dict.
    fn encoded(&self) -> String {
        let pairs: Vec<String> = self
            .0
            .iter()
            .map(|(name, value)| format!("{}={}", quote_plus(name), quote_plus(value)))
            .collect();
        pairs.join("&")
    }

    /// `QuerySerializer._serialize` of `value` at `prefix`.
    fn value(
        &mut self,
        model: &Model,
        value: &Value,
        shape: ShapeId,
        prefix: &str,
    ) -> Result<(), SerializeError> {
        let resolved = model.shape(shape);
        match &resolved.kind {
            ShapeKind::Structure { .. } => self.structure(model, value, shape, prefix),
            ShapeKind::List { member } => self.list(model, value, shape, *member, prefix),
            ShapeKind::Map { key, value: item } => {
                self.map(model, value, shape, (*key, *item), prefix)
            }
            ShapeKind::Blob => {
                self.set(prefix, common::base64_in(value)?);
                Ok(())
            }
            ShapeKind::Timestamp => {
                let format = resolved
                    .traits
                    .timestamp_format
                    .as_deref()
                    .unwrap_or("iso8601");
                self.set(prefix, py_str(&common::timestamp_out(value, format)?));
                Ok(())
            }
            ShapeKind::Boolean => {
                let text = if truthy(value) { "true" } else { "false" };
                self.set(prefix, text.into());
                Ok(())
            }
            ShapeKind::Float | ShapeKind::Double => {
                self.set(prefix, py_str(&common::special_float(value)));
                Ok(())
            }
            _ => {
                self.set(prefix, py_str(value));
                Ok(())
            }
        }
    }

    fn structure(
        &mut self,
        model: &Model,
        value: &Value,
        shape: ShapeId,
        prefix: &str,
    ) -> Result<(), SerializeError> {
        let Value::Map(map) = value else {
            return Err(SerializeError::new(format!(
                "'{}' object has no attribute 'items'",
                common::short_type(value)
            )));
        };
        for (key, item) in map.iter() {
            let member = common::member(model, shape, key)?;
            let name = model.shape(member).traits.name.as_deref().unwrap_or(key);
            let member_prefix = if prefix.is_empty() {
                name.to_owned()
            } else {
                format!("{prefix}.{name}")
            };
            self.value(model, item, member, &member_prefix)?;
        }
        Ok(())
    }

    /// `_serialize_type_list`: `Name=` when empty; flattened lists take the
    /// member's name in place of the last part of the prefix.
    fn list(
        &mut self,
        model: &Model,
        value: &Value,
        shape: ShapeId,
        member: ShapeId,
        prefix: &str,
    ) -> Result<(), SerializeError> {
        let items = match value {
            Value::List(items) => items.as_slice(),
            _ => return Err(SerializeError::new("a list is required")),
        };
        if items.is_empty() {
            self.set(prefix, String::new());
            return Ok(());
        }
        let member_name = model.shape(member).traits.name.as_deref();
        let list_prefix = if model.shape(shape).traits.is(Flag::Flattened) {
            match member_name {
                Some(name) => match prefix.rsplit_once('.') {
                    Some((head, _)) => format!("{head}.{name}"),
                    None => name.to_owned(),
                },
                None => prefix.to_owned(),
            }
        } else {
            format!("{prefix}.{}", member_name.unwrap_or("member"))
        };
        for (index, item) in items.iter().enumerate() {
            self.value(model, item, member, &format!("{list_prefix}.{}", index + 1))?;
        }
        Ok(())
    }

    /// `_serialize_type_map`: `entry.N.key` and `entry.N.value`, or without
    /// `entry` when flattened, with the key and value shapes' names.
    fn map(
        &mut self,
        model: &Model,
        value: &Value,
        shape: ShapeId,
        (key_shape, value_shape): (ShapeId, ShapeId),
        prefix: &str,
    ) -> Result<(), SerializeError> {
        let Value::Map(map) = value else {
            return Err(SerializeError::new("a dict is required"));
        };
        let full = if model.shape(shape).traits.is(Flag::Flattened) {
            prefix.to_owned()
        } else {
            format!("{prefix}.entry")
        };
        let key_name = model
            .shape(key_shape)
            .traits
            .name
            .as_deref()
            .unwrap_or("key");
        let value_name = model
            .shape(value_shape)
            .traits
            .name
            .as_deref()
            .unwrap_or("value");
        for (index, (key, item)) in map.iter().enumerate() {
            let number = index + 1;
            let key_value = Value::String(key.to_owned());
            self.value(
                model,
                &key_value,
                key_shape,
                &format!("{full}.{number}.{key_name}"),
            )?;
            self.value(
                model,
                item,
                value_shape,
                &format!("{full}.{number}.{value_name}"),
            )?;
        }
        Ok(())
    }
}

/// Python truthiness, as `if value:` reads it.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Integer(number) => *number != 0,
        Value::BigInteger(_) | Value::Timestamp(_) => true,
        Value::Float(number) => *number != 0.0,
        Value::String(text) => !text.is_empty(),
        Value::Blob(bytes) => !bytes.is_empty(),
        Value::List(items) => !items.is_empty(),
        Value::Map(map) => !map.is_empty(),
    }
}

/// `urllib.parse.quote_plus` with no extra safe characters: UTF-8, a
/// space as `+`, and every byte but letters, digits and `-_.~` as `%XX`.
fn quote_plus(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else if byte == b' ' {
            encoded.push('+');
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// `_build_name_to_xml_node`'s value: one element, or every element with a
/// repeated name.
#[derive(Clone, Debug)]
enum Node<'a> {
    One(&'a Element),
    Many(Vec<&'a Element>),
}

impl<'a> Node<'a> {
    /// The element whose children a mapping is built from: the first of a
    /// repeated name.
    fn first(&self) -> &'a Element {
        match self {
            Self::One(element) => element,
            Self::Many(elements) => elements[0],
        }
    }
}

/// `_build_name_to_xml_node`: children by local name in document order, a
/// repeated name collecting its elements.
fn by_name<'a>(node: &Node<'a>) -> Vec<(String, Node<'a>)> {
    let mut mapping: Vec<(String, Node<'_>)> = Vec::new();
    for child in &node.first().children {
        let name = child.local_name().to_owned();
        match mapping.iter_mut().find(|(other, _)| *other == name) {
            Some((_, Node::Many(elements))) => elements.push(child),
            Some((_, slot)) => *slot = Node::Many(vec![slot.first(), child]),
            None => mapping.push((name, Node::One(child))),
        }
    }
    mapping
}

fn lookup<'m, 'a>(mapping: &'m [(String, Node<'a>)], name: &str) -> Option<&'m Node<'a>> {
    mapping
        .iter()
        .find(|(other, _)| other == name)
        .map(|(_, node)| node)
}

/// A body as XML, or `ResponseParserError`'s text for one that is not.
fn document(body: &[u8]) -> Result<Element, ParseError> {
    xml::parse(body).map_err(|error| {
        let shown = py_repr(&Value::Blob(body.to_vec()));
        ParseError::new(format!(
            "Unable to parse response ({error}), invalid XML received. Further retries may \
             succeed:\n{shown}"
        ))
    })
}

struct Reader<'m, 'o> {
    model: &'m Model,
    options: &'o ParseOptions<'o>,
}

impl Reader<'_, '_> {
    /// `_parse_shape`.
    fn shape(&self, shape: ShapeId, node: &Node<'_>) -> Result<Value, ParseError> {
        match &self.model.shape(shape).kind {
            ShapeKind::Structure { .. } => self.structure(shape, node).map(Value::Map),
            ShapeKind::List { member } => self.list(shape, *member, node),
            ShapeKind::Map { key, value } => self.map(shape, (*key, *value), node),
            kind => self.scalar(kind, node),
        }
    }

    /// `_handle_structure`: members by their XML names, attributes for
    /// `xmlAttribute` members, header members skipped.
    fn structure(&self, shape: ShapeId, node: &Node<'_>) -> Result<Map, ParseError> {
        let model = self.model;
        let node = if model.shape(shape).traits.is(Flag::Exception) {
            error_root(node)
        } else {
            node.clone()
        };
        let mapping = by_name(&node);
        let present: Vec<&str> = mapping.iter().map(|(name, _)| name.as_str()).collect();
        if let Some(tag) = common::unknown_union_member(model, shape, &present)? {
            return Ok(common::unknown_member_value(tag));
        }
        let mut parsed = Map::new();
        for member in model.members(shape) {
            let traits = &model.shape(member.shape).traits;
            if traits.location.is_some() || traits.is(Flag::EventHeader) {
                continue;
            }
            let name = xml_name(model, member.shape, &member.name);
            if let Some(child) = lookup(&mapping, name) {
                parsed.insert(member.name.to_string(), self.shape(member.shape, child)?);
            } else if traits.is(Flag::XmlAttribute)
                && let Some(value) =
                    attribute(node.first(), traits.name.as_deref().unwrap_or_default())
            {
                parsed.insert(member.name.to_string(), Value::String(value));
            }
        }
        Ok(parsed)
    }

    /// `_handle_list`: a flattened list's repeated elements, else the
    /// element's children.
    fn list(&self, shape: ShapeId, member: ShapeId, node: &Node<'_>) -> Result<Value, ParseError> {
        let flattened = self.model.shape(shape).traits.is(Flag::Flattened);
        let items: Vec<&Element> = match node {
            Node::Many(elements) => elements.clone(),
            Node::One(element) if flattened => vec![*element],
            Node::One(element) => element.children.iter().collect(),
        };
        let parsed = items
            .into_iter()
            .map(|item| self.shape(member, &Node::One(item)))
            .collect::<Result<_, _>>()?;
        Ok(Value::List(parsed))
    }

    /// `_handle_map`: each entry's key and value elements by their names; a
    /// pair missing one keeps the previous pair's, as the loop's variables
    /// do.
    fn map(
        &self,
        shape: ShapeId,
        (key_shape, value_shape): (ShapeId, ShapeId),
        node: &Node<'_>,
    ) -> Result<Value, ParseError> {
        let model = self.model;
        let key_name = model
            .shape(key_shape)
            .traits
            .name
            .as_deref()
            .unwrap_or("key");
        let value_name = model
            .shape(value_shape)
            .traits
            .name
            .as_deref()
            .unwrap_or("value");
        let entries: Vec<&Element> = match node {
            Node::Many(elements) => elements.clone(),
            Node::One(element) if model.shape(shape).traits.is(Flag::Flattened) => vec![*element],
            Node::One(element) => element.children.iter().collect(),
        };
        let mut parsed = Map::new();
        let (mut key, mut value) = (None, None);
        for entry in entries {
            for pair in &entry.children {
                let tag = pair.local_name();
                if tag == key_name {
                    key = Some(self.shape(key_shape, &Node::One(pair))?);
                } else if tag == value_name {
                    value = Some(self.shape(value_shape, &Node::One(pair))?);
                } else {
                    return Err(ParseError::new(format!("Unknown tag: {tag}")));
                }
            }
            let (Some(Value::String(name)), Some(item)) = (&key, &value) else {
                return Err(ParseError::new("cannot access local variable 'key_name'"));
            };
            parsed.insert(name.clone(), item.clone());
        }
        Ok(Value::Map(parsed))
    }

    /// The scalar handlers, each reading the element's text (`''` when it
    /// has none).
    fn scalar(&self, kind: &ShapeKind, node: &Node<'_>) -> Result<Value, ParseError> {
        let Node::One(element) = node else {
            return Err(ParseError::new(
                "expected one element, found a repeated one",
            ));
        };
        let text = element.text.as_deref().unwrap_or("");
        let invalid = |what: &str| {
            ParseError::new(format!("{what}: {}", py_repr(&Value::String(text.into()))))
        };
        match kind {
            ShapeKind::Boolean => Ok(Value::Bool(text == "true")),
            ShapeKind::Float | ShapeKind::Double => float_from_text(text)
                .map(Value::Float)
                .ok_or_else(|| invalid("could not convert string to float")),
            ShapeKind::Integer | ShapeKind::Long => {
                int_from_text(text).ok_or_else(|| invalid("invalid literal for int() with base 10"))
            }
            ShapeKind::Timestamp => (self.options.timestamp)(&Value::String(text.into())),
            ShapeKind::Blob => (self.options.blob)(text),
            _ => Ok(Value::String(text.into())),
        }
    }
}

/// `_member_key_name`: a flattened list's member name, else the member's
/// `locationName`, else its name.
fn xml_name<'m>(model: &'m Model, shape: ShapeId, member: &'m str) -> &'m str {
    let resolved = model.shape(shape);
    if let ShapeKind::List { member: item } = resolved.kind
        && resolved.traits.is(Flag::Flattened)
        && let Some(name) = model.shape(item).traits.name.as_deref()
    {
        return name;
    }
    resolved.traits.name.as_deref().unwrap_or(member)
}

/// An `xmlAttribute` member's value: the attribute whose name, with its
/// namespace replaced by the location name's prefix, is the location name.
fn attribute(element: &Element, location: &str) -> Option<String> {
    let prefix = location.split(':').next().unwrap_or_default();
    element.attributes.iter().find_map(|(name, value)| {
        let renamed = match (name.find('{'), name.rfind('}')) {
            (Some(start), Some(end)) => format!("{}{prefix}:{}", &name[..start], &name[end + 1..]),
            _ => name.clone(),
        };
        (renamed == location).then(|| value.clone())
    })
}

/// `_get_error_root`: an `ErrorResponse`'s `Error` child.
fn error_root<'a>(node: &Node<'a>) -> Node<'a> {
    let root = node.first();
    if root.local_name() == "ErrorResponse"
        && let Some(error) = root
            .children
            .iter()
            .find(|child| child.local_name() == "Error")
    {
        return Node::One(error);
    }
    node.clone()
}

/// `_replace_nodes`: an element with children as a dict of them, any other
/// as its text.
fn replaced(mapping: Vec<(String, Node<'_>)>) -> Map {
    let mut parsed = Map::new();
    for (name, node) in mapping {
        let value = match &node {
            Node::One(element) if element.children.is_empty() => {
                element.text.clone().map_or(Value::Null, Value::String)
            }
            _ => Value::Map(replaced(by_name(&node))),
        };
        parsed.insert(name, value);
    }
    parsed
}

impl Family for Query {
    /// `_parse_body_as_xml` with the metadata injected.
    fn do_parse(
        &self,
        model: &Model,
        shape: Option<ShapeId>,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Value, ParseError> {
        let root = document(&response.body)?;
        let mut parsed = match shape {
            Some(shape) => body_shape(model, shape, &root, options)?,
            None => Map::new(),
        };
        if let Some(metadata) = lookup(&by_name(&Node::One(&root)), "ResponseMetadata") {
            let fields = by_name(metadata)
                .into_iter()
                .map(|(name, node)| {
                    (
                        name,
                        node.first().text.clone().map_or(Value::Null, Value::String),
                    )
                })
                .collect();
            parsed.insert("ResponseMetadata".into(), Value::Map(fields));
        }
        Ok(Value::Map(parsed))
    }

    /// `QueryParser._do_error_parse`.
    fn do_error_parse(&self, response: &HttpResponse) -> Result<Map, ParseError> {
        let root = document(&response.body)?;
        let mut parsed = replaced(by_name(&Node::One(&root)));
        if let Some(errors) = parsed.remove("Errors") {
            let Value::Map(errors) = errors else {
                return Err(ParseError::new(
                    "dictionary update sequence element #0 has length 1; 2 is required",
                ));
            };
            for (key, value) in errors {
                parsed.insert(key, value);
            }
        }
        if let Some(id) = parsed.remove("RequestId") {
            let mut metadata = Map::new();
            metadata.insert("RequestId".into(), id);
            parsed.insert("ResponseMetadata".into(), Value::Map(metadata));
        }
        Ok(parsed)
    }

    /// `_parse_body_as_xml` without the metadata.
    fn do_modeled_error_parse(
        &self,
        model: &Model,
        shape: ShapeId,
        response: &HttpResponse,
        options: &ParseOptions<'_>,
    ) -> Result<Map, ParseError> {
        body_shape(model, shape, &document(&response.body)?, options)
    }
}

/// The shape parsed from the root, or from the `resultWrapper` element.
fn body_shape(
    model: &Model,
    shape: ShapeId,
    root: &Element,
    options: &ParseOptions<'_>,
) -> Result<Map, ParseError> {
    let reader = Reader { model, options };
    let start = match model.shape(shape).traits.result_wrapper.as_deref() {
        Some(wrapper) => lookup(&by_name(&Node::One(root)), wrapper)
            .cloned()
            .ok_or_else(|| ParseError::new(format!("'{wrapper}'")))?,
        None => Node::One(root),
    };
    match reader.shape(shape, &start)? {
        Value::Map(map) => Ok(map),
        other => Err(ParseError::new(format!(
            "expected a structure, parsed {}",
            py_str(&other)
        ))),
    }
}

#[cfg(test)]
mod tests;
