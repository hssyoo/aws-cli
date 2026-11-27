//! The [`Value`] side of [`model::serde`]: an input map as the one
//! [`SerializableStruct`] ([`ValueStruct`]), the walk that drives a
//! serializer's writes by shape type ([`write_value`]), and the walk that
//! assembles a parsed output from a deserializer ([`build_value`]).
//!
//! What every botocore serializer and parser shares lives here: a
//! structure's members in the input's order on the way out and in the
//! model's on the way in, botocore's texts for an input of the wrong kind
//! of container, `null` structure members dropped and `null` list items
//! and map values kept, the tagged-union check, and the CLI's parse hooks.
//! What differs by format is the codec's.
use crate::error::ParseError;
use crate::protocol::ParseOptions;
use model::pyrepr::py_short_type_name;
use model::schema::{Schema, ShapeType};
use model::serde::{
    MapSerializer, Read, SerdeError, SerdeResult, SerializableStruct, ShapeDeserializer,
    ShapeSerializer,
};
use model::value::{Key, Map, Value};

/// A structure input: its schema and the map the caller passed, whose
/// members it writes in the map's own order, as botocore's serializers
/// iterate the input dict.
#[derive(Clone, Copy, Debug)]
pub struct ValueStruct<'a> {
    schema: Schema<'a>,
    map: &'a Map,
}

impl<'a> ValueStruct<'a> {
    pub const fn new(schema: Schema<'a>, map: &'a Map) -> Self {
        Self { schema, map }
    }
}

impl SerializableStruct for ValueStruct<'_> {
    fn schema(&self) -> Schema<'_> {
        self.schema
    }

    fn serialize_members(&self, serializer: &mut dyn ShapeSerializer) -> SerdeResult {
        for (key, item) in self.map.iter() {
            write_value(member(self.schema, key)?, item, serializer)?;
        }
        Ok(())
    }
}

/// `shape.members[key]`: the member's schema, else the `KeyError`'s text.
/// An input's keys are the CLI's, so a key with a lone surrogate is only
/// ever such a `KeyError`, as it is in Python.
fn member<'s>(schema: Schema<'s>, key: &Key) -> Result<Schema<'s>, SerdeError> {
    key.as_str()
        .and_then(|name| schema.member(name))
        .ok_or_else(|| SerdeError::new(format!("'{}'", key.text())))
}

/// `Serializer._serialize`: `value` written through `serializer` by its
/// schema's type. A structure, list or map that is given another kind of
/// value fails with the text Python's iteration of it raises in every
/// botocore serializer. A shape type botocore has no handler for (one the
/// validator leaves undecided) is written as a string is, as
/// `_default_serialize` does.
pub fn write_value(
    schema: Schema<'_>,
    value: &Value,
    serializer: &mut dyn ShapeSerializer,
) -> SerdeResult {
    match schema.shape_type() {
        ShapeType::Structure | ShapeType::Union => {
            let Value::Map(map) = value else {
                return Err(SerdeError::new(format!(
                    "'{}' object has no attribute 'items'",
                    py_short_type_name(value)
                )));
            };
            serializer.write_struct(schema, &ValueStruct::new(schema, map))
        }
        ShapeType::List => {
            let Value::List(items) = value else {
                return Err(SerdeError::new("a list is required"));
            };
            let item = child(schema, schema.list_member())?;
            serializer.write_list(schema, items.len(), &mut |serializer| {
                items
                    .iter()
                    .try_for_each(|value| write_entry(item, value, serializer))
            })
        }
        ShapeType::Map => {
            let Value::Map(map) = value else {
                return Err(SerdeError::new("a dict is required"));
            };
            let item = child(schema, schema.map_value())?;
            serializer.write_map(schema, map.len(), &mut |entries: &mut dyn MapSerializer| {
                map.iter().try_for_each(|(key, value)| {
                    entries.entry(key, &mut |serializer| write_entry(item, value, serializer))
                })
            })
        }
        ShapeType::Boolean => serializer.write_boolean(schema, value),
        ShapeType::Byte => serializer.write_byte(schema, value),
        ShapeType::Short => serializer.write_short(schema, value),
        ShapeType::Integer => serializer.write_integer(schema, value),
        ShapeType::Long => serializer.write_long(schema, value),
        ShapeType::Float => serializer.write_float(schema, value),
        ShapeType::Double => serializer.write_double(schema, value),
        ShapeType::BigInteger => serializer.write_big_integer(schema, value),
        ShapeType::BigDecimal => serializer.write_big_decimal(schema, value),
        ShapeType::String | ShapeType::Unknown => serializer.write_string(schema, value),
        ShapeType::Blob => serializer.write_blob(schema, value),
        ShapeType::Timestamp => serializer.write_timestamp(schema, value),
        ShapeType::Document => serializer.write_document(schema, value),
    }
}

/// A list item or map value: `None` is the codec's [`ShapeSerializer::write_null`].
fn write_entry(
    schema: Schema<'_>,
    value: &Value,
    serializer: &mut dyn ShapeSerializer,
) -> SerdeResult {
    match value {
        Value::Null => serializer.write_null(schema),
        value => write_value(schema, value, serializer),
    }
}

/// A list's member or a map's value, which every built set has.
fn child<'s>(schema: Schema<'s>, child: Option<Schema<'s>>) -> Result<Schema<'s>, SerdeError> {
    child.ok_or_else(|| SerdeError::new(format!("{} has no member", schema.id().as_str())))
}

/// `ResponseParser._parse_shape` over `deserializer`: the parsed value of
/// `schema`. A structure's members come in the model's order under their
/// member names, `null` ones left out, and a `null` structure is `None`;
/// list items and map values keep their wire order, `null` ones kept. A
/// union whose one set member the model does not know is botocore's
/// `SDK_UNKNOWN_MEMBER`. Timestamps and blobs go through `options`, the
/// CLI's parse hooks (identity for `cli_timestamp_format = wire`); map
/// keys are the codec's as they are.
pub fn build_value(
    schema: Schema<'_>,
    deserializer: &mut dyn ShapeDeserializer,
    options: &ParseOptions<'_>,
) -> Result<Value, ParseError> {
    match schema.shape_type() {
        ShapeType::Structure | ShapeType::Union => structure(schema, deserializer, options),
        ShapeType::List => {
            let item = child(schema, schema.list_member())?;
            let mut items = Vec::new();
            let read = deserializer.read_list(schema, &mut |deserializer| {
                items.push(entry(item, deserializer, options)?);
                Ok(())
            })?;
            Ok(present(read, || Value::List(items)))
        }
        ShapeType::Map => {
            let item = child(schema, schema.map_value())?;
            let mut parsed = Map::new();
            let read = deserializer.read_map(schema, &mut |key, deserializer| {
                parsed.insert_key(key, entry(item, deserializer, options)?);
                Ok(())
            })?;
            Ok(present(read, || Value::Map(parsed)))
        }
        ShapeType::Timestamp => (options.timestamp)(&deserializer.read_scalar(schema)?),
        ShapeType::Blob => (options.blob)(&deserializer.read_scalar(schema)?),
        ShapeType::Document => Ok(deserializer.read_document(schema)?),
        _ => Ok(deserializer.read_scalar(schema)?),
    }
}

fn structure(
    schema: Schema<'_>,
    deserializer: &mut dyn ShapeDeserializer,
    options: &ParseOptions<'_>,
) -> Result<Value, ParseError> {
    if schema.shape_type() == ShapeType::Union
        && !deserializer.is_null()
        && let Some(tag) = unknown_union_member(schema, deserializer)?
    {
        return Ok(Value::Map(unknown_member_value(tag)));
    }
    let mut parsed = Map::new();
    let read = deserializer.read_struct(schema, &mut |member, deserializer| {
        if !deserializer.is_null() {
            let name = member.member_name().unwrap_or_default();
            parsed.insert(name.to_owned(), build_value(member, deserializer, options)?);
        }
        Ok(())
    })?;
    Ok(present(read, || Value::Map(parsed)))
}

/// A list item or map value: `null` kept as `None`.
fn entry(
    schema: Schema<'_>,
    deserializer: &mut dyn ShapeDeserializer,
    options: &ParseOptions<'_>,
) -> Result<Value, SerdeError> {
    if deserializer.is_null() {
        return Ok(Value::Null);
    }
    Ok(build_value(schema, deserializer, options)?)
}

fn present(read: Read, value: impl FnOnce() -> Value) -> Value {
    match read {
        Read::Present => value(),
        Read::Null => Value::Null,
    }
}

/// `ResponseParser._has_unknown_tagged_union_member`: the tag of the one
/// set member when the model does not know it (by its `locationName`, else
/// its name), or `Err` when other than exactly one is set. `__type` is no
/// member.
fn unknown_union_member(
    schema: Schema<'_>,
    deserializer: &dyn ShapeDeserializer,
) -> Result<Option<Key>, ParseError> {
    let Some(tags) = deserializer.union_tags() else {
        return Ok(None);
    };
    let set: Vec<Key> = tags.into_iter().filter(|tag| *tag != *"__type").collect();
    let [tag] = &set[..] else {
        return Err(ParseError::new(format!(
            "Invalid service response: {} must have one and only one member set.",
            schema.target().id().name()
        )));
    };
    let known = schema.members().any(|member| {
        let name = member.location_name().or(member.member_name());
        *tag == *name.unwrap_or_default()
    });
    Ok((!known).then(|| tag.clone()))
}

/// `_handle_unknown_tagged_union_member`: the tag as the `str` it is.
fn unknown_member_value(tag: Key) -> Map {
    let mut name = Map::new();
    name.insert("name".into(), tag.to_value());
    let mut parsed = Map::new();
    parsed.insert("SDK_UNKNOWN_MEMBER".into(), Value::Map(name));
    parsed
}

#[cfg(test)]
mod tests;
