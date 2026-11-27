//! A paginator's expression: what `jmespath.compile` gives for an output
//! token, a result key, a non-aggregate key or `more_results`. A member
//! name or a dotted path of them, which is what botocore's own paginators
//! file uses but for a handful of entries, is read with
//! [`Expression::search`] name by name and written with
//! [`Expression::set`] (`set_value_from_jmespath`), over any object model
//! that implements [`Lookup`] and [`Assign`]: plain [`Value`]s here, and
//! the paginator's shared objects in the client. Any other expression is
//! evaluated by the `jmespath` engine, within [`PAGINATORS`].
use crate::pyjmespath::Python;
use crate::pyrepr::py_type_name;
use crate::value::{Map, Value};
use jmespath::{Feature, Subset};
use std::sync::Arc;

/// The JMESPath a paginator evaluates beyond a dotted path: every
/// construct whose evaluation cannot raise. Comparators (`<` of a number
/// and a string raises `TypeError`), slices (a zero step raises
/// `ValueError`), functions and expression references are outside it.
pub const PAGINATORS: Subset = Subset {
    features: &[
        Feature::Identifier,
        Feature::Subexpression,
        Feature::Index,
        Feature::ListWildcard,
        Feature::ObjectWildcard,
        Feature::Filter,
        Feature::Flatten,
        Feature::Current,
        Feature::Literal,
        Feature::MultiSelectList,
        Feature::MultiSelectHash,
        Feature::Logic,
        Feature::Pipe,
    ],
    functions: &[],
};

/// A compiled paginator expression.
#[derive(Clone, Debug)]
pub struct Expression {
    text: Box<str>,
    /// The compiled expression, unless it is a name or a dotted path.
    compiled: Option<Arc<jmespath::Expression<Value>>>,
}

impl Expression {
    /// `jmespath.compile(text)`: a name or a dotted path of JMESPath
    /// unquoted identifiers (`[A-Za-z_][A-Za-z0-9_]*`), or any other
    /// expression within [`PAGINATORS`]; `None` for text that does not
    /// compile or is outside it.
    pub fn parse(text: &str) -> Option<Self> {
        let identifier = |part: &str| {
            part.chars()
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
                && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        };
        let compiled = if text.split('.').all(identifier) {
            None
        } else {
            let compiled = jmespath::Expression::parse(&Python, text).ok()?;
            compiled.check(&PAGINATORS).ok()?;
            Some(Arc::new(compiled))
        };
        Some(Self {
            text: text.into(),
            compiled,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Whether this is a name or a dotted path of them, which
    /// `set_value_from_jmespath` writes and [`Lookup`] reads through
    /// shared objects.
    pub fn is_path(&self) -> bool {
        self.compiled.is_none()
    }

    /// The parts `set_value_from_jmespath` splits the text into at dots:
    /// a path's member names, outermost first.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.text.split('.')
    }

    /// `jmespath.search(expression, value)`, `None` where it finds `None`.
    /// A path looks each name up in a dict, and is `None` once one is
    /// missing or the object it is looked up in is no dict; any other
    /// expression is [`Lookup::evaluate`]d.
    pub fn search<T: Lookup>(&self, value: T) -> Option<T> {
        let found = match &self.compiled {
            None => self
                .names()
                .try_fold(value, |found, name| found.lookup(name)),
            Some(_) => value.evaluate(self),
        };
        found.filter(|found| !found.is_none())
    }

    /// `jmespath.search(expression, value)` of a [`Value`], as a new one;
    /// `Value::Null` for `None`. Every expression [`Self::parse`] gives
    /// evaluates without raising, so an error never reaches here.
    pub fn find(&self, value: &Value) -> Value {
        match &self.compiled {
            None => self.search(value).cloned().unwrap_or(Value::Null),
            Some(compiled) => compiled
                .search(&Python, value)
                .map_or(Value::Null, std::borrow::Cow::into_owned),
        }
    }

    /// `set_value_from_jmespath(source, expression, value)`: each name but
    /// the last made an empty dict where its parent lacks it, and the last
    /// assigned.
    ///
    /// # Errors
    /// The `TypeError` Python's `in` or item assignment raises on the
    /// first object of the path that is no dict, as `str()` gives it; or
    /// `InvalidExpressionError`'s text, for an expression with `[`, `]`
    /// or `*`.
    pub fn set<T: Assign>(&self, source: T, value: T::Item) -> Result<(), String> {
        if self.text.contains(['[', ']', '*']) {
            return Err(format!(
                "Invalid expression {}: Only dotted lookups are supported.",
                self.text
            ));
        }
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

/// Two expressions are equal when their texts are: one text compiles to
/// one expression.
impl PartialEq for Expression {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl Eq for Expression {}

impl std::fmt::Display for Expression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl PartialEq<&str> for Expression {
    fn eq(&self, other: &&str) -> bool {
        *self.text == **other
    }
}

/// An object `jmespath.search` reads.
pub trait Lookup: Sized {
    /// `self[name]` of a dict; `None` for a missing name or anything else.
    fn lookup(&self, name: &str) -> Option<Self>;
    /// Whether this is Python's `None`.
    fn is_none(&self) -> bool;
    /// `jmespath.search(expression, self)` of an expression beyond a
    /// dotted path, which [`Expression::find`] evaluates on a copy of
    /// what this object holds now: what it finds, as an object of this
    /// model.
    fn evaluate(&self, expression: &Expression) -> Option<Self>;
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

/// A page's own values, borrowed: what a path finds is the object itself.
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
    /// A borrow holds no value the search builds, so it finds nothing:
    /// read such an expression with [`Expression::find`]. The paginator
    /// reads one only as an output token or `more_results`
    /// ([`super::Config`]), through its own objects.
    fn evaluate(&self, _: &Expression) -> Option<Self> {
        None
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
