//! `BaseCBORParser.parse_data_item`: one data item read from the front of
//! a body into the tree botocore holds, an [`Item`], with its error texts.
//!
//! The reader is botocore's, not RFC 8949's: integers to 64 bits (a
//! negative one down to `-2**64`), byte and text strings definite or
//! chunked (a chunk's major type unchecked), arrays and maps definite or
//! indefinite, tag 1 alone (its value through the timestamp parser, as
//! botocore runs it while it reads), half, single and double floats, and
//! the simple values `false`, `true`, `null` and `undefined` (`None`).
//! A map is a `dict`: a repeated key keeps its first position and takes
//! the last value, keys comparing as Python's do (`1`, `1.0` and `True`
//! are one key), and a list or map key is `unhashable`. Bytes after the
//! item are never read.
use crate::error::ParseError;
use crate::protocol::{TimestampHandler, decoded};
use model::pyrepr::{py_repr, py_short_type_name, py_str};
use model::value::{Key, Map, Value};

/// The deepest nesting of arrays, maps and tags read, counting the
/// outermost. Python's own recursion limit stops botocore's reader a
/// little deeper (about 330 maps or 490 arrays from a bare interpreter,
/// fewer under the CLI), so every body read here botocore reads too.
pub const MAX_DEPTH: usize = 200;

/// A decoded data item, as `parse_data_item` returns it.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// A number, string, byte string, `True`, `False` or `None`, or what the
    /// timestamp parser made of a tag 1 value.
    Scalar(Value),
    List(Vec<Item>),
    /// A `dict`'s entries in their order, each key once.
    Map(Vec<(Item, Item)>),
}

const END_OF_STREAM: &str = "End of stream reached; this indicates a malformed CBOR response \
                             from the server or an issue in botocore";

/// `parse_data_item` of the item at the front of `body`, tag 1 values
/// through `timestamp`.
///
/// # Errors
/// botocore's `ResponseParserError` texts, and the Python errors its
/// reader lets through (a text string that is not UTF-8, an unhashable
/// key, a length past `ssize_t`); a body nested past [`MAX_DEPTH`] names
/// the native limit.
pub fn decode(body: &[u8], timestamp: TimestampHandler<'_>) -> Result<Item, ParseError> {
    let mut reader = Reader {
        body,
        at: 0,
        depth: 0,
        timestamp,
    };
    reader.item()
}

struct Reader<'b, 'h> {
    body: &'b [u8],
    at: usize,
    depth: usize,
    timestamp: TimestampHandler<'h>,
}

impl Reader<'_, '_> {
    /// `_read_from_stream`: exactly `len` bytes, else the end of stream.
    /// `BufferedReader.read` refuses a length past `ssize_t` first.
    fn read(&mut self, len: u64) -> Result<&[u8], ParseError> {
        if len > i64::MAX as u64 {
            return Err(ParseError::new(
                "cannot fit 'int' into an index-sized integer",
            ));
        }
        // `PyBytes_FromStringAndSize`'s own bound, its header's 33 bytes.
        if len > i64::MAX as u64 - 33 {
            return Err(ParseError::new("byte string is too large"));
        }
        let rest = &self.body[self.at..];
        let len = usize::try_from(len).unwrap_or(usize::MAX);
        if rest.len() < len {
            return Err(ParseError::new(END_OF_STREAM));
        }
        self.at += len;
        Ok(&rest[..len])
    }

    fn byte(&mut self) -> Result<u8, ParseError> {
        Ok(self.read(1)?[0])
    }

    /// `_handle_break_code`: a `0xff` next is consumed; the end of the
    /// body is no break.
    fn at_break(&mut self) -> bool {
        let found = self.body.get(self.at) == Some(&0xff);
        self.at += usize::from(found);
        found
    }

    fn item(&mut self) -> Result<Item, ParseError> {
        let initial = self.byte()?;
        let info = initial & 0x1f;
        match initial >> 5 {
            0 => Ok(Item::Scalar(int(i128::from(self.unsigned(info)?)))),
            1 => Ok(Item::Scalar(int(-1 - i128::from(self.unsigned(info)?)))),
            2 => Ok(Item::Scalar(Value::Blob(self.bytes(info)?))),
            3 => {
                let bytes = self.bytes(info)?;
                Ok(Item::Scalar(Value::String(decoded(&bytes)?.to_owned())))
            }
            major => {
                self.depth += 1;
                if self.depth > MAX_DEPTH {
                    return Err(ParseError::new(format!(
                        "native client limit: the response holds CBOR data items nested more \
                         than {MAX_DEPTH} deep, which the native client does not represent"
                    )));
                }
                let item = match major {
                    4 => self.array(info),
                    5 => self.map(info),
                    6 => self.tag(info),
                    _ => self.simple(info),
                };
                self.depth -= 1;
                item
            }
        }
    }

    /// `_parse_unsigned_integer`.
    fn unsigned(&mut self, info: u8) -> Result<u64, ParseError> {
        let len = match info {
            0..24 => return Ok(u64::from(info)),
            24 => 1,
            25 => 2,
            26 => 4,
            27 => 8,
            _ => {
                return Err(ParseError::new(format!(
                    "Invalid CBOR integer returned from the service; unparsable additional \
                     info found for major type 0 or 1: {info}"
                )));
            }
        };
        let bytes = self.read(len)?;
        Ok(bytes
            .iter()
            .fold(0, |value, byte| (value << 8) | u64::from(*byte)))
    }

    /// `_parse_byte_string`: definite, or chunks to a break, each chunk's
    /// length read whatever its major type.
    fn bytes(&mut self, info: u8) -> Result<Vec<u8>, ParseError> {
        if info != 31 {
            let len = self.unsigned(info)?;
            return Ok(self.read(len)?.to_vec());
        }
        let mut joined = Vec::new();
        while !self.at_break() {
            let info = self.byte()? & 0x1f;
            let len = self.unsigned(info)?;
            joined.extend_from_slice(self.read(len)?);
        }
        Ok(joined)
    }

    /// `_parse_array`. A length is never trusted to reserve room.
    fn array(&mut self, info: u8) -> Result<Item, ParseError> {
        let mut items = Vec::new();
        if info == 31 {
            while !self.at_break() {
                items.push(self.item()?);
            }
        } else {
            for _ in 0..self.unsigned(info)? {
                items.push(self.item()?);
            }
        }
        Ok(Item::List(items))
    }

    /// `_parse_map`: each key, then its value, then `items[key] = value`.
    fn map(&mut self, info: u8) -> Result<Item, ParseError> {
        let mut entries = Vec::new();
        let mut entry = |this: &mut Self| -> Result<(), ParseError> {
            let key = this.item()?;
            let value = this.item()?;
            insert(&mut entries, key, value)
        };
        if info == 31 {
            while !self.at_break() {
                entry(self)?;
            }
        } else {
            for _ in 0..self.unsigned(info)? {
                entry(self)?;
            }
        }
        Ok(Item::Map(entries))
    }

    /// `_parse_tag`: the value is read before the tag is judged; tag 1 is
    /// `_parse_datetime`, the timestamp parser of an `int` or `float`
    /// (`bool` among them).
    fn tag(&mut self, info: u8) -> Result<Item, ParseError> {
        let tag = self.unsigned(info)?;
        let value = self.item()?;
        if tag != 1 {
            return Err(ParseError::new(format!(
                "Found CBOR tag not supported by botocore: {tag}"
            )));
        }
        match value {
            Item::Scalar(
                number @ (Value::Integer(_)
                | Value::BigInteger(_)
                | Value::Float(_)
                | Value::Bool(_)),
            ) => Ok(Item::Scalar((self.timestamp)(&number)?)),
            other => Err(ParseError::new(format!(
                "Unable to parse datetime value: {}",
                other.py_str()
            ))),
        }
    }

    /// `_parse_simple_and_float`.
    fn simple(&mut self, info: u8) -> Result<Item, ParseError> {
        let value = match info {
            20 => Value::Bool(false),
            21 => Value::Bool(true),
            22 | 23 => Value::Null,
            25 => {
                let bytes = self.read(2)?;
                Value::Float(half(u16::from_be_bytes([bytes[0], bytes[1]])))
            }
            26 => {
                let bytes = self.read(4)?;
                let bits = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                Value::Float(f64::from(f32::from_bits(bits)))
            }
            27 => {
                let mut bits = [0; 8];
                bits.copy_from_slice(self.read(8)?);
                Value::Float(f64::from_be_bytes(bits))
            }
            _ => {
                return Err(ParseError::new(format!(
                    "Invalid additional info found for major type 7: {info}.  This indicates \
                     an unsupported simple type or an indefinite float value"
                )));
            }
        };
        Ok(Item::Scalar(value))
    }
}

/// A Python `int`.
fn int(value: i128) -> Value {
    i64::try_from(value).map_or_else(
        |_| Value::BigInteger(value.to_string().into()),
        Value::Integer,
    )
}

/// `struct.unpack('>e')`: an IEEE half-precision float.
fn half(bits: u16) -> f64 {
    let sign = if bits & 0x8000 == 0 { 1.0 } else { -1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let fraction = f64::from(bits & 0x3ff);
    sign * match exponent {
        0 => fraction * 2f64.powi(-24),
        31 if fraction == 0.0 => f64::INFINITY,
        31 => f64::NAN,
        _ => (1.0 + fraction / 1024.0) * 2f64.powi(exponent - 15),
    }
}

/// `items[key] = value` in a `dict`: a key equal to one held replaces its
/// value in place.
fn insert(entries: &mut Vec<(Item, Item)>, key: Item, value: Item) -> Result<(), ParseError> {
    let Item::Scalar(scalar) = &key else {
        return Err(unhashable(&key));
    };
    if matches!(scalar, Value::List(_) | Value::Map(_)) {
        return Err(unhashable(&key));
    }
    let held = entries.iter_mut().find(|(held, _)| match held {
        Item::Scalar(held) => same_key(held, scalar),
        _ => false,
    });
    match held {
        Some((_, old)) => *old = value,
        None => entries.push((key, value)),
    }
    Ok(())
}

fn unhashable(key: &Item) -> ParseError {
    ParseError::new(format!("unhashable type: '{}'", key.type_name()))
}

/// Python's `==` between two hashable keys: numbers by value across
/// `bool`, `int` and `float` (`nan` equal to nothing, as a decoded `nan`
/// is never the same object), anything else within its own type.
fn same_key(a: &Value, b: &Value) -> bool {
    match (number(a), number(b)) {
        (Some(x), Some(y)) => match (x, y) {
            (Number::Int(x), Number::Int(y)) => x == y,
            (Number::Float(x), Number::Float(y)) => x == y,
            (Number::Int(int), Number::Float(float)) | (Number::Float(float), Number::Int(int)) => {
                float.fract() == 0.0 && float.abs() < 2f64.powi(127) && float as i128 == int
            }
        },
        (None, None) => a == b,
        _ => false,
    }
}

enum Number {
    Int(i128),
    Float(f64),
}

fn number(value: &Value) -> Option<Number> {
    match value {
        Value::Bool(flag) => Some(Number::Int(i128::from(*flag))),
        Value::Integer(number) => Some(Number::Int(i128::from(*number))),
        Value::BigInteger(text) => text.parse().ok().map(Number::Int),
        Value::Float(number) => Some(Number::Float(*number)),
        _ => None,
    }
}

impl Item {
    /// `type(item).__name__`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Scalar(value) => py_short_type_name(value),
            Self::List(_) => "list",
            Self::Map(_) => "dict",
        }
    }

    /// `dict.get(key)` of a text key; `None` for a missing key or an item
    /// that is no map.
    pub fn get(&self, key: &str) -> Option<&Self> {
        let Self::Map(entries) = self else {
            return None;
        };
        entries.iter().find_map(|(held, value)| match held {
            Self::Scalar(Value::String(text)) if text == key => Some(value),
            _ => None,
        })
    }

    /// The item as a parsed value. A map key that is not text (botocore
    /// keeps it, but no output the CLI writes holds one the way Python's
    /// does) fails the call, naming the native limit: the request was sent,
    /// so the Python route cannot take it over.
    ///
    /// # Errors
    /// See above.
    pub fn to_value(&self) -> Result<Value, ParseError> {
        Ok(match self {
            Self::Scalar(value) => value.clone(),
            Self::List(items) => {
                Value::List(items.iter().map(Self::to_value).collect::<Result<_, _>>()?)
            }
            Self::Map(entries) => {
                let mut map = Map::with_capacity(entries.len());
                for (key, value) in entries {
                    map.insert_key(key.text_key()?, value.to_value()?);
                }
                Value::Map(map)
            }
        })
    }

    /// The item as a map key: text, else the native limit [`Self::to_value`]
    /// names.
    ///
    /// # Errors
    /// See above.
    pub fn text_key(&self) -> Result<Key, ParseError> {
        match self {
            Self::Scalar(Value::String(text)) => Ok(Key::Str(text.clone())),
            Self::Scalar(Value::PyString(text)) => Ok(Key::Py(text.clone())),
            other => Err(ParseError::new(format!(
                "native client limit: the response holds a CBOR map key that is not text ({}), \
                 which the native client does not represent",
                other.repr()
            ))),
        }
    }

    /// `repr(item)`.
    pub fn repr(&self) -> String {
        match self {
            Self::Scalar(value) => py_repr(value),
            Self::List(items) => {
                let items: Vec<String> = items.iter().map(Self::repr).collect();
                format!("[{}]", items.join(", "))
            }
            Self::Map(entries) => {
                let entries: Vec<String> = (entries.iter())
                    .map(|(key, value)| format!("{}: {}", key.repr(), value.repr()))
                    .collect();
                format!("{{{}}}", entries.join(", "))
            }
        }
    }

    /// `str(item)`.
    pub fn py_str(&self) -> String {
        match self {
            Self::Scalar(value) => py_str(value),
            other => other.repr(),
        }
    }
}
