//! `FormCodec`: botocore's `QuerySerializer` body, a form of
//! `Name.member.N`-style fields `urlencode`d. A structure's members take
//! their `locationName`, else their name, after the structure's prefix; a
//! list's items `member.N` (or the member's name in place of the list's
//! when flattened); a map's entries `entry.N.key` and `entry.N.value`
//! (without `entry` when flattened). Scalars are `str(value)`, a boolean's
//! truthiness `true` or `false`, a float's special values by name, a blob
//! in base64 and a timestamp in its format. There is no deserializer: the
//! query protocol reads XML.
//!
//! [`EC2_FORM`] writes botocore's `EC2Serializer` body instead: a name is
//! the `queryName`, else the `locationName` capitalized, else the member's
//! own name, and a list's items are `Name.N` whatever its traits, an empty
//! list writing nothing.
use super::value::{ValueStruct, write_value};
use crate::error::SerializeError;
use crate::protocol::{base64_in, special_float, timestamp_out};
use model::pyrepr::{py_short_type_name, py_str, py_str_text};
use model::schema::{Flag, Schema};
use model::serde::{
    Codec, FinishSerializer, MapSerializer, SerdeError, SerdeResult, SerializableStruct,
    ShapeDeserializer, ShapeSerializer,
};
use model::value::{Key, Value};
use std::borrow::Cow;

/// The form codec of one [`Dialect`]. It writes only.
#[derive(Debug)]
pub struct FormCodec {
    dialect: Dialect,
}

/// The awsQuery [`FormCodec`].
pub static FORM: FormCodec = FormCodec {
    dialect: Dialect::Query,
};

/// The ec2Query [`FormCodec`].
pub static EC2_FORM: FormCodec = FormCodec {
    dialect: Dialect::Ec2,
};

/// Which botocore serializer's names and lists a form follows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dialect {
    /// `QuerySerializer`.
    #[default]
    Query,
    /// `EC2Serializer`.
    Ec2,
}

impl Codec for FormCodec {
    fn media_type(&self) -> &'static str {
        "application/x-www-form-urlencoded; charset=utf-8"
    }

    fn serializer(&self) -> Box<dyn FinishSerializer + '_> {
        Box::new(FormSerializer::new(self.dialect))
    }

    fn deserializer<'a>(
        &self,
        _body: &'a [u8],
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, SerdeError> {
        Err(SerdeError::new("the form codec does not deserialize"))
    }
}

impl Dialect {
    /// `_get_serialized_name`: the name `schema` takes on the wire, or
    /// `default` when its traits give none. `EC2Serializer` capitalizes a
    /// `locationName` (`name[0].upper() + name[1:]`) but not `default`.
    fn serialized_name<'s>(self, schema: Schema<'s>, default: &'s str) -> Cow<'s, str> {
        match self {
            Self::Query => Cow::Borrowed(schema.location_name().unwrap_or(default)),
            Self::Ec2 => match (schema.ec2_query_name(), schema.location_name()) {
                (Some(name), _) => Cow::Borrowed(name),
                (None, Some(name)) => Cow::Owned(capitalized(name)),
                (None, None) => Cow::Borrowed(default),
            },
        }
    }
}

/// `name[0].upper() + name[1:]`: the first character in Python's full
/// uppercase (`'ß'` becomes `'SS'`).
fn capitalized(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Where the next value's field name comes from.
#[derive(Debug)]
enum Frame {
    /// A structure's members, after its prefix.
    Members(String),
    /// A list's items, numbered from 1 after the prefix.
    Items { prefix: String, written: usize },
    /// One value with this name, a map entry's key or value, until it is
    /// written.
    Slot(Option<String>),
}

/// The form's fields in insertion order; setting a field again replaces
/// its value in place, as the serializer's dict does. A value is a `str` in
/// `pytext::wtf8`'s encoding, which may hold a lone surrogate until
/// [`FinishSerializer::finish`] fails on it. A value written at the top,
/// outside any structure, list or map, is the field `""`.
#[derive(Debug, Default)]
pub struct FormSerializer {
    fields: Vec<(String, Vec<u8>)>,
    frames: Vec<Frame>,
    dialect: Dialect,
}

impl FormSerializer {
    /// An empty form that names its fields as `dialect` does.
    pub fn new(dialect: Dialect) -> Self {
        Self {
            dialect,
            ..Self::default()
        }
    }

    /// Sets the field `name`, as `serialized[name] = value` does.
    pub fn set(&mut self, name: &str, value: Vec<u8>) {
        match self.fields.iter_mut().find(|(other, _)| other == name) {
            Some((_, old)) => *old = value,
            None => self.fields.push((name.to_owned(), value)),
        }
    }

    /// The name of the value `schema` is about to be written as.
    fn name(&mut self, schema: Schema<'_>) -> String {
        let dialect = self.dialect;
        match self.frames.last_mut() {
            None => String::new(),
            Some(Frame::Slot(name)) => name.take().unwrap_or_default(),
            Some(Frame::Members(prefix)) => {
                let default = schema.member_name().unwrap_or_default();
                let name = dialect.serialized_name(schema, default);
                if prefix.is_empty() {
                    name.into_owned()
                } else {
                    format!("{prefix}.{name}")
                }
            }
            Some(Frame::Items { prefix, written }) => {
                *written += 1;
                format!("{prefix}.{written}")
            }
        }
    }

    /// `write` with `frame` the source of its values' names.
    fn within(
        &mut self,
        frame: Frame,
        write: impl FnOnce(&mut Self) -> SerdeResult,
    ) -> SerdeResult {
        self.frames.push(frame);
        let written = write(self);
        self.frames.pop();
        written
    }

    /// Sets the field for `schema` to `value`.
    fn scalar(&mut self, schema: Schema<'_>, value: Vec<u8>) -> SerdeResult {
        let name = self.name(schema);
        self.set(&name, value);
        Ok(())
    }

    /// The body on the wire: `AWSRequestPreparer._prepare_body`'s
    /// `urlencode(params, doseq=True)` of the serializer's dict.
    ///
    /// # Errors
    /// `quote_plus`'s `UnicodeEncodeError` for a value with a lone
    /// surrogate, its position within that value.
    fn encoded(&self) -> Result<String, SerdeError> {
        let mut pairs = Vec::with_capacity(self.fields.len());
        for (name, value) in &self.fields {
            let value = pytext::codec::encode_utf8(value, pytext::codec::Errors::Strict)
                .map_err(|error| SerdeError::new(error.to_string()))?;
            pairs.push(format!(
                "{}={}",
                quote_plus(name.as_bytes()),
                quote_plus(&value)
            ));
        }
        Ok(pairs.join("&"))
    }
}

fn serde(error: SerializeError) -> SerdeError {
    SerdeError::new(error.message)
}

impl ShapeSerializer for FormSerializer {
    fn write_struct(&mut self, schema: Schema<'_>, value: &dyn SerializableStruct) -> SerdeResult {
        let prefix = self.name(schema);
        self.within(Frame::Members(prefix), |form| value.serialize_members(form))
    }

    /// `_serialize_type_list`: `Name=` when empty; a flattened list takes
    /// its member's name in place of the last part of its prefix.
    fn write_list(
        &mut self,
        schema: Schema<'_>,
        len: usize,
        items: &mut dyn FnMut(&mut dyn ShapeSerializer) -> SerdeResult,
    ) -> SerdeResult {
        let prefix = self.name(schema);
        if self.dialect == Dialect::Ec2 {
            // `EC2Serializer._serialize_type_list`: `Name.N`, nothing when empty.
            return self.within(Frame::Items { prefix, written: 0 }, |form| items(form));
        }
        if len == 0 {
            self.set(&prefix, Vec::new());
            return Ok(());
        }
        let member_name = schema.list_member().and_then(Schema::location_name);
        let prefix = if schema.flag(Flag::XmlFlattened) {
            match member_name {
                Some(name) => match prefix.rsplit_once('.') {
                    Some((head, _)) => format!("{head}.{name}"),
                    None => name.to_owned(),
                },
                None => prefix,
            }
        } else {
            format!("{prefix}.{}", member_name.unwrap_or("member"))
        };
        self.within(Frame::Items { prefix, written: 0 }, |form| items(form))
    }

    /// `_serialize_type_map`: `entry.N.key` and `entry.N.value`, or without
    /// `entry` when flattened, with the key and value shapes' names; the key
    /// is written through the key's shape.
    fn write_map(
        &mut self,
        schema: Schema<'_>,
        _len: usize,
        entries: &mut dyn FnMut(&mut dyn MapSerializer) -> SerdeResult,
    ) -> SerdeResult {
        let prefix = self.name(schema);
        let (Some(key), Some(value)) = (schema.map_key(), schema.map_value()) else {
            return Err(SerdeError::new(format!(
                "the map {} has no key or value",
                schema.target().id().name()
            )));
        };
        let prefix = if schema.flag(Flag::XmlFlattened) {
            prefix
        } else {
            format!("{prefix}.entry")
        };
        let dialect = self.dialect;
        entries(&mut Entries {
            form: self,
            prefix,
            key,
            key_name: dialect.serialized_name(key, "key"),
            value_name: dialect.serialized_name(value, "value"),
            written: 0,
        })
    }

    fn write_boolean(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        let text = if crate::python::truthy(value) {
            "true"
        } else {
            "false"
        };
        self.scalar(schema, text.as_bytes().to_vec())
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

    fn write_float(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        self.scalar(schema, py_str(&special_float(value)).into_bytes())
    }

    fn write_double(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        self.write_float(schema, value)
    }

    fn write_big_integer(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        self.write_string(schema, value)
    }

    fn write_big_decimal(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        self.write_string(schema, value)
    }

    /// `str(value)`, a lone surrogate kept for `urlencode`.
    fn write_string(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        self.scalar(schema, py_str_text(value).into_owned())
    }

    fn write_blob(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        let name = self.name(schema);
        let encoded = base64_in(value).map_err(serde)?;
        self.set(&name, encoded.into_bytes());
        Ok(())
    }

    fn write_timestamp(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        let name = self.name(schema);
        let format = schema.timestamp_format_text().unwrap_or("iso8601");
        let written = timestamp_out(value, format).map_err(serde)?;
        self.set(&name, py_str(&written).into_bytes());
        Ok(())
    }

    /// A document is a `structure` to the query serializer.
    fn write_document(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        let Value::Map(map) = value else {
            return Err(SerdeError::new(format!(
                "'{}' object has no attribute 'items'",
                py_short_type_name(value)
            )));
        };
        self.write_struct(schema, &ValueStruct::new(schema, map))
    }

    /// `None` is serialized through the item's shape as any other value.
    fn write_null(&mut self, schema: Schema<'_>) -> SerdeResult {
        write_value(schema, &Value::Null, self)
    }
}

impl FinishSerializer for FormSerializer {
    fn finish(self: Box<Self>) -> Result<Vec<u8>, SerdeError> {
        self.encoded().map(String::into_bytes)
    }
}

/// A map's entries as the form numbers them.
struct Entries<'f, 's> {
    form: &'f mut FormSerializer,
    prefix: String,
    key: Schema<'s>,
    key_name: Cow<'s, str>,
    value_name: Cow<'s, str>,
    written: usize,
}

impl MapSerializer for Entries<'_, '_> {
    fn entry(
        &mut self,
        key: &Key,
        value: &mut dyn FnMut(&mut dyn ShapeSerializer) -> SerdeResult,
    ) -> SerdeResult {
        self.written += 1;
        let entry = format!("{}.{}", self.prefix, self.written);
        let key_field = Frame::Slot(Some(format!("{entry}.{}", self.key_name)));
        let (key_schema, key_value) = (self.key, key.to_value());
        self.form
            .within(key_field, |form| write_value(key_schema, &key_value, form))?;
        let value_field = Frame::Slot(Some(format!("{entry}.{}", self.value_name)));
        self.form.within(value_field, |form| value(form))
    }
}

/// `urllib.parse.quote_plus` with no extra safe characters, of text
/// already encoded as UTF-8: a space as `+`, and every byte but letters,
/// digits and `-_.~` as `%XX`.
fn quote_plus(text: &[u8]) -> String {
    let mut encoded = String::with_capacity(text.len());
    for &byte in text {
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

#[cfg(test)]
mod tests;
