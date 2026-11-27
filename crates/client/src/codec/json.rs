//! The JSON codec: botocore's JSON body over [`model::serde`].
//!
//! The serializer is `JSONSerializer._serialize`: it builds the body's
//! value tree (a structure's members under their `locationName`, a blob as
//! base64, a timestamp in its `timestampFormat`, the special floats as
//! text), which [`dumps`] writes as `json.dumps` does. The `json` and
//! `rest-json` protocols share it.
//!
//! The deserializer is a cursor over the tree `_parse_body_as_json` reads
//! ([`load`]): numbers keep the spelling `pyjson` gives them, and text that
//! is no JSON is `{'message': body}`. `JSONParser` reads values as the
//! wire holds them; `RestJSONParser`'s handlers convert scalars as Python's
//! `int()`, `float()` and `ensure_boolean` do, split a header list's text
//! and read a `jsonvalue` header. Which one reads is the codec's
//! [`Handlers`], fixed on its `static` instance.
use crate::error::ParseError;
use crate::protocol::common;
use model::pyjson::{LoadError, Options};
use model::pyrepr::{
    float_from_py_text, int_from_py_text, int_from_text, py_repr, py_short_type_name, py_str,
    py_str_text,
};
use model::schema::{Flag, Schema, ShapeType};
use model::serde::{
    Codec, FinishSerializer, MapSerializer, Read, SerdeError, SerdeResult, SerializableStruct,
    ShapeDeserializer, ShapeSerializer,
};
use model::value::{Key, Map, Value};

/// The parser whose handlers read a body's values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handlers {
    /// `JSONParser`: `BaseJSONParser`'s, values as the wire holds them.
    Json,
    /// `RestJSONParser`: `BaseRestParser`'s and its own scalar handlers.
    RestJson,
}

/// The JSON codec, with the handlers its deserializer reads through.
#[derive(Debug)]
pub struct JsonCodec {
    handlers: Handlers,
}

/// The `json` protocol's codec.
pub static JSON: JsonCodec = JsonCodec {
    handlers: Handlers::Json,
};

/// The `rest-json` protocol's codec.
pub static REST_JSON: JsonCodec = JsonCodec {
    handlers: Handlers::RestJson,
};

impl JsonCodec {
    /// A cursor at `value`, a parsed body or a value outside it (a header's
    /// text, the status code) that the same handlers read.
    pub const fn cursor<'v>(&self, value: &'v Value) -> JsonCursor<'v> {
        JsonCursor {
            value,
            handlers: self.handlers,
        }
    }
}

impl Codec for JsonCodec {
    fn media_type(&self) -> &'static str {
        "application/json"
    }

    fn serializer(&self) -> Box<dyn FinishSerializer + '_> {
        Box::new(JsonSerializer::default())
    }

    fn deserializer<'a>(
        &self,
        body: &'a [u8],
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, SerdeError> {
        Ok(Box::new(JsonBody {
            tree: load(body)?,
            handlers: self.handlers,
        }))
    }
}

/// `BaseJSONParser._parse_body_as_json`: nothing is `{}`, the body decoded
/// as UTF-8 (whose `UnicodeDecodeError` botocore does not catch), and text
/// `json.loads` refuses is `{'message': body}`. A lone surrogate is kept.
/// Valid text nested past the reader's depth limit fails the call, naming
/// the limit: the request was sent, so the Python route cannot take it
/// over without sending it again.
///
/// # Errors
/// A body that is not UTF-8, or valid JSON past the reader's limit.
pub fn load(body: &[u8]) -> Result<Value, ParseError> {
    if body.is_empty() {
        return Ok(Value::Map(Map::new()));
    }
    let text = common::decoded(body)?;
    match model::pyjson::loads(text) {
        Ok(value) => Ok(value),
        Err(LoadError::Invalid(_)) => {
            let mut map = Map::new();
            map.insert("message".into(), Value::String(text.to_owned()));
            Ok(Value::Map(map))
        }
        Err(LoadError::Limit(limit)) => Err(common::beyond_limit(limit)),
    }
}

/// `json.dumps` of a body's value tree, with botocore's default options.
///
/// # Errors
/// A value `json.dumps` refuses.
pub fn dumps(value: &Value) -> Result<Vec<u8>, SerdeError> {
    model::pyjson::dumps(value, &Options::default(), None)
        .map(String::into_bytes)
        .map_err(|error| SerdeError::new(error.to_string()))
}

/// An open aggregate the serializer is writing into.
#[derive(Debug)]
enum Frame {
    Struct(Map),
    List(Vec<Value>),
    /// A map, and the key of the entry being written.
    Map(Map, Option<Key>),
}

/// `JSONSerializer._serialize`: builds the body's value tree, which
/// [`JsonSerializer::into_value`] yields and [`FinishSerializer::finish`]
/// writes with [`dumps`].
#[derive(Debug, Default)]
pub struct JsonSerializer {
    frames: Vec<Frame>,
    root: Option<Value>,
}

impl JsonSerializer {
    /// The value tree written.
    ///
    /// # Errors
    /// Nothing was written, or an aggregate is still open.
    pub fn into_value(self) -> Result<Value, SerdeError> {
        if !self.frames.is_empty() {
            return Err(SerdeError::new("an unfinished JSON value"));
        }
        self.root
            .ok_or_else(|| SerdeError::new("no JSON value written"))
    }

    /// A written value: under its member's `locationName`, else its name,
    /// at its list position, or under its map key.
    fn put(&mut self, schema: Schema<'_>, value: Value) -> SerdeResult {
        match self.frames.last_mut() {
            None => self.root = Some(value),
            Some(Frame::Struct(map)) => {
                map.insert(common::wire_name(schema).to_owned(), value);
            }
            Some(Frame::List(items)) => items.push(value),
            Some(Frame::Map(map, key)) => {
                let key = key
                    .take()
                    .ok_or_else(|| SerdeError::new("a map value without its key"))?;
                map.insert_key(key, value);
            }
        }
        Ok(())
    }

    /// An aggregate: `frame` open while `write` fills it, then put.
    fn nested(
        &mut self,
        frame: Frame,
        schema: Schema<'_>,
        write: impl FnOnce(&mut Self) -> SerdeResult,
    ) -> SerdeResult {
        self.frames.push(frame);
        write(self)?;
        let value = match self.frames.pop() {
            Some(Frame::Struct(map) | Frame::Map(map, _)) => Value::Map(map),
            Some(Frame::List(items)) => Value::List(items),
            None => return Err(SerdeError::new("an unbalanced JSON value")),
        };
        self.put(schema, value)
    }
}

impl FinishSerializer for JsonSerializer {
    fn finish(self: Box<Self>) -> Result<Vec<u8>, SerdeError> {
        dumps(&self.into_value()?)
    }
}

impl MapSerializer for JsonSerializer {
    fn entry(
        &mut self,
        key: &Key,
        value: &mut dyn FnMut(&mut dyn ShapeSerializer) -> SerdeResult,
    ) -> SerdeResult {
        match self.frames.last_mut() {
            Some(Frame::Map(_, pending)) => *pending = Some(key.clone()),
            _ => return Err(SerdeError::new("a map entry outside a map")),
        }
        value(self)
    }
}

/// The scalar writes `JSONSerializer` passes through as they are.
macro_rules! as_is {
    ($($name:ident),*) => {$(
        fn $name(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
            self.put(schema, value.clone())
        }
    )*};
}

impl ShapeSerializer for JsonSerializer {
    /// `_serialize_type_structure`: members in the caller's order.
    fn write_struct(&mut self, schema: Schema<'_>, value: &dyn SerializableStruct) -> SerdeResult {
        self.nested(Frame::Struct(Map::new()), schema, |this| {
            value.serialize_members(this)
        })
    }

    /// `_serialize_type_list`: a `None` item kept as `null`.
    fn write_list(
        &mut self,
        schema: Schema<'_>,
        len: usize,
        items: &mut dyn FnMut(&mut dyn ShapeSerializer) -> SerdeResult,
    ) -> SerdeResult {
        let frame = Frame::List(Vec::with_capacity(len));
        self.nested(frame, schema, |this| items(this))
    }

    /// `_serialize_type_map`: entries in the caller's order.
    fn write_map(
        &mut self,
        schema: Schema<'_>,
        len: usize,
        entries: &mut dyn FnMut(&mut dyn MapSerializer) -> SerdeResult,
    ) -> SerdeResult {
        let frame = Frame::Map(Map::with_capacity(len), None);
        self.nested(frame, schema, |this| entries(this))
    }

    as_is!(
        write_boolean,
        write_byte,
        write_short,
        write_integer,
        write_long,
        write_big_integer,
        write_big_decimal,
        write_string,
        write_document
    );

    /// `_serialize_type_float`: the special values as text.
    fn write_float(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        self.put(schema, common::special_float(value))
    }

    /// `_serialize_type_double`, as a float.
    fn write_double(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        self.put(schema, common::special_float(value))
    }

    /// `_serialize_type_blob`: base64, text encoded as UTF-8 first.
    fn write_blob(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        let encoded = common::base64_in(value).map_err(|error| SerdeError::new(error.message))?;
        self.put(schema, Value::String(encoded))
    }

    /// `_serialize_type_timestamp`: in its `timestampFormat`, epoch
    /// seconds by default.
    fn write_timestamp(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        let format = schema.timestamp_format_text().unwrap_or("unixtimestamp");
        let out =
            common::timestamp_out(value, format).map_err(|error| SerdeError::new(error.message))?;
        self.put(schema, out)
    }

    fn write_null(&mut self, schema: Schema<'_>) -> SerdeResult {
        self.put(schema, Value::Null)
    }
}

/// A body the codec parsed, which its deserializer owns.
struct JsonBody {
    tree: Value,
    handlers: Handlers,
}

impl JsonBody {
    const fn cursor(&self) -> JsonCursor<'_> {
        JsonCursor {
            value: &self.tree,
            handlers: self.handlers,
        }
    }
}

impl ShapeDeserializer for JsonBody {
    fn read_struct(
        &mut self,
        schema: Schema<'_>,
        each: &mut dyn FnMut(Schema<'_>, &mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError> {
        self.cursor().read_struct(schema, each)
    }
    fn read_list(
        &mut self,
        schema: Schema<'_>,
        each: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError> {
        self.cursor().read_list(schema, each)
    }
    fn read_map(
        &mut self,
        schema: Schema<'_>,
        each: &mut dyn FnMut(Key, &mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError> {
        self.cursor().read_map(schema, each)
    }
    fn read_scalar(&mut self, schema: Schema<'_>) -> Result<Value, SerdeError> {
        self.cursor().read_scalar(schema)
    }
    fn read_document(&mut self, schema: Schema<'_>) -> Result<Value, SerdeError> {
        self.cursor().read_document(schema)
    }
    fn is_null(&self) -> bool {
        self.cursor().is_null()
    }
    fn union_tags(&self) -> Option<Vec<Key>> {
        self.cursor().union_tags()
    }
}

/// A deserializer at one value of a parsed body, reading as its
/// [`Handlers`] do. It hands `null` members to the walk, which drops them.
#[derive(Clone, Copy, Debug)]
pub struct JsonCursor<'v> {
    value: &'v Value,
    handlers: Handlers,
}

impl<'v> JsonCursor<'v> {
    const fn at<'w>(&self, value: &'w Value) -> JsonCursor<'w> {
        JsonCursor {
            value,
            handlers: self.handlers,
        }
    }

    /// `_handle_map`'s key: the wire key read through the key's shape, as
    /// text.
    fn key(&self, schema: Schema<'_>, key: &Key) -> Result<Key, SerdeError> {
        Ok(match self.at(&key.to_value()).read_scalar(schema)? {
            Value::String(key) => Key::Str(key),
            Value::PyString(key) => Key::Py(key),
            other => Key::Str(py_str(&other)),
        })
    }

    /// `RestJSONParser`'s and `BaseRestParser`'s scalar handlers, which
    /// apply to headers and body alike: `_handle_string` (a `jsonvalue`
    /// header), and `_handle_boolean`, `_handle_integer` and
    /// `_handle_float` with their `long` and `double` aliases. `None`
    /// leaves the shape to `BaseJSONParser`.
    fn rest_scalar(&self, schema: Schema<'_>) -> Option<Result<Value, SerdeError>> {
        match schema.shape_type() {
            ShapeType::String => json_value(schema, self.value),
            ShapeType::Boolean => Some(Ok(Value::Bool(ensure_boolean(self.value)))),
            ShapeType::Integer | ShapeType::Long => Some(int(self.value)),
            ShapeType::Float | ShapeType::Double => Some(float(self.value)),
            _ => None,
        }
    }
}

/// The `TypeError` or `AttributeError` of using `value` as what it is not.
fn mismatch(value: &Value, text: &str) -> SerdeError {
    SerdeError::new(format!("'{}' object {text}", py_short_type_name(value)))
}

impl ShapeDeserializer for JsonCursor<'_> {
    /// `_handle_structure`: `null` stays `None`; members by their JSON
    /// names.
    fn read_struct(
        &mut self,
        schema: Schema<'_>,
        each: &mut dyn FnMut(Schema<'_>, &mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError> {
        let map = match self.value {
            Value::Null => return Ok(Read::Null),
            Value::Map(map) => map,
            other => return Err(mismatch(other, "has no attribute 'get'")),
        };
        for member in schema.members() {
            if let Some(raw) = map.get(common::wire_name(member)) {
                each(member, &mut self.at(raw))?;
            }
        }
        Ok(Read::Present)
    }

    /// `ResponseParser._handle_list`, and for `RestJSONParser` a header
    /// list's text split first (`BaseRestParser._handle_list`).
    fn read_list(
        &mut self,
        schema: Schema<'_>,
        each: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError> {
        if self.handlers == Handlers::RestJson
            && let Some(items) = header_items(schema, self.value)
        {
            for item in &items? {
                each(&mut self.at(item))?;
            }
            return Ok(Read::Present);
        }
        let Value::List(items) = self.value else {
            return Err(mismatch(self.value, "is not iterable"));
        };
        for item in items {
            each(&mut self.at(item))?;
        }
        Ok(Read::Present)
    }

    /// `_handle_map`: keys through the key shape, in wire order.
    fn read_map(
        &mut self,
        schema: Schema<'_>,
        each: &mut dyn FnMut(Key, &mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError> {
        let (key_shape, _) = common::map_entry(schema).map_err(SerdeError::new)?;
        let Value::Map(map) = self.value else {
            return Err(mismatch(self.value, "has no attribute 'items'"));
        };
        for (key, item) in map.iter() {
            each(self.key(key_shape, key)?, &mut self.at(item))?;
        }
        Ok(Read::Present)
    }

    /// A scalar as the handlers read it; a blob's must be text.
    fn read_scalar(&mut self, schema: Schema<'_>) -> Result<Value, SerdeError> {
        if self.handlers == Handlers::RestJson
            && let Some(parsed) = self.rest_scalar(schema)
        {
            return parsed;
        }
        match self.value {
            Value::String(_) | Value::PyString(_) => {}
            other if schema.shape_type() == ShapeType::Blob => {
                return Err(SerdeError::new(format!(
                    "argument should be a bytes-like object or ASCII string, not '{}'",
                    py_short_type_name(other)
                )));
            }
            _ => {}
        }
        Ok(self.value.clone())
    }

    fn read_document(&mut self, _: Schema<'_>) -> Result<Value, SerdeError> {
        Ok(self.value.clone())
    }

    fn is_null(&self) -> bool {
        *self.value == Value::Null
    }

    fn union_tags(&self) -> Option<Vec<Key>> {
        let Value::Map(map) = self.value else {
            return None;
        };
        let set = map.iter().filter(|(_, value)| **value != Value::Null);
        Some(set.map(|(key, _)| key.clone()).collect())
    }
}

/// `BaseRestParser._handle_string` for a `jsonvalue` header
/// (`is_json_value_header`): the text base64-decoded and read as JSON.
/// `None` for any other string, which stays as it is.
fn json_value(shape: Schema<'_>, value: &Value) -> Option<Result<Value, SerdeError>> {
    let header = shape.location() == Some("header");
    (header && shape.flag(Flag::JsonValue)).then(|| json_value_of(value))
}

/// `json.loads(base64.b64decode(value).decode('utf-8'))`.
fn json_value_of(value: &Value) -> Result<Value, SerdeError> {
    let Value::String(text) = value else {
        return Err(SerdeError::new(format!(
            "argument should be a bytes-like object or ASCII string, not '{}'",
            py_short_type_name(value)
        )));
    };
    if !text.is_ascii() {
        return Err(SerdeError::new(
            "string argument should contain only ASCII characters",
        ));
    }
    let bytes = pytext::base64::decode(text).map_err(SerdeError::new)?;
    match model::pyjson::loads(common::decoded(&bytes)?) {
        Ok(value) => Ok(value),
        Err(LoadError::Invalid(message)) => Err(SerdeError::new(message)),
        Err(LoadError::Limit(limit)) => Err(common::beyond_limit(limit).into()),
    }
}

/// `BaseRestParser._handle_list`'s split: a header list's text as its
/// comma-separated parts, each stripped, as RFC 7230 lists them. `None`
/// for a list that is no header or is a list already.
fn header_items(shape: Schema<'_>, value: &Value) -> Option<Result<Vec<Value>, SerdeError>> {
    if shape.location() != Some("header") {
        return None;
    }
    match value {
        Value::List(_) => None,
        Value::String(text) => {
            let parts = text
                .split(',')
                .map(|part| Value::String(pytext::text::strip(part).to_owned()));
            Some(Ok(parts.collect()))
        }
        other => Some(Err(mismatch(other, "has no attribute 'split'"))),
    }
}

/// `ensure_boolean`: a bool as it is, a string by whether it is `true` in
/// any case, anything else false.
fn ensure_boolean(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        Value::String(text) => text.to_lowercase() == "true",
        // `'\ud800'.lower()` keeps the surrogate, so it is never `'true'`.
        Value::PyString(_) => false,
        _ => false,
    }
}

/// `int(value)`: a float truncated, a bool as 0 or 1, text as a base-10
/// literal.
fn int(value: &Value) -> Result<Value, SerdeError> {
    match value {
        Value::Integer(_) | Value::BigInteger(_) => Ok(value.clone()),
        Value::Bool(flag) => Ok(Value::Integer(i64::from(*flag))),
        Value::Float(number) => {
            crate::python::int_of_float(*number).map_err(|error| SerdeError::new(error.to_string()))
        }
        Value::String(text) => int_from_text(text).map_err(SerdeError::new),
        Value::PyString(text) => int_from_py_text(text.as_bytes()).map_err(SerdeError::new),
        other => Err(SerdeError::new(format!(
            "int() argument must be a string, a bytes-like object or a real number, not '{}'",
            py_short_type_name(other)
        ))),
    }
}

/// `float(value)`: an int as the nearest float, a bool as 0.0 or 1.0, text
/// as a literal (`NaN` and `Infinity` among them).
fn float(value: &Value) -> Result<Value, SerdeError> {
    let number = match value {
        Value::Float(number) => *number,
        Value::Integer(number) => *number as f64,
        Value::Bool(flag) => f64::from(u8::from(*flag)),
        Value::BigInteger(digits) => digits
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite())
            .ok_or_else(|| SerdeError::new("int too large to convert to float"))?,
        Value::String(_) | Value::PyString(_) => float_from_py_text(&py_str_text(value))
            .ok_or_else(|| {
                SerdeError::new(format!(
                    "could not convert string to float: {}",
                    py_repr(value)
                ))
            })?,
        other => {
            return Err(SerdeError::new(format!(
                "float() argument must be a string or a real number, not '{}'",
                py_short_type_name(other)
            )));
        }
    };
    Ok(Value::Float(number))
}

#[cfg(test)]
mod tests;
