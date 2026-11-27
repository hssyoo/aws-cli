//! Schema-driven serialization traits over [`crate::value::Value`] and the
//! error they share, shaped after smithy-rs's `aws_smithy_schema::serde`
//! and `codec`.
//!
//! botocore's serializers act on the Python object they are given (an
//! `int` passed for a `double` is written as `1`) and write a structure's
//! members in the input dict's order, so a scalar write carries the
//! [`Value`], not a Rust primitive. Its parsers parse the whole body into a
//! tree, then walk the schema, so a [`ShapeDeserializer`] calls back in
//! schema order, not wire order.
//!
//! Only traits live here. The one [`SerializableStruct`] over a `Value`
//! map, the schema walks and the codecs are the client's.
use crate::schema::Schema;
use crate::value::{Key, Value};

/// A serializer or deserializer failure: the text botocore raises for it,
/// e.g. `a dict is required`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SerdeError {
    message: String,
}

impl SerdeError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    /// The text, owned.
    pub fn into_message(self) -> String {
        self.message
    }
}

impl std::fmt::Display for SerdeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SerdeError {}

/// What a write callback returns.
pub type SerdeResult = Result<(), SerdeError>;

/// Writes one value of a schema to a wire format. `schema` is the value's
/// own schema: a structure member's, a list's `member`, a map's `value`,
/// or a root. A member schema carries its name and wire-name traits, so a
/// codec writing a structure's member takes its key from the schema; one
/// writing a list item or map value takes its position from the enclosing
/// write.
///
/// Every scalar write passes the input's [`Value`] as it is: converting it
/// (base64 of a blob, a timestamp's format, a special float's text) and
/// refusing the wrong kind of value, with botocore's text, are the codec's.
pub trait ShapeSerializer {
    /// A structure or union: `value` writes the members back through this
    /// serializer, in its own order.
    fn write_struct(&mut self, schema: Schema<'_>, value: &dyn SerializableStruct) -> SerdeResult;
    /// A list of `len` items. `items` is called once and writes every item
    /// through the serializer it is given, with the list's member schema.
    fn write_list(
        &mut self,
        schema: Schema<'_>,
        len: usize,
        items: &mut dyn FnMut(&mut dyn ShapeSerializer) -> SerdeResult,
    ) -> SerdeResult;
    /// A map of `len` entries. `entries` is called once and writes every
    /// entry through the [`MapSerializer`] it is given.
    fn write_map(
        &mut self,
        schema: Schema<'_>,
        len: usize,
        entries: &mut dyn FnMut(&mut dyn MapSerializer) -> SerdeResult,
    ) -> SerdeResult;
    fn write_boolean(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult;
    fn write_byte(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult;
    fn write_short(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult;
    fn write_integer(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult;
    fn write_long(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult;
    fn write_float(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult;
    fn write_double(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult;
    fn write_big_integer(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult;
    fn write_big_decimal(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult;
    fn write_string(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult;
    fn write_blob(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult;
    fn write_timestamp(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult;
    /// A document (a C2J `document` structure): the value whole.
    fn write_document(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult;
    /// A `None` list item or map value. Botocore's formats differ here:
    /// JSON writes `null`, query serializes `None` through the item's
    /// schema as any other value, so the codec decides.
    fn write_null(&mut self, schema: Schema<'_>) -> SerdeResult;
}

/// Writes a map's entries, each a key and a value written through the
/// serializer `value` is given, with the map's `value` schema.
pub trait MapSerializer {
    fn entry(
        &mut self,
        key: &Key,
        value: &mut dyn FnMut(&mut dyn ShapeSerializer) -> SerdeResult,
    ) -> SerdeResult;
}

/// A structure value a [`ShapeSerializer`] writes.
pub trait SerializableStruct {
    fn schema(&self) -> Schema<'_>;
    /// Writes each member through `serializer`, with the member's schema.
    fn serialize_members(&self, serializer: &mut dyn ShapeSerializer) -> SerdeResult;
}

/// Whether an aggregate read found a value or a `null`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Read {
    Present,
    Null,
}

/// Reads one value of a schema from a parsed wire tree, positioned at it.
/// The callbacks run in schema order (a structure's members as the model
/// declares them) or, for lists and maps, in wire order; a type mismatch
/// fails with the format's own botocore text.
pub trait ShapeDeserializer {
    /// A structure or union: `each` is called with each member's schema and
    /// a deserializer at its value, for the members the wire has, in
    /// schema order. `Read::Null` (and no call) for a wire `null`.
    fn read_struct(
        &mut self,
        schema: Schema<'_>,
        each: &mut dyn FnMut(Schema<'_>, &mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError>;
    /// A list: `each` is called per item, in wire order.
    fn read_list(
        &mut self,
        schema: Schema<'_>,
        each: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError>;
    /// A map: `each` is called per entry with its key, in wire order.
    fn read_map(
        &mut self,
        schema: Schema<'_>,
        each: &mut dyn FnMut(Key, &mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError>;
    /// The converted botocore scalar for a boolean, number or string, with
    /// the format's own conversion and error text (XML's `text == "true"`;
    /// JSON's parsed number as it is). For a timestamp or blob, the raw
    /// wire value, which the caller's parse hooks convert.
    fn read_scalar(&mut self, schema: Schema<'_>) -> Result<Value, SerdeError>;
    /// A document: the value whole.
    fn read_document(&mut self, schema: Schema<'_>) -> Result<Value, SerdeError>;
    /// Whether the value here is a wire `null`.
    fn is_null(&self) -> bool;
    /// For a structure value, the wire names of its members that are set
    /// (non-null), in wire order: what botocore's tagged-union check reads.
    /// `None` for any other value.
    fn union_tags(&self) -> Option<Vec<Key>>;
}

/// A wire format: bytes to and from what its (de)serializers walk.
/// Object-safe; its settings are fixed on its `static` instance.
pub trait Codec: Sync + std::fmt::Debug {
    fn media_type(&self) -> &'static str;
    fn serializer(&self) -> Box<dyn FinishSerializer + '_>;
    fn deserializer<'a>(
        &self,
        body: &'a [u8],
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, SerdeError>;
}

/// A serializer that yields its bytes once every value is written.
pub trait FinishSerializer: ShapeSerializer {
    fn finish(self: Box<Self>) -> Result<Vec<u8>, SerdeError>;
}

#[cfg(test)]
mod tests;
