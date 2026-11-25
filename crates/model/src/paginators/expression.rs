//! A paginator's expression: what `jmespath.compile` gives for an output
//! token, a result key, a non-aggregate key or `more_results`, limited to
//! JMESPath unquoted identifiers joined by dots, which is what botocore's
//! own paginators file uses but for a handful of entries. Read with
//! [`Expression::search`] (`jmespath.search`) and written with
//! [`Expression::set`] (`set_value_from_jmespath`), over any object model
//! that implements [`Lookup`] and [`Assign`]: plain [`Value`]s here, and
//! the paginator's shared objects in the client.
use crate::pyrepr::py_type_name;
use crate::value::{Map, Value};

/// A member name, or a dotted path of member names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expression(Box<str>);

impl Expression {
    /// `text`, when every dot-separated part is a JMESPath unquoted
    /// identifier (`[A-Za-z_][A-Za-z0-9_]*`).
    pub fn parse(text: &str) -> Option<Self> {
        let identifier = |part: &str| {
            part.chars()
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
                && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        };
        text.split('.').all(identifier).then(|| Self(text.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The member names, outermost first.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.split('.')
    }

    /// `jmespath.search(expression, value)`: each name looked up in a
    /// dict; `None` once one is missing, the object it is looked up in is
    /// no dict, or the object found is `None`.
    pub fn search<T: Lookup>(&self, value: T) -> Option<T> {
        self.names()
            .try_fold(value, |found, name| found.lookup(name))
            .filter(|found| !found.is_none())
    }

    /// `set_value_from_jmespath(source, expression, value)`: each name but
    /// the last made an empty dict where its parent lacks it, and the last
    /// assigned.
    ///
    /// # Errors
    /// The `TypeError` Python's `in` or item assignment raises on the
    /// first object of the path that is no dict, as `str()` gives it.
    pub fn set<T: Assign>(&self, source: T, value: T::Item) -> Result<(), String> {
        let mut names = self.names().peekable();
        let mut current = source;
        while let Some(name) = names.next() {
            let last = names.peek().is_none();
            if let Some(found) = current.not_a_dict() {
                return Err(type_error(&found, name, !last));
            }
            if last {
                current.assign(name, value);
                return Ok(());
            }
            match current.child(name) {
                Some(child) => current = child,
                None => return Ok(()),
            }
        }
        Ok(())
    }
}

impl std::fmt::Display for Expression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl PartialEq<&str> for Expression {
    fn eq(&self, other: &&str) -> bool {
        *self.0 == **other
    }
}

/// An object `jmespath.search` looks names up in.
pub trait Lookup: Sized {
    /// `self[name]` of a dict; `None` for a missing name or anything else.
    fn lookup(&self, name: &str) -> Option<Self>;
    /// Whether this is Python's `None`.
    fn is_none(&self) -> bool;
}

/// An object `set_value_from_jmespath` writes through.
pub trait Assign: Sized {
    /// What is assigned at the path's end.
    type Item;
    /// What this is when it is no dict; `None` for a dict.
    fn not_a_dict(&self) -> Option<NotADict<'_>>;
    /// `if name not in self: self[name] = {}`, then `self[name]`, of a
    /// dict.
    fn child(self, name: &str) -> Option<Self>;
    /// `self[name] = value`, of a dict.
    fn assign(self, name: &str, value: Self::Item);
}

/// An object that is no dict, as much of it as Python's `TypeError` texts
/// read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotADict<'a> {
    /// A `str`, in `pytext::wtf8`'s encoding: a lone surrogate's too.
    Str(&'a [u8]),
    Bytes,
    List,
    /// Anything else, by its type's name (`tp_name`).
    Other(&'static str),
}

impl<'a> NotADict<'a> {
    /// What `value` is, or `None` for a dict.
    pub fn of(value: &'a Value) -> Option<Self> {
        match value {
            Value::Map(_) => None,
            Value::String(text) => Some(Self::Str(text.as_bytes())),
            Value::PyString(text) => Some(Self::Str(text.as_bytes())),
            Value::Blob(_) => Some(Self::Bytes),
            Value::List(_) => Some(Self::List),
            other => Some(Self::Other(py_type_name(other))),
        }
    }
}

/// What `set_value_from_jmespath` raises on `found`, no dict: with more of
/// the path, `name in found` and then `found[name]` read or assigned; at
/// its end, `found[name] = value`.
fn type_error(found: &NotADict<'_>, name: &str, more: bool) -> String {
    match found {
        NotADict::List => "list indices must be integers or slices, not str".to_owned(),
        // `name in text`: `name` is Rust text, which matches no part of a
        // surrogate's three bytes, so a byte search finds what Python does.
        NotADict::Str(text) if more && contains(text, name.as_bytes()) => {
            "string indices must be integers, not 'str'".to_owned()
        }
        NotADict::Bytes if more => "a bytes-like object is required, not 'str'".to_owned(),
        NotADict::Str(_) => "'str' object does not support item assignment".to_owned(),
        NotADict::Bytes => "'bytes' object does not support item assignment".to_owned(),
        NotADict::Other(class) if more => {
            format!("argument of type '{class}' is not a container or iterable")
        }
        NotADict::Other(class) => format!("'{class}' object does not support item assignment"),
    }
}

fn contains(text: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || text.windows(needle.len()).any(|window| window == needle)
}

impl Lookup for &Value {
    fn lookup(&self, name: &str) -> Option<Self> {
        match self {
            Value::Map(map) => map.get(name),
            _ => None,
        }
    }
    fn is_none(&self) -> bool {
        matches!(self, Value::Null)
    }
}

impl Assign for &mut Value {
    type Item = Value;
    fn not_a_dict(&self) -> Option<NotADict<'_>> {
        NotADict::of(self)
    }
    fn child(self, name: &str) -> Option<Self> {
        let Value::Map(map) = self else {
            return None;
        };
        if !map.contains_key(name) {
            map.insert(name.to_owned(), Value::Map(Map::new()));
        }
        map.get_mut(name)
    }
    fn assign(self, name: &str, value: Value) {
        if let Value::Map(map) = self {
            map.insert(name.to_owned(), value);
        }
    }
}

#[cfg(test)]
mod tests;
