//! A page as Python holds it: each dict and list one object, which every
//! reference to it shares. botocore's paginator never copies a page:
//! `_record_non_aggregate_key_values` keeps references into the first one,
//! `build_full_result` keeps the first list of each result key and
//! `extend`s it, and `set_value_from_jmespath` writes into the dicts it
//! walks, so each sees the others' changes. Everything else is a value no
//! one changes in place.
//!
//! No borrow of an object is held while another is borrowed, so the same
//! object reached twice is never borrowed against itself.
use super::py;
use model::value::{Key, Map, Value};
use std::cell::RefCell;
use std::rc::Rc;

/// A dict's entries, in insertion order.
type Entries = Vec<(Key, Object)>;

/// A Python object.
#[derive(Clone, Debug)]
pub(super) enum Object {
    /// A `str`, number, `bool`, `None`, `bytes` or `datetime`: never a
    /// [`Value::List`] or [`Value::Map`].
    Value(Value),
    List(Rc<RefCell<Vec<Object>>>),
    Dict(Rc<RefCell<Entries>>),
}

impl Object {
    pub(super) const NONE: Self = Self::Value(Value::Null);

    pub(super) fn dict() -> Self {
        Self::Dict(Rc::default())
    }

    pub(super) fn list(items: Vec<Self>) -> Self {
        Self::List(Rc::new(RefCell::new(items)))
    }

    /// `value` as new objects, none shared.
    pub(super) fn new(value: Value) -> Self {
        match value {
            Value::List(items) => Self::list(items.into_iter().map(Self::new).collect()),
            Value::Map(map) => Self::Dict(Rc::new(RefCell::new(
                map.into_iter()
                    .map(|(key, value)| (key, Self::new(value)))
                    .collect(),
            ))),
            value => Self::Value(value),
        }
    }

    /// What the object holds now, copied.
    pub(super) fn value(&self) -> Value {
        match self {
            Self::Value(value) => value.clone(),
            Self::List(items) => Value::List(items.borrow().iter().map(Self::value).collect()),
            Self::Dict(entries) => Value::Map(
                entries
                    .borrow()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.value()))
                    .collect(),
            ),
        }
    }

    /// [`Self::value`] of a dict.
    pub(super) fn map(&self) -> Map {
        match self.value() {
            Value::Map(map) => map,
            _ => Map::new(),
        }
    }

    pub(super) fn is_none(&self) -> bool {
        matches!(self, Self::Value(Value::Null))
    }

    /// `type(self).__name__`.
    pub(super) fn type_name(&self) -> &'static str {
        match self {
            Self::Value(value) => py::type_name(value),
            Self::List(_) => "list",
            Self::Dict(_) => "dict",
        }
    }

    /// `bool(self)`.
    pub(super) fn truthy(&self) -> bool {
        match self {
            Self::Value(value) => py::truthy(Some(value)),
            Self::List(items) => !items.borrow().is_empty(),
            Self::Dict(entries) => !entries.borrow().is_empty(),
        }
    }

    /// `len(self)`.
    pub(super) fn len(&self) -> Result<i128, String> {
        let count = match self {
            Self::Value(value) => return py::len(value),
            Self::List(items) => items.borrow().len(),
            Self::Dict(entries) => entries.borrow().len(),
        };
        Ok(i128::try_from(count).unwrap_or(i128::MAX))
    }

    /// `self[key]` of a dict, the object itself.
    pub(super) fn get(&self, key: &str) -> Option<Self> {
        self.get_key(&Key::from(key))
    }

    /// [`Self::get`] of any key, a lone surrogate's included.
    pub(super) fn get_key(&self, key: &Key) -> Option<Self> {
        let Self::Dict(entries) = self else {
            return None;
        };
        let entries = entries.borrow();
        entries
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
    }

    /// `self[key] = value` of a dict: a key it has keeps its place.
    ///
    /// # Errors
    /// The `TypeError` of anything else.
    pub(super) fn insert(&self, key: &str, value: Self) -> Result<(), String> {
        self.insert_key(Key::from(key), value)
    }

    /// [`Self::insert`] of any key.
    ///
    /// # Errors
    /// As [`Self::insert`].
    pub(super) fn insert_key(&self, key: Key, value: Self) -> Result<(), String> {
        let Self::Dict(entries) = self else {
            return Err(format!(
                "'{}' object does not support item assignment",
                self.type_name()
            ));
        };
        let mut entries = entries.borrow_mut();
        match entries.iter_mut().find(|(name, _)| *name == key) {
            Some((_, old)) => *old = value,
            None => entries.push((key, value)),
        }
        Ok(())
    }

    /// A dict's entries as they are now, each value the object itself.
    pub(super) fn entries(&self) -> Entries {
        match self {
            Self::Dict(entries) => entries.borrow().clone(),
            _ => Vec::new(),
        }
    }

    /// `self.extend(items)` of a list, with another list's items.
    ///
    /// # Errors
    /// The `AttributeError` of anything else.
    pub(super) fn extend(&self, items: &Self) -> Result<(), String> {
        let Self::List(list) = self else {
            return Err(format!(
                "'{}' object has no attribute 'extend'",
                self.type_name()
            ));
        };
        let added = match items {
            Self::List(items) => items.borrow().clone(),
            _ => Vec::new(),
        };
        list.borrow_mut().extend(added);
        Ok(())
    }

    /// `self[start:]` of a list or `str`: a new list of the same items.
    pub(super) fn slice_from(&self, start: &Value) -> Result<Self, String> {
        match self {
            Self::Value(value) => py::slice_from(value, start).map(Self::Value),
            Self::List(items) => {
                let items = items.borrow();
                let start = py::slice_start(start, items.len())?;
                Ok(Self::list(items[start..].to_vec()))
            }
            Self::Dict(_) => Ok(self.clone()),
        }
    }

    /// `self[:stop]`: a new list of the same items; a dict looks the slice
    /// up, a `KeyError`, as slices hash since Python 3.12.
    pub(super) fn slice_to(&self, stop: i128) -> Result<Self, String> {
        match self {
            Self::Value(value) => py::slice_to(value, stop).map(Self::Value),
            Self::List(items) => {
                let items = items.borrow();
                let end = py::slice_stop(stop, items.len());
                Ok(Self::list(items[..end].to_vec()))
            }
            Self::Dict(_) => Err(format!("slice(None, {stop}, None)")),
        }
    }
}
