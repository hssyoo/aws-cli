//! The CBOR codec: botocore's CBOR body over [`model::serde`].
//!
//! The serializer is `CBORSerializer`: it writes the body's bytes as it
//! goes, every length definite and as short as it fits, a structure's
//! `None` members dropped and its members under their `locationName`, a
//! timestamp as tag 1 over its epoch seconds whatever its
//! `timestampFormat`, a float in four bytes and a double in eight, and the
//! special floats in two. A shape type botocore has no `_serialize_type_`
//! method for fails with the `AttributeError` that lookup raises.
//!
//! The deserializer reads the tree [`decode`] gives as `BaseRpcV2Parser`
//! and `ResponseParser` read it: a structure's members by their member
//! names, a list by Python's iteration of whatever is there, and every
//! other value as it was decoded. Tag 1 values reach the timestamp parser
//! inside [`decode`], as botocore runs it; the codec's own deserializer,
//! which has no parse hooks, keeps them as their numbers.
use crate::protocol::timestamp_out;
use model::pyrepr::{py_repr, py_short_type_name};
use model::schema::{Schema, ShapeType};
use model::serde::{
    Codec, FinishSerializer, MapSerializer, Read, SerdeError, SerdeResult, SerializableStruct,
    ShapeDeserializer, ShapeSerializer,
};
use model::value::{Key, Value};

mod decode;

pub use decode::{Item, MAX_DEPTH, decode};

/// The CBOR codec.
#[derive(Debug)]
pub struct CborCodec;

/// The `smithy-rpc-v2-cbor` protocol's codec.
pub static CBOR: CborCodec = CborCodec;

impl Codec for CborCodec {
    fn media_type(&self) -> &'static str {
        "application/cbor"
    }

    fn serializer(&self) -> Box<dyn FinishSerializer + '_> {
        Box::new(CborSerializer::default())
    }

    fn deserializer<'a>(
        &self,
        body: &'a [u8],
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, SerdeError> {
        Ok(Box::new(CborBody {
            tree: decode(body, &|value| Ok(value.clone()))?,
        }))
    }
}

/// Major types.
const UNSIGNED: u8 = 0;
const NEGATIVE: u8 = 1;
const BYTES: u8 = 2;
const TEXT: u8 = 3;
const ARRAY: u8 = 4;
const MAP: u8 = 5;
const TAG: u8 = 6;
const SIMPLE: u8 = 7;

/// What the serializer is writing into.
#[derive(Debug)]
enum Open {
    /// A structure, its members counted as they are written into the
    /// buffer, which holds what came before it.
    Struct {
        before: Vec<u8>,
        members: usize,
    },
    List,
    /// A map, and the `AttributeError` its key shape's type raises, if any.
    Map(Option<String>),
}

/// `CBORSerializer._serialize_data_item`, writing the body's bytes.
#[derive(Debug, Default)]
pub struct CborSerializer {
    out: Vec<u8>,
    open: Vec<Open>,
}

impl CborSerializer {
    /// `_get_initial_byte` and the argument after it, as
    /// `_get_additional_info_and_num_bytes` sizes it.
    fn head(&mut self, major: u8, argument: u64) {
        let major = major << 5;
        match argument {
            0..24 => self.out.push(major | argument as u8),
            24..256 => self.out.extend([major | 24, argument as u8]),
            256..65536 => {
                self.out.push(major | 25);
                self.out.extend((argument as u16).to_be_bytes());
            }
            65536..4294967296 => {
                self.out.push(major | 26);
                self.out.extend((argument as u32).to_be_bytes());
            }
            _ => {
                self.out.push(major | 27);
                self.out.extend(argument.to_be_bytes());
            }
        }
    }

    fn text(&mut self, text: &[u8]) {
        self.head(TEXT, text.len() as u64);
        self.out.extend_from_slice(text);
    }

    /// Before a value: in a structure, a `None` member is dropped (`false`)
    /// and any other is counted and written after its key, its
    /// `locationName` else its name.
    fn member(&mut self, schema: Schema<'_>, value: Option<&Value>) -> bool {
        let Some(Open::Struct { members, .. }) = self.open.last_mut() else {
            return true;
        };
        if value == Some(&Value::Null) {
            return false;
        }
        *members += 1;
        self.text(crate::protocol::wire_name(schema).as_bytes());
        true
    }

    /// `_serialize_type_integer` of a Python `int` (or `bool`).
    fn integer(&mut self, value: &Value) -> SerdeResult {
        let number: i128 = match value {
            Value::Integer(number) => i128::from(*number),
            Value::Bool(flag) => i128::from(*flag),
            Value::BigInteger(text) => text.parse().unwrap_or(i128::MAX),
            // `-1 - value` of a negative one, then `major | value` below 24,
            // else `value.to_bytes`.
            Value::Float(number) => {
                let argument = if *number >= 0.0 {
                    *number
                } else {
                    -1.0 - number
                };
                return Err(SerdeError::new(if argument < 24.0 {
                    "unsupported operand type(s) for |: 'int' and 'float'"
                } else {
                    "'float' object has no attribute 'to_bytes'"
                }));
            }
            other => {
                return Err(SerdeError::new(format!(
                    "'>=' not supported between instances of '{}' and 'int'",
                    py_short_type_name(other)
                )));
            }
        };
        let (major, argument) = if number >= 0 {
            (UNSIGNED, number)
        } else {
            (NEGATIVE, -1 - number)
        };
        let argument =
            u64::try_from(argument).map_err(|_| SerdeError::new("int too big to convert"))?;
        self.head(major, argument);
        Ok(())
    }

    /// `_serialize_type_float` (`single`) or `_serialize_type_double`.
    fn float(&mut self, value: &Value, single: bool) -> SerdeResult {
        let number = match value {
            Value::Float(number) => *number,
            Value::Integer(number) => *number as f64,
            Value::Bool(flag) => f64::from(u8::from(*flag)),
            Value::BigInteger(text) => text
                .parse::<f64>()
                .ok()
                .filter(|number| number.is_finite())
                .ok_or_else(|| SerdeError::new("int too large to convert to float"))?,
            other => {
                return Err(SerdeError::new(format!(
                    "must be real number, not {}",
                    py_short_type_name(other)
                )));
            }
        };
        let special = match number {
            f64::INFINITY => Some(0x7c00u16),
            f64::NEG_INFINITY => Some(0xfc00),
            _ if number.is_nan() => Some(0x7e00),
            _ => None,
        };
        if let Some(bits) = special {
            self.out.push(SIMPLE << 5 | 25);
            self.out.extend(bits.to_be_bytes());
        } else if single {
            let narrowed = number as f32;
            if narrowed.is_infinite() {
                return Err(SerdeError::new("float too large to pack with f format"));
            }
            self.out.push(SIMPLE << 5 | 26);
            self.out.extend(narrowed.to_be_bytes());
        } else {
            self.out.push(SIMPLE << 5 | 27);
            self.out.extend(number.to_be_bytes());
        }
        Ok(())
    }
}

/// The `AttributeError` of a shape type `CBORSerializer` has no method for.
fn no_method(type_name: &str) -> SerdeError {
    SerdeError::new(format!(
        "'RpcV2CBORSerializer' object has no attribute '_serialize_type_{type_name}'"
    ))
}

/// `value.encode('utf-8')` of a `str`.
fn utf8(value: &Value) -> Result<Option<std::borrow::Cow<'_, [u8]>>, SerdeError> {
    match value {
        Value::String(text) => Ok(Some(text.as_bytes().into())),
        Value::PyString(text) => {
            pytext::codec::encode_utf8(text.as_bytes(), pytext::codec::Errors::Strict)
                .map(Some)
                .map_err(|error| SerdeError::new(error.to_string()))
        }
        _ => Ok(None),
    }
}

/// Python's truth of a value, `bool(value)`.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Integer(number) => *number != 0,
        Value::Float(number) => *number != 0.0,
        Value::String(text) => !text.is_empty(),
        Value::PyString(text) => !text.as_bytes().is_empty(),
        Value::Blob(bytes) => !bytes.is_empty(),
        Value::List(items) => !items.is_empty(),
        Value::Map(map) => !map.is_empty(),
        Value::BigInteger(_) | Value::Timestamp(_) => true,
    }
}

impl FinishSerializer for CborSerializer {
    fn finish(self: Box<Self>) -> Result<Vec<u8>, SerdeError> {
        if !self.open.is_empty() {
            return Err(SerdeError::new("an unfinished CBOR value"));
        }
        Ok(self.out)
    }
}

impl MapSerializer for CborSerializer {
    /// The key through the map's key shape, then the value.
    fn entry(
        &mut self,
        key: &Key,
        value: &mut dyn FnMut(&mut dyn ShapeSerializer) -> SerdeResult,
    ) -> SerdeResult {
        match self.open.last() {
            Some(Open::Map(None)) => {}
            Some(Open::Map(Some(error))) => return Err(SerdeError::new(error.clone())),
            _ => return Err(SerdeError::new("a map entry outside a map")),
        }
        let key = key.to_value();
        let text = utf8(&key)?.unwrap_or_default().into_owned();
        self.text(&text);
        value(self)
    }
}

macro_rules! no_method {
    ($($name:ident),*) => {$(
        fn $name(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
            if !self.member(schema, Some(value)) {
                return Ok(());
            }
            Err(no_method(schema.type_name()))
        }
    )*};
}

impl ShapeSerializer for CborSerializer {
    /// `_serialize_type_structure`: the members not `None`, in the
    /// caller's order, counted before they are written.
    fn write_struct(&mut self, schema: Schema<'_>, value: &dyn SerializableStruct) -> SerdeResult {
        self.member(schema, None);
        let before = std::mem::take(&mut self.out);
        self.open.push(Open::Struct { before, members: 0 });
        value.serialize_members(self)?;
        let Some(Open::Struct { before, members }) = self.open.pop() else {
            return Err(SerdeError::new("an unbalanced CBOR structure"));
        };
        let written = std::mem::replace(&mut self.out, before);
        self.head(MAP, members as u64);
        self.out.extend(written);
        Ok(())
    }

    /// `_serialize_type_list`: a `None` item is `null`.
    fn write_list(
        &mut self,
        schema: Schema<'_>,
        len: usize,
        items: &mut dyn FnMut(&mut dyn ShapeSerializer) -> SerdeResult,
    ) -> SerdeResult {
        self.member(schema, None);
        self.head(ARRAY, len as u64);
        self.open.push(Open::List);
        items(self)?;
        self.open.pop();
        Ok(())
    }

    /// `_serialize_type_map`: each key through the key shape, a `None`
    /// value as `null`.
    fn write_map(
        &mut self,
        schema: Schema<'_>,
        len: usize,
        entries: &mut dyn FnMut(&mut dyn MapSerializer) -> SerdeResult,
    ) -> SerdeResult {
        self.member(schema, None);
        let key_error = match schema.map_key() {
            Some(key) if key.shape_type() == ShapeType::String => None,
            Some(key) => Some(no_method(key.type_name()).into_message()),
            None => Some(format!("the map {} has no key", schema.id().as_str())),
        };
        self.head(MAP, len as u64);
        self.open.push(Open::Map(key_error));
        entries(self)?;
        self.open.pop();
        Ok(())
    }

    no_method!(
        write_byte,
        write_short,
        write_big_integer,
        write_big_decimal
    );

    fn write_boolean(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        if self.member(schema, Some(value)) {
            self.out
                .push(SIMPLE << 5 | if truthy(value) { 21 } else { 20 });
        }
        Ok(())
    }

    fn write_integer(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        if !self.member(schema, Some(value)) {
            return Ok(());
        }
        self.integer(value)
    }

    fn write_long(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        self.write_integer(schema, value)
    }

    fn write_float(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        if !self.member(schema, Some(value)) {
            return Ok(());
        }
        self.float(value, true)
    }

    fn write_double(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        if !self.member(schema, Some(value)) {
            return Ok(());
        }
        self.float(value, false)
    }

    /// `_serialize_type_string`; a shape of a type botocore does not know
    /// (which the walk writes as a string) has no method.
    fn write_string(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        if !self.member(schema, Some(value)) {
            return Ok(());
        }
        if schema.shape_type() != ShapeType::String {
            return Err(no_method(schema.type_name()));
        }
        let text = utf8(value)?.ok_or_else(|| {
            SerdeError::new(format!(
                "'{}' object has no attribute 'encode'",
                py_short_type_name(value)
            ))
        })?;
        let text = text.into_owned();
        self.text(&text);
        Ok(())
    }

    /// `_serialize_type_blob`: text as UTF-8, bytes as they are; anything
    /// else is read as a file.
    fn write_blob(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        if !self.member(schema, Some(value)) {
            return Ok(());
        }
        let bytes = match value {
            Value::Blob(bytes) => bytes.clone(),
            other => utf8(other)?
                .ok_or_else(|| {
                    SerdeError::new(format!(
                        "'{}' object has no attribute 'read'",
                        py_short_type_name(other)
                    ))
                })?
                .into_owned(),
        };
        self.head(BYTES, bytes.len() as u64);
        self.out.extend(bytes);
        Ok(())
    }

    /// `_serialize_type_timestamp`: tag 1, then the epoch seconds as an
    /// integer, or as a double with a fraction.
    fn write_timestamp(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        if !self.member(schema, Some(value)) {
            return Ok(());
        }
        let seconds = timestamp_out(value, "unixtimestamp")
            .map_err(|error| SerdeError::new(error.message))?;
        self.head(TAG, 1);
        match seconds {
            Value::Float(_) => self.float(&seconds, false),
            _ => self.integer(&seconds),
        }
    }

    /// A document is botocore's `structure` with no members: its `None`
    /// entries dropped, any other entry the `KeyError` of its key.
    fn write_document(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        if !self.member(schema, Some(value)) {
            return Ok(());
        }
        let Value::Map(map) = value else {
            return Err(SerdeError::new(format!(
                "'{}' object has no attribute 'items'",
                py_short_type_name(value)
            )));
        };
        if let Some((key, _)) = map.iter().find(|(_, value)| **value != Value::Null) {
            return Err(SerdeError::new(py_repr(&key.to_value())));
        }
        self.head(MAP, 0);
        Ok(())
    }

    fn write_null(&mut self, _schema: Schema<'_>) -> SerdeResult {
        self.out.push(SIMPLE << 5 | 22);
        Ok(())
    }
}

/// A decoded body, which its deserializer owns.
struct CborBody {
    tree: Item,
}

impl ShapeDeserializer for CborBody {
    fn read_struct(
        &mut self,
        schema: Schema<'_>,
        each: &mut dyn FnMut(Schema<'_>, &mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError> {
        CborCursor::new(&self.tree).read_struct(schema, each)
    }
    fn read_list(
        &mut self,
        schema: Schema<'_>,
        each: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError> {
        CborCursor::new(&self.tree).read_list(schema, each)
    }
    fn read_map(
        &mut self,
        schema: Schema<'_>,
        each: &mut dyn FnMut(Key, &mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError> {
        CborCursor::new(&self.tree).read_map(schema, each)
    }
    fn read_scalar(&mut self, schema: Schema<'_>) -> Result<Value, SerdeError> {
        CborCursor::new(&self.tree).read_scalar(schema)
    }
    fn read_document(&mut self, schema: Schema<'_>) -> Result<Value, SerdeError> {
        CborCursor::new(&self.tree).read_document(schema)
    }
    fn is_null(&self) -> bool {
        CborCursor::new(&self.tree).is_null()
    }
    fn union_tags(&self) -> Option<Vec<Key>> {
        None
    }
}

/// A deserializer at one item of a decoded body, reading as
/// `BaseRpcV2Parser` does. It hands `None` members to the walk, which
/// drops them.
#[derive(Clone, Copy, Debug)]
pub struct CborCursor<'i> {
    item: &'i Item,
}

impl<'i> CborCursor<'i> {
    pub const fn new(item: &'i Item) -> Self {
        Self { item }
    }

    fn mismatch(&self, text: &str) -> SerdeError {
        SerdeError::new(format!("'{}' object {text}", self.item.type_name()))
    }

    /// `ResponseParser._handle_list`'s `for item in node`: a list's items,
    /// a dict's keys, a `str`'s characters, a `bytes`' integers.
    fn iterated(&self) -> Result<std::borrow::Cow<'i, [Item]>, SerdeError> {
        Ok(match self.item {
            Item::List(items) => items.into(),
            Item::Map(entries) => entries.iter().map(|(key, _)| key.clone()).collect(),
            Item::Scalar(Value::String(text)) => (text.chars())
                .map(|char| Item::Scalar(Value::String(char.into())))
                .collect(),
            Item::Scalar(Value::Blob(bytes)) => (bytes.iter())
                .map(|byte| Item::Scalar(Value::Integer(i64::from(*byte))))
                .collect(),
            _ => return Err(self.mismatch("is not iterable")),
        })
    }
}

impl ShapeDeserializer for CborCursor<'_> {
    /// `BaseRpcV2Parser._handle_structure`: a union must have exactly one
    /// member set (`__type` aside), whatever its name; then each member
    /// by its member name, `node.get` of whatever the node is.
    fn read_struct(
        &mut self,
        schema: Schema<'_>,
        each: &mut dyn FnMut(Schema<'_>, &mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError> {
        if schema.shape_type() == ShapeType::Union {
            let set = match self.item {
                Item::Map(entries) => (entries.iter())
                    .filter(|(key, value)| {
                        let named_type =
                            matches!(key, Item::Scalar(Value::String(text)) if text == "__type");
                        !named_type && *value != Item::Scalar(Value::Null)
                    })
                    .count(),
                Item::List(_) => {
                    return Err(SerdeError::new("pop expected at most 1 argument, got 2"));
                }
                _ => return Err(self.mismatch("has no attribute 'copy'")),
            };
            if set != 1 {
                return Err(SerdeError::new(format!(
                    "Invalid service response: {} must have one and only one member set.",
                    schema.target().id().name()
                )));
            }
        }
        for member in schema.members() {
            if !matches!(self.item, Item::Map(_)) {
                return Err(self.mismatch("has no attribute 'get'"));
            }
            let name = member.member_name().unwrap_or_default();
            if let Some(found) = self.item.get(name) {
                each(member, &mut CborCursor::new(found))?;
            }
        }
        Ok(Read::Present)
    }

    fn read_list(
        &mut self,
        _schema: Schema<'_>,
        each: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError> {
        for item in self.iterated()?.iter() {
            each(&mut CborCursor::new(item))?;
        }
        Ok(Read::Present)
    }

    /// A map's entries, keys as text: what a `dict` parsed whole holds.
    fn read_map(
        &mut self,
        _schema: Schema<'_>,
        each: &mut dyn FnMut(Key, &mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError> {
        let Item::Map(entries) = self.item else {
            return Err(self.mismatch("has no attribute 'items'"));
        };
        for (key, value) in entries {
            each(key.text_key()?, &mut CborCursor::new(value))?;
        }
        Ok(Read::Present)
    }

    /// `_default_handle`: the value as it was decoded.
    fn read_scalar(&mut self, _schema: Schema<'_>) -> Result<Value, SerdeError> {
        Ok(self.item.to_value()?)
    }

    fn read_document(&mut self, _schema: Schema<'_>) -> Result<Value, SerdeError> {
        Ok(self.item.to_value()?)
    }

    fn is_null(&self) -> bool {
        *self.item == Item::Scalar(Value::Null)
    }

    /// The union check is [`Self::read_struct`]'s: botocore's RPC v2 parser
    /// has no unknown-member case.
    fn union_tags(&self) -> Option<Vec<Key>> {
        None
    }
}

#[cfg(test)]
mod tests;
