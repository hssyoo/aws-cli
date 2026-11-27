//! A test codec: botocore's JSON body (`JSONSerializer._serialize`, the
//! `json` protocol's reader) over the serde traits, so the shared walks
//! can be replayed against today's serializer and parser. It also records
//! which write each value took.
use model::pyrepr::{float_from_text, py_short_type_name, py_str};
use model::schema::Schema;
use model::serde::{
    MapSerializer, Read, SerdeError, SerdeResult, SerializableStruct, ShapeDeserializer,
    ShapeSerializer,
};
use model::value::{Key, Map, Value};
use timestamp::Timestamp;

enum Frame {
    Struct(Map),
    List(Vec<Value>),
    Map(Map, Option<Key>),
}

/// Builds the body's value tree, as `JSONSerializer` does before `dumps`.
#[derive(Default)]
pub(super) struct Writer {
    frames: Vec<Frame>,
    root: Option<Value>,
    pub(super) calls: Vec<&'static str>,
}

impl Writer {
    pub(super) fn finish(self) -> Result<Value, SerdeError> {
        self.root.ok_or_else(|| SerdeError::new("nothing written"))
    }

    /// A written value, under its member's wire name, at its list position
    /// or under its map key.
    fn put(&mut self, schema: Schema<'_>, value: Value) -> SerdeResult {
        match self.frames.last_mut() {
            None => self.root = Some(value),
            Some(Frame::Struct(map)) => {
                let name = schema.location_name().or(schema.member_name());
                map.insert(name.unwrap_or_default().to_owned(), value);
            }
            Some(Frame::List(items)) => items.push(value),
            Some(Frame::Map(map, key)) => {
                let key = key
                    .take()
                    .ok_or_else(|| SerdeError::new("a value without key"))?;
                map.insert_key(key, value);
            }
        }
        Ok(())
    }

    fn scalar(&mut self, call: &'static str, schema: Schema<'_>, value: Value) -> SerdeResult {
        self.calls.push(call);
        self.put(schema, value)
    }

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
            None => return Err(SerdeError::new("unbalanced")),
        };
        self.put(schema, value)
    }
}

impl MapSerializer for Writer {
    fn entry(
        &mut self,
        key: &Key,
        value: &mut dyn FnMut(&mut dyn ShapeSerializer) -> SerdeResult,
    ) -> SerdeResult {
        if let Some(Frame::Map(_, pending)) = self.frames.last_mut() {
            *pending = Some(key.clone());
        }
        value(self)
    }
}

/// Every scalar write that `JSONSerializer` passes through unchanged.
macro_rules! as_is {
    ($($name:ident),*) => {$(
        fn $name(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
            self.scalar(stringify!($name), schema, value.clone())
        }
    )*};
}

impl ShapeSerializer for Writer {
    fn write_struct(&mut self, schema: Schema<'_>, value: &dyn SerializableStruct) -> SerdeResult {
        self.calls.push("write_struct");
        self.nested(Frame::Struct(Map::new()), schema, |this| {
            value.serialize_members(this)
        })
    }
    fn write_list(
        &mut self,
        schema: Schema<'_>,
        _: usize,
        items: &mut dyn FnMut(&mut dyn ShapeSerializer) -> SerdeResult,
    ) -> SerdeResult {
        self.calls.push("write_list");
        self.nested(Frame::List(Vec::new()), schema, |this| items(this))
    }
    fn write_map(
        &mut self,
        schema: Schema<'_>,
        _: usize,
        entries: &mut dyn FnMut(&mut dyn MapSerializer) -> SerdeResult,
    ) -> SerdeResult {
        self.calls.push("write_map");
        self.nested(Frame::Map(Map::new(), None), schema, |this| entries(this))
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
    fn write_float(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        self.scalar("write_float", schema, special_float(value))
    }
    fn write_double(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        self.scalar("write_double", schema, special_float(value))
    }
    fn write_blob(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        let encoded = match value {
            Value::String(text) => pytext::base64::encode(text.as_bytes()),
            Value::Blob(bytes) => pytext::base64::encode(bytes),
            other => {
                return Err(SerdeError::new(format!(
                    "a bytes-like object is required, not '{}'",
                    py_short_type_name(other)
                )));
            }
        };
        self.scalar("write_blob", schema, Value::String(encoded))
    }
    fn write_timestamp(&mut self, schema: Schema<'_>, value: &Value) -> SerdeResult {
        let at = instant(value)
            .ok_or_else(|| SerdeError::new(format!("Invalid timestamp \"{}\"", py_str(value))))?;
        let text = match schema.timestamp_format_text().unwrap_or("unixtimestamp") {
            "iso8601" => Value::String(at.iso8601()),
            "rfc822" => Value::String(at.rfc822()),
            _ if at.subsec_micros() == 0 => Value::Integer(at.unix_seconds()),
            _ => Value::Float(at.unix_micros() as f64 / 1e6),
        };
        self.scalar("write_timestamp", schema, text)
    }
    fn write_null(&mut self, schema: Schema<'_>) -> SerdeResult {
        self.scalar("write_null", schema, Value::Null)
    }
}

fn special_float(value: &Value) -> Value {
    match value {
        Value::Float(number) if number.is_nan() => Value::String("NaN".into()),
        Value::Float(number) if number.is_infinite() && *number > 0.0 => {
            Value::String("Infinity".into())
        }
        Value::Float(number) if number.is_infinite() => Value::String("-Infinity".into()),
        other => other.clone(),
    }
}

/// `parse_to_aware_datetime`, for the inputs the cases pass.
fn instant(value: &Value) -> Option<Timestamp> {
    match value {
        Value::Timestamp(at) => Some(*at),
        Value::Integer(seconds) => Timestamp::from_unix_seconds(*seconds),
        Value::Float(seconds) => Timestamp::from_epoch_float(*seconds),
        Value::String(text) => match float_from_text(text) {
            Some(seconds) => Timestamp::from_epoch_float(seconds),
            None => {
                let zoned = Timestamp::parse_iso8601_zoned(text)
                    .or_else(|| Timestamp::parse_rfc822_zoned(text))?;
                let offset = i64::from(zoned.offset_seconds.unwrap_or(0)) * 1_000_000;
                let local = Timestamp::from_civil(zoned.civil)?;
                Timestamp::from_unix_micros(local.unix_micros() - offset)
            }
        },
        _ => None,
    }
}

/// A cursor over a parsed JSON body, as `BaseJSONParser`'s handlers read
/// it. It hands `null` members to the walk, which drops them.
pub(super) struct Cursor<'v>(pub(super) &'v Value);

fn mismatch(value: &Value, text: &str) -> SerdeError {
    SerdeError::new(format!("'{}' object {text}", py_short_type_name(value)))
}

impl ShapeDeserializer for Cursor<'_> {
    fn read_struct(
        &mut self,
        schema: Schema<'_>,
        each: &mut dyn FnMut(Schema<'_>, &mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError> {
        let map = match self.0 {
            Value::Null => return Ok(Read::Null),
            Value::Map(map) => map,
            other => return Err(mismatch(other, "has no attribute 'get'")),
        };
        for member in schema.members() {
            let name = member.location_name().or(member.member_name());
            if let Some(raw) = map.get(name.unwrap_or_default()) {
                each(member, &mut Cursor(raw))?;
            }
        }
        Ok(Read::Present)
    }
    fn read_list(
        &mut self,
        _: Schema<'_>,
        each: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError> {
        let Value::List(items) = self.0 else {
            return Err(mismatch(self.0, "is not iterable"));
        };
        items.iter().try_for_each(|item| each(&mut Cursor(item)))?;
        Ok(Read::Present)
    }
    fn read_map(
        &mut self,
        _: Schema<'_>,
        each: &mut dyn FnMut(Key, &mut dyn ShapeDeserializer) -> SerdeResult,
    ) -> Result<Read, SerdeError> {
        let Value::Map(map) = self.0 else {
            return Err(mismatch(self.0, "has no attribute 'items'"));
        };
        map.iter()
            .try_for_each(|(key, item)| each(key.clone(), &mut Cursor(item)))?;
        Ok(Read::Present)
    }
    fn read_scalar(&mut self, schema: Schema<'_>) -> Result<Value, SerdeError> {
        match self.0 {
            Value::String(_) | Value::PyString(_) => {}
            other if schema.shape_type() == model::schema::ShapeType::Blob => {
                return Err(SerdeError::new(format!(
                    "argument should be a bytes-like object or ASCII string, not '{}'",
                    py_short_type_name(other)
                )));
            }
            _ => {}
        }
        Ok(self.0.clone())
    }
    fn read_document(&mut self, _: Schema<'_>) -> Result<Value, SerdeError> {
        Ok(self.0.clone())
    }
    fn is_null(&self) -> bool {
        *self.0 == Value::Null
    }
    fn union_tags(&self) -> Option<Vec<Key>> {
        let Value::Map(map) = self.0 else {
            return None;
        };
        let set = map.iter().filter(|(_, value)| **value != Value::Null);
        Some(set.map(|(key, _)| key.clone()).collect())
    }
}
