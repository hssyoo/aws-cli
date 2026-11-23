//! The dynamic value every request input and response output passes through:
//! the Python objects botocore's serializers take and its parsers build.
use timestamp::Timestamp;

/// A Python value as botocore sees one.
///
/// Integers are unbounded in Python; those beyond `i64` keep their decimal
/// text. `Float` is Python's `float`. `Timestamp` stands for an aware
/// `datetime.datetime` in `datetime.timezone.utc`, `Blob` for `bytes`.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Integer(i64),
    /// An integer outside `i64`, as canonical decimal text (`str(int)`).
    BigInteger(Box<str>),
    Float(f64),
    String(String),
    Blob(Vec<u8>),
    Timestamp(Timestamp),
    List(Vec<Value>),
    Map(Map),
}

/// A string-keyed map in insertion order, as a Python `dict` keeps it.
///
/// Lookups are linear: maps here are request and response structures, which
/// are small.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Map {
    entries: Vec<(String, Value)>,
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
        match self.entries.iter_mut().find(|(k, _)| *k == key) {
            Some((_, old)) => Some(std::mem::replace(old, value)),
            None => {
                self.entries.push((key, value));
                None
            }
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
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

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(k, _)| k.as_str())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }
}

impl IntoIterator for Map {
    type Item = (String, Value);
    type IntoIter = std::vec::IntoIter<(String, Value)>;

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

    pub fn as_list(&self) -> Option<&[Self]> {
        match self {
            Self::List(items) => Some(items),
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
