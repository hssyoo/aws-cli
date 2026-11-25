//! The dynamic value every request input and response output passes through:
//! the Python objects botocore's serializers take and its parsers build.
pub use pytext::wtf8::Wtf8Buf;
use timestamp::Timestamp;

/// A Python value as botocore sees one.
///
/// Integers are unbounded in Python; those beyond `i64` keep their decimal
/// text. `Float` is Python's `float`. `Timestamp` stands for an aware
/// `datetime.datetime` in `datetime.timezone.utc`, `Blob` for `bytes`.
/// A `str` is `String`, or `PyString` when it holds a lone surrogate, which
/// a server's JSON can send and a Rust `String` cannot hold.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Integer(i64),
    /// An integer outside `i64`, as canonical decimal text (`str(int)`).
    BigInteger(Box<str>),
    Float(f64),
    String(String),
    /// A `str` holding at least one lone surrogate; never one a `String`
    /// could hold, so the two variants never stand for the same text.
    PyString(Wtf8Buf),
    Blob(Vec<u8>),
    Timestamp(Timestamp),
    List(Vec<Value>),
    Map(Map),
}

/// A `dict` key: a `str`, which holds lone surrogates as a value's may.
/// Keys a model or the CLI makes are always `Str`. Keys order as Python
/// sorts `str`s, by code point, whichever variant each is.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Str(String),
    /// A key holding at least one lone surrogate, as [`Value::PyString`].
    Py(Wtf8Buf),
}

impl Key {
    /// The key as Rust text; `None` for one with a lone surrogate.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(text) => Some(text),
            Self::Py(_) => None,
        }
    }

    /// The key's text in `pytext::wtf8`'s encoding.
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Str(text) => text.as_bytes(),
            Self::Py(text) => text.as_bytes(),
        }
    }

    /// The key as Rust text for a message: a lone surrogate as `\udXXX`,
    /// as stderr's `backslashreplace` prints it.
    pub fn text(&self) -> std::borrow::Cow<'_, str> {
        match self {
            Self::Str(text) => std::borrow::Cow::Borrowed(text),
            Self::Py(text) => pytext::codec::backslashreplace(text.as_bytes()),
        }
    }

    /// The key as the `str` value it is.
    pub fn to_value(&self) -> Value {
        match self {
            Self::Str(text) => Value::String(text.clone()),
            Self::Py(text) => Value::PyString(text.clone()),
        }
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// By code point: the bytes of `pytext::wtf8`'s encoding order as the code
/// points do.
impl Ord for Key {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_bytes().cmp(other.as_bytes())
    }
}

impl From<String> for Key {
    fn from(text: String) -> Self {
        Self::Str(text)
    }
}

impl From<&str> for Key {
    fn from(text: &str) -> Self {
        Self::Str(text.to_owned())
    }
}

impl From<Wtf8Buf> for Key {
    fn from(text: Wtf8Buf) -> Self {
        Self::Py(text)
    }
}

impl From<pytext::wtf8::Text> for Key {
    fn from(text: pytext::wtf8::Text) -> Self {
        text.map_or_else(Self::Py, Self::Str)
    }
}

impl PartialEq<str> for Key {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == Some(other)
    }
}

/// A string-keyed map in insertion order, as a Python `dict` keeps it.
///
/// Lookups are linear: maps here are request and response structures, which
/// are small. A key is compared as the `str` it is: one with a lone
/// surrogate is never equal to a Rust `&str`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Map {
    entries: Vec<(Key, Value)>,
}

impl Map {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
        }
    }

    /// `dict[key] = value`: a present key keeps its position and returns the
    /// value it had; a new key goes last.
    pub fn insert(&mut self, key: String, value: Value) -> Option<Value> {
        self.insert_key(Key::Str(key), value)
    }

    /// [`Self::insert`] of any key, a lone surrogate's included.
    pub fn insert_key(&mut self, key: Key, value: Value) -> Option<Value> {
        match self.entries.iter_mut().find(|(k, _)| *k == key) {
            Some((_, old)) => Some(std::mem::replace(old, value)),
            None => {
                self.entries.push((key, value));
                None
            }
        }
    }

    /// `dict.get(key)` of a key without a lone surrogate.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// `dict.get(key)` of any key.
    pub fn get_key(&self, key: &Key) -> Option<&Value> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.entries
            .iter_mut()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// `dict.pop(key, None)`: the other keys keep their order.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let index = self.entries.iter().position(|(k, _)| k == key)?;
        Some(self.entries.remove(index).1)
    }

    /// [`Self::remove`] of any key.
    pub fn remove_key(&mut self, key: &Key) -> Option<Value> {
        let index = self.entries.iter().position(|(k, _)| k == key)?;
        Some(self.entries.remove(index).1)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn keys(&self) -> impl Iterator<Item = &Key> {
        self.entries.iter().map(|(k, _)| k)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Key, &Value)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }
}

impl IntoIterator for Map {
    type Item = (Key, Value);
    type IntoIter = std::vec::IntoIter<(Key, Value)>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl FromIterator<(String, Value)> for Map {
    fn from_iter<I: IntoIterator<Item = (String, Value)>>(items: I) -> Self {
        let mut map = Self::new();
        for (key, value) in items {
            map.insert(key, value);
        }
        map
    }
}

impl FromIterator<(Key, Value)> for Map {
    fn from_iter<I: IntoIterator<Item = (Key, Value)>>(items: I) -> Self {
        let mut map = Self::new();
        for (key, value) in items {
            map.insert_key(key, value);
        }
        map
    }
}

/// Why JSON text did not parse.
#[derive(Debug)]
pub struct JsonError(serde_json::Error);

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for JsonError {}

impl Value {
    /// The `str` value of `text`: a `String`, or a `PyString` for text with
    /// a lone surrogate.
    pub fn from_text(text: pytext::wtf8::Text) -> Self {
        text.map_or_else(Self::PyString, Self::String)
    }

    /// A map's value for `key`, as `dict.get` reads it; `None` when absent
    /// or when this is not a map.
    pub fn get(&self, key: &str) -> Option<&Self> {
        self.as_map()?.get(key)
    }

    pub const fn as_map(&self) -> Option<&Map> {
        match self {
            Self::Map(map) => Some(map),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text),
            _ => None,
        }
    }

    /// `json.loads(text)`, with Python's number rules: a number with a
    /// fraction or exponent is a `float` (`1.50` is `1.5`, `1e400` is
    /// infinity), any other is an `int` of any size.
    ///
    /// Two inputs Python accepts are refused here: the `NaN` and `Infinity`
    /// literals, and lone surrogate escapes, which no Rust string can hold.
    pub fn from_json(text: &[u8]) -> Result<Self, JsonError> {
        let json: serde_json::Value = serde_json::from_slice(text).map_err(JsonError)?;
        Ok(Self::from_serde(&json))
    }

    /// The value of a parsed JSON document, with [`Value::from_json`]'s rules.
    pub fn from_serde(json: &serde_json::Value) -> Self {
        match json {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(value) => Self::Bool(*value),
            serde_json::Value::Number(number) => Self::from_number_text(number.as_str()),
            serde_json::Value::String(text) => Self::String(text.clone()),
            serde_json::Value::Array(items) => {
                Self::List(items.iter().map(Self::from_serde).collect())
            }
            serde_json::Value::Object(object) => Self::Map(
                object
                    .iter()
                    .map(|(key, value)| (key.clone(), Self::from_serde(value)))
                    .collect(),
            ),
        }
    }

    /// `json.decoder`'s number rule: `float(text)` when the text has a
    /// fraction or exponent, otherwise `int(text)`.
    fn from_number_text(text: &str) -> Self {
        if text.contains(['.', 'e', 'E']) {
            return Self::Float(text.parse().unwrap_or(f64::NAN));
        }
        match text.parse::<i64>() {
            Ok(value) => Self::Integer(value),
            Err(_) => Self::BigInteger(text.into()),
        }
    }
}

#[cfg(test)]
mod tests;
